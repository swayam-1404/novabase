//! Checksummed write-ahead log and redo recovery for NovaDB.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use nova_core::error::{NovaError, Result};
use nova_storage::{Page, PageId, PageManager, PAGE_SIZE};

const MAGIC: &[u8; 4] = b"NVWL";
const VERSION: u16 = 1;
const HEADER_SIZE: usize = 36;
const CHECKSUM_OFFSET: usize = 28;
const MAX_PAYLOAD: usize = PAGE_SIZE + 8;

/// Monotonically increasing log sequence number.
pub type Lsn = u64;
/// Transaction identifier carried by every record.
pub type TransactionId = u64;

/// Durable WAL operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// Starts a transaction.
    Begin,
    /// Stores a complete validated page after-image.
    PageWrite { page_id: PageId, page: Box<Page> },
    /// Makes all preceding writes for the transaction recoverable.
    Commit,
    /// Marks the transaction aborted.
    Abort,
    /// Establishes a synchronization point.
    Checkpoint,
}

/// One decoded WAL record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    pub lsn: Lsn,
    pub transaction_id: TransactionId,
    pub operation: Operation,
}

/// Append-only write-ahead log.
#[derive(Debug)]
pub struct WriteAheadLog {
    path: PathBuf,
    file: File,
    records: Vec<LogRecord>,
    next_lsn: Lsn,
    durable_lsn: Option<Lsn>,
}

impl WriteAheadLog {
    /// Opens or creates a WAL, validating records and discarding a torn tail.
    ///
    /// # Errors
    ///
    /// Returns a typed I/O, unsupported-version, or corruption error.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let (records, valid_length) = decode_records(&bytes, true)?;
        if valid_length != bytes.len() {
            file.set_len(u64::try_from(valid_length).map_err(size_error)?)?;
        }
        file.seek(SeekFrom::End(0))?;
        let next_lsn = records
            .last()
            .map_or(0, |record| record.lsn.saturating_add(1));
        let durable_lsn = records.last().map(|record| record.lsn);
        Ok(Self {
            path,
            file,
            records,
            next_lsn,
            durable_lsn,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn records(&self) -> &[LogRecord] {
        &self.records
    }

    #[must_use]
    pub const fn durable_lsn(&self) -> Option<Lsn> {
        self.durable_lsn
    }

    /// Appends a begin record.
    ///
    /// # Errors
    /// Returns a typed encoding, LSN-overflow, or I/O error.
    pub fn begin(&mut self, transaction_id: TransactionId) -> Result<Lsn> {
        self.append(transaction_id, Operation::Begin)
    }

    /// Appends a full-page after-image.
    ///
    /// # Errors
    /// Returns a typed page-encoding, LSN-overflow, or I/O error.
    pub fn log_page_write(&mut self, transaction_id: TransactionId, page: &Page) -> Result<Lsn> {
        self.append(
            transaction_id,
            Operation::PageWrite {
                page_id: page.id(),
                page: Box::new(page.clone()),
            },
        )
    }

    /// Appends a commit record.
    ///
    /// # Errors
    /// Returns a typed encoding, LSN-overflow, or I/O error.
    pub fn commit(&mut self, transaction_id: TransactionId) -> Result<Lsn> {
        self.append(transaction_id, Operation::Commit)
    }

    /// Appends an abort record.
    ///
    /// # Errors
    /// Returns a typed encoding, LSN-overflow, or I/O error.
    pub fn abort(&mut self, transaction_id: TransactionId) -> Result<Lsn> {
        self.append(transaction_id, Operation::Abort)
    }

    /// Appends a checkpoint record using transaction id zero.
    ///
    /// # Errors
    /// Returns a typed encoding, LSN-overflow, or I/O error.
    pub fn checkpoint(&mut self) -> Result<Lsn> {
        self.append(0, Operation::Checkpoint)
    }

    /// Synchronizes every appended record and advances `durable_lsn`.
    ///
    /// # Errors
    /// Returns a typed I/O error if synchronization fails.
    pub fn flush(&mut self) -> Result<()> {
        self.file.sync_all()?;
        self.durable_lsn = self.records.last().map(|record| record.lsn);
        Ok(())
    }

    fn append(&mut self, transaction_id: TransactionId, operation: Operation) -> Result<Lsn> {
        let lsn = self.next_lsn;
        let record = LogRecord {
            lsn,
            transaction_id,
            operation,
        };
        let bytes = encode_record(&record)?;
        self.file.write_all(&bytes)?;
        self.records.push(record);
        self.next_lsn = self
            .next_lsn
            .checked_add(1)
            .ok_or_else(|| NovaError::Storage("WAL LSN overflow".to_owned()))?;
        Ok(lsn)
    }
}

/// Replays committed page after-images into a page file.
///
/// The operation is idempotent because each redo record replaces a complete
/// page. Incomplete tail records are ignored; complete corrupt records fail.
///
/// # Errors
///
/// Returns a typed WAL/page corruption, unsupported-version, or I/O error.
pub fn recover(wal_path: impl AsRef<Path>, page_path: impl AsRef<Path>) -> Result<usize> {
    let mut bytes = Vec::new();
    File::open(wal_path)?.read_to_end(&mut bytes)?;
    let (records, _) = decode_records(&bytes, true)?;
    let committed: BTreeSet<TransactionId> = records
        .iter()
        .filter_map(|record| {
            matches!(record.operation, Operation::Commit).then_some(record.transaction_id)
        })
        .collect();
    let aborted: BTreeSet<TransactionId> = records
        .iter()
        .filter_map(|record| {
            matches!(record.operation, Operation::Abort).then_some(record.transaction_id)
        })
        .collect();
    let mut pages = PageManager::open(page_path)?;
    let mut applied = 0;
    for record in records {
        let Operation::PageWrite { page_id, page } = record.operation else {
            continue;
        };
        if !committed.contains(&record.transaction_id) || aborted.contains(&record.transaction_id) {
            continue;
        }
        while pages.page_count() <= page_id.get() {
            pages.allocate()?;
        }
        pages.write(&page)?;
        applied += 1;
    }
    pages.sync()?;
    Ok(applied)
}

fn encode_record(record: &LogRecord) -> Result<Vec<u8>> {
    let (tag, payload) = match &record.operation {
        Operation::Begin => (0, Vec::new()),
        Operation::PageWrite { page_id, page } => {
            if page.id() != *page_id {
                return Err(NovaError::InvalidArgument(
                    "WAL page id does not match page header".to_owned(),
                ));
            }
            let mut payload = Vec::with_capacity(8 + PAGE_SIZE);
            payload.extend_from_slice(&page_id.get().to_be_bytes());
            payload.extend_from_slice(&page.to_bytes()?);
            (1, payload)
        }
        Operation::Commit => (2, Vec::new()),
        Operation::Abort => (3, Vec::new()),
        Operation::Checkpoint => (4, Vec::new()),
    };
    let mut bytes = Vec::with_capacity(HEADER_SIZE + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_be_bytes());
    bytes.push(tag);
    bytes.push(0);
    bytes.extend_from_slice(
        &u32::try_from(payload.len())
            .map_err(size_error)?
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&record.lsn.to_be_bytes());
    bytes.extend_from_slice(&record.transaction_id.to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&payload);
    let checksum = crc32(&bytes, CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4);
    bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_be_bytes());
    Ok(bytes)
}

fn decode_records(bytes: &[u8], allow_torn_tail: bool) -> Result<(Vec<LogRecord>, usize)> {
    let mut records = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        if bytes.len() - position < HEADER_SIZE {
            if allow_torn_tail {
                break;
            }
            return corruption("truncated WAL header");
        }
        let header = &bytes[position..position + HEADER_SIZE];
        if &header[..4] != MAGIC {
            return corruption("invalid WAL magic");
        }
        let version = u16::from_be_bytes(header[4..6].try_into().map_err(fixed_error)?);
        if version != VERSION {
            return Err(NovaError::Unsupported(format!("WAL version {version}")));
        }
        if header[7] != 0 || header[32..36] != [0; 4] {
            return corruption("WAL reserved field is non-zero");
        }
        let payload_len = usize::try_from(u32::from_be_bytes(
            header[8..12].try_into().map_err(fixed_error)?,
        ))
        .map_err(size_error)?;
        if payload_len > MAX_PAYLOAD {
            return corruption("WAL payload exceeds maximum");
        }
        let total = HEADER_SIZE
            .checked_add(payload_len)
            .ok_or_else(|| NovaError::Corruption("WAL length overflow".to_owned()))?;
        if bytes.len() - position < total {
            if allow_torn_tail {
                break;
            }
            return corruption("truncated WAL payload");
        }
        let encoded = &bytes[position..position + total];
        let stored = u32::from_be_bytes(header[28..32].try_into().map_err(fixed_error)?);
        if stored != crc32(encoded, CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4) {
            return corruption("WAL checksum mismatch");
        }
        let lsn = u64::from_be_bytes(header[12..20].try_into().map_err(fixed_error)?);
        if lsn != u64::try_from(records.len()).map_err(size_error)? {
            return corruption("WAL LSN sequence is not contiguous");
        }
        let transaction_id = u64::from_be_bytes(header[20..28].try_into().map_err(fixed_error)?);
        let payload = &encoded[HEADER_SIZE..];
        let operation = decode_operation(header[6], payload)?;
        records.push(LogRecord {
            lsn,
            transaction_id,
            operation,
        });
        position += total;
    }
    Ok((records, position))
}

fn decode_operation(tag: u8, payload: &[u8]) -> Result<Operation> {
    match tag {
        0 | 2 | 3 | 4 if !payload.is_empty() => corruption("marker WAL record has payload"),
        0 => Ok(Operation::Begin),
        1 => {
            if payload.len() != 8 + PAGE_SIZE {
                return corruption("page-write WAL payload has invalid length");
            }
            let page_id = PageId::new(u64::from_be_bytes(
                payload[..8].try_into().map_err(fixed_error)?,
            ));
            let page = Page::from_bytes(&payload[8..])?;
            if page.id() != page_id {
                return corruption("WAL page id does not match after-image");
            }
            Ok(Operation::PageWrite {
                page_id,
                page: Box::new(page),
            })
        }
        2 => Ok(Operation::Commit),
        3 => Ok(Operation::Abort),
        4 => Ok(Operation::Checkpoint),
        other => corruption(&format!("unknown WAL operation tag {other}")),
    }
}

fn crc32(bytes: &[u8], zeroed: std::ops::Range<usize>) -> u32 {
    let mut crc = u32::MAX;
    for (index, byte) in bytes.iter().copied().enumerate() {
        crc ^= u32::from(if zeroed.contains(&index) { 0 } else { byte });
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn corruption<T>(message: &str) -> Result<T> {
    Err(NovaError::Corruption(message.to_owned()))
}

fn fixed_error<T>(_: T) -> NovaError {
    NovaError::Internal("fixed-width WAL decode failed".to_owned())
}

fn size_error(error: impl std::fmt::Display) -> NovaError {
    NovaError::Storage(format!("WAL size is not representable: {error}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use nova_storage::SlotId;

    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn path(label: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "novadb-wal-{label}-{}-{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            extension
        ))
    }

    fn page(id: u64, value: &[u8]) -> Page {
        let mut page = Page::new(PageId::new(id));
        page.insert(value).unwrap();
        page
    }

    #[test]
    fn append_flush_and_reopen_preserve_records_and_lsns() {
        let path = path("append", "wal");
        let mut wal = WriteAheadLog::open(&path).unwrap();
        assert_eq!(wal.begin(7).unwrap(), 0);
        assert_eq!(wal.log_page_write(7, &page(0, b"one")).unwrap(), 1);
        assert_eq!(wal.commit(7).unwrap(), 2);
        assert_eq!(wal.durable_lsn(), None);
        wal.flush().unwrap();
        assert_eq!(wal.durable_lsn(), Some(2));
        drop(wal);
        let reopened = WriteAheadLog::open(&path).unwrap();
        assert_eq!(reopened.records().len(), 3);
        assert!(matches!(
            reopened.records()[1].operation,
            Operation::PageWrite { .. }
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn recovery_replays_only_committed_transactions_and_is_idempotent() {
        let wal_path = path("recovery", "wal");
        let page_path = path("recovery", "pages");
        let mut wal = WriteAheadLog::open(&wal_path).unwrap();
        wal.begin(1).unwrap();
        wal.log_page_write(1, &page(0, b"committed")).unwrap();
        wal.commit(1).unwrap();
        wal.begin(2).unwrap();
        wal.log_page_write(2, &page(1, b"uncommitted")).unwrap();
        wal.flush().unwrap();
        assert_eq!(recover(&wal_path, &page_path).unwrap(), 1);
        assert_eq!(recover(&wal_path, &page_path).unwrap(), 1);
        let mut pages = PageManager::open(&page_path).unwrap();
        assert_eq!(pages.page_count(), 1);
        assert_eq!(
            pages.read(PageId::new(0)).unwrap().get(SlotId::new(0)),
            Some(b"committed".as_slice())
        );
        fs::remove_file(wal_path).unwrap();
        fs::remove_file(page_path).unwrap();
    }

    #[test]
    fn torn_tail_is_truncated_but_complete_corruption_is_rejected() {
        let path = path("torn", "wal");
        let mut wal = WriteAheadLog::open(&path).unwrap();
        wal.begin(1).unwrap();
        wal.commit(1).unwrap();
        wal.flush().unwrap();
        drop(wal);
        let valid = fs::read(&path).unwrap();
        fs::write(&path, &valid[..valid.len() - 3]).unwrap();
        let reopened = WriteAheadLog::open(&path).unwrap();
        assert_eq!(reopened.records().len(), 1);
        drop(reopened);
        let mut corrupt = valid;
        corrupt[CHECKSUM_OFFSET] ^= 1;
        fs::write(&path, corrupt).unwrap();
        assert!(matches!(
            WriteAheadLog::open(&path),
            Err(NovaError::Corruption(_))
        ));
        fs::remove_file(path).unwrap();
    }
}
