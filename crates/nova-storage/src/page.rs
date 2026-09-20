use nova_core::error::{NovaError, Result};

use crate::checksum::crc32;

/// Fixed on-disk page size in bytes.
pub const PAGE_SIZE: usize = 4096;

/// Page format magic at the start of every page.
pub const PAGE_MAGIC: [u8; 4] = *b"NVPG";

/// Current slotted-page format version.
pub const PAGE_FORMAT_VERSION: u16 = 1;

/// Fixed header size in bytes.
pub const PAGE_HEADER_SIZE: usize = 32;

const SLOT_SIZE: usize = 4;
const CHECKSUM_RANGE: std::ops::Range<usize> = 16..20;
const DELETED_OFFSET: u16 = u16::MAX;

/// Stable zero-based identifier of a page in a page file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageId(u64);

impl PageId {
    /// Creates a page identifier from its raw numeric value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw numeric page identifier.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Stable zero-based identifier of a record slot within a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotId(u16);

impl SlotId {
    /// Creates a slot identifier from its raw numeric value.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Returns the raw numeric slot identifier.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// A slotted data page containing opaque record byte strings.
///
/// Slot identifiers stay stable when records are removed. Serialization
/// compacts live payloads at the end of the page while the slot directory grows
/// forward from the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    id: PageId,
    slots: Vec<Option<Vec<u8>>>,
}

impl Page {
    /// Creates an empty page.
    #[must_use]
    pub const fn new(id: PageId) -> Self {
        Self {
            id,
            slots: Vec::new(),
        }
    }

    /// Returns this page's file-relative identifier.
    #[must_use]
    pub const fn id(&self) -> PageId {
        self.id
    }

    /// Returns the number of live records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// Reports whether the page contains no live records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the total number of stable slots, including deleted slots.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Returns the bytes stored in `slot`, or `None` for a missing/deleted slot.
    #[must_use]
    pub fn get(&self, slot: SlotId) -> Option<&[u8]> {
        self.slots
            .get(usize::from(slot.get()))
            .and_then(Option::as_deref)
    }

    /// Iterates live records in ascending stable slot order.
    pub fn records(&self) -> impl Iterator<Item = (SlotId, &[u8])> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            let id = u16::try_from(index).ok().map(SlotId::new)?;
            slot.as_deref().map(|record| (id, record))
        })
    }

    /// Returns the number of bytes currently available for a new record.
    ///
    /// A deleted slot is reused first and therefore does not consume another
    /// slot-directory entry.
    #[must_use]
    pub fn free_space(&self) -> usize {
        let directory_size = self.slots.len() * SLOT_SIZE;
        let payload_size: usize = self.slots.iter().flatten().map(Vec::len).sum();
        let reusable_slot = self.slots.iter().any(Option::is_none);
        PAGE_SIZE
            .saturating_sub(PAGE_HEADER_SIZE + directory_size + payload_size)
            .saturating_sub(if reusable_slot { 0 } else { SLOT_SIZE })
    }

    /// Inserts an opaque record and returns its stable slot identifier.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::Storage`] if the record does not fit or the slot
    /// identifier space is exhausted.
    pub fn insert(&mut self, record: &[u8]) -> Result<SlotId> {
        if record.len() > self.free_space() {
            return Err(NovaError::Storage(format!(
                "record of {} bytes does not fit in page {} ({} bytes free)",
                record.len(),
                self.id.get(),
                self.free_space()
            )));
        }

        if let Some(index) = self.slots.iter().position(Option::is_none) {
            self.slots[index] = Some(record.to_vec());
            return slot_id(index);
        }

        let id = slot_id(self.slots.len())?;
        self.slots.push(Some(record.to_vec()));
        Ok(id)
    }

    /// Removes and returns a record while preserving its slot for reuse.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::NotFound`] if the slot is outside the directory or
    /// already deleted.
    pub fn remove(&mut self, slot: SlotId) -> Result<Vec<u8>> {
        self.slots
            .get_mut(usize::from(slot.get()))
            .and_then(Option::take)
            .ok_or_else(|| NovaError::NotFound(format!("page slot {}", slot.get())))
    }

    /// Serializes the page into its checksummed fixed-size representation.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::Storage`] if internal contents cannot be represented
    /// by the page format.
    pub fn to_bytes(&self) -> Result<[u8; PAGE_SIZE]> {
        let slot_count = u16::try_from(self.slots.len())
            .map_err(|_| NovaError::Storage("page has too many slots".to_owned()))?;
        let live_count = u16::try_from(self.len())
            .map_err(|_| NovaError::Storage("page has too many live records".to_owned()))?;
        let free_start = PAGE_HEADER_SIZE
            .checked_add(self.slots.len() * SLOT_SIZE)
            .ok_or_else(|| NovaError::Storage("page directory size overflow".to_owned()))?;
        if free_start > PAGE_SIZE {
            return Err(NovaError::Storage(
                "page directory exceeds page size".to_owned(),
            ));
        }

        let mut bytes = [0_u8; PAGE_SIZE];
        let mut payload_start = PAGE_SIZE;
        for (index, slot) in self.slots.iter().enumerate() {
            let directory_offset = PAGE_HEADER_SIZE + index * SLOT_SIZE;
            if let Some(record) = slot {
                payload_start = payload_start
                    .checked_sub(record.len())
                    .filter(|start| *start >= free_start)
                    .ok_or_else(|| {
                        NovaError::Storage("page payload exceeds page size".to_owned())
                    })?;
                bytes[payload_start..payload_start + record.len()].copy_from_slice(record);
                put_u16(
                    &mut bytes[directory_offset..directory_offset + 2],
                    as_u16(payload_start)?,
                );
                put_u16(
                    &mut bytes[directory_offset + 2..directory_offset + SLOT_SIZE],
                    as_u16(record.len())?,
                );
            } else {
                put_u16(
                    &mut bytes[directory_offset..directory_offset + 2],
                    DELETED_OFFSET,
                );
            }
        }

        bytes[..4].copy_from_slice(&PAGE_MAGIC);
        put_u16(&mut bytes[4..6], PAGE_FORMAT_VERSION);
        put_u16(&mut bytes[6..8], as_u16(PAGE_HEADER_SIZE)?);
        put_u64(&mut bytes[8..16], self.id.get());
        put_u16(&mut bytes[20..22], slot_count);
        put_u16(&mut bytes[22..24], live_count);
        put_u16(&mut bytes[24..26], as_u16(free_start)?);
        put_u16(&mut bytes[26..28], as_u16(payload_start)?);
        let checksum = crc32(&bytes, CHECKSUM_RANGE);
        put_u32(&mut bytes[CHECKSUM_RANGE], checksum);
        Ok(bytes)
    }

    /// Decodes and validates one fixed-size page.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::Unsupported`] for an unknown page version and
    /// [`NovaError::Corruption`] for invalid magic, checksum, header, slot, or
    /// payload metadata.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != PAGE_SIZE {
            return Err(corruption(format!(
                "page must be exactly {PAGE_SIZE} bytes; got {}",
                bytes.len()
            )));
        }
        if bytes[..4] != PAGE_MAGIC {
            return Err(corruption("bad page magic"));
        }
        let version = read_u16(&bytes[4..6]);
        if version != PAGE_FORMAT_VERSION {
            return Err(NovaError::Unsupported(format!(
                "unsupported page format version {version}"
            )));
        }
        if usize::from(read_u16(&bytes[6..8])) != PAGE_HEADER_SIZE {
            return Err(corruption("invalid page header size"));
        }
        let expected_checksum = read_u32(&bytes[CHECKSUM_RANGE]);
        if crc32(bytes, CHECKSUM_RANGE) != expected_checksum {
            return Err(corruption("page checksum mismatch"));
        }
        if bytes[28..32] != [0; 4] {
            return Err(corruption("non-zero reserved page header bytes"));
        }

        let id = PageId::new(read_u64(&bytes[8..16]));
        let slot_count = usize::from(read_u16(&bytes[20..22]));
        let live_count = usize::from(read_u16(&bytes[22..24]));
        let free_start = usize::from(read_u16(&bytes[24..26]));
        let free_end = usize::from(read_u16(&bytes[26..28]));
        let expected_free_start = PAGE_HEADER_SIZE
            .checked_add(slot_count * SLOT_SIZE)
            .ok_or_else(|| corruption("slot directory size overflow"))?;
        if free_start != expected_free_start || free_start > free_end || free_end > PAGE_SIZE {
            return Err(corruption("invalid page free-space boundaries"));
        }

        let mut ranges = Vec::with_capacity(live_count);
        let mut slots = Vec::with_capacity(slot_count);
        for index in 0..slot_count {
            let offset = PAGE_HEADER_SIZE + index * SLOT_SIZE;
            let record_offset = read_u16(&bytes[offset..offset + 2]);
            let record_length = usize::from(read_u16(&bytes[offset + 2..offset + SLOT_SIZE]));
            if record_offset == DELETED_OFFSET {
                if record_length != 0 {
                    return Err(corruption("deleted slot has a non-zero length"));
                }
                slots.push(None);
                continue;
            }

            let start = usize::from(record_offset);
            let end = start
                .checked_add(record_length)
                .filter(|end| start >= free_end && *end <= PAGE_SIZE)
                .ok_or_else(|| corruption("slot payload lies outside payload area"))?;
            if ranges
                .iter()
                .any(|&(other_start, other_end)| start < other_end && other_start < end)
            {
                return Err(corruption("slot payloads overlap"));
            }
            ranges.push((start, end));
            slots.push(Some(bytes[start..end].to_vec()));
        }
        if slots.iter().filter(|slot| slot.is_some()).count() != live_count {
            return Err(corruption("live slot count does not match header"));
        }
        if ranges
            .iter()
            .map(|(start, _)| *start)
            .min()
            .unwrap_or(PAGE_SIZE)
            != free_end
        {
            return Err(corruption("payload boundary does not match live slots"));
        }
        let payload_bytes: usize = ranges.iter().map(|(start, end)| end - start).sum();
        if PAGE_SIZE - free_end != payload_bytes {
            return Err(corruption("payload area contains gaps"));
        }

        Ok(Self { id, slots })
    }
}

fn slot_id(index: usize) -> Result<SlotId> {
    u16::try_from(index)
        .map(SlotId::new)
        .map_err(|_| NovaError::Storage("page slot identifier space exhausted".to_owned()))
}

fn as_u16(value: usize) -> Result<u16> {
    u16::try_from(value).map_err(|_| NovaError::Storage("page offset exceeds u16".to_owned()))
}

fn corruption(message: impl Into<String>) -> NovaError {
    NovaError::Corruption(message.into())
}

fn put_u16(output: &mut [u8], value: u16) {
    output.copy_from_slice(&value.to_be_bytes());
}

fn put_u32(output: &mut [u8], value: u32) {
    output.copy_from_slice(&value.to_be_bytes());
}

fn put_u64(output: &mut [u8], value: u64) {
    output.copy_from_slice(&value.to_be_bytes());
}

fn read_u16(input: &[u8]) -> u16 {
    u16::from_be_bytes([input[0], input[1]])
}

fn read_u32(input: &[u8]) -> u32 {
    u32::from_be_bytes([input[0], input[1], input[2], input[3]])
}

fn read_u64(input: &[u8]) -> u64 {
    u64::from_be_bytes([
        input[0], input[1], input[2], input[3], input[4], input[5], input[6], input[7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_page_round_trips() {
        let page = Page::new(PageId::new(7));
        let decoded = Page::from_bytes(&page.to_bytes().unwrap()).unwrap();
        assert_eq!(decoded, page);
        assert!(decoded.is_empty());
    }

    #[test]
    fn records_round_trip_and_deleted_slot_is_reused() {
        let mut page = Page::new(PageId::new(3));
        let first = page.insert(b"first").unwrap();
        let second = page.insert(b"").unwrap();
        let third = page.insert(b"third record").unwrap();
        assert_eq!(page.remove(second).unwrap(), b"");
        let replacement = page.insert(b"replacement").unwrap();
        assert_eq!(replacement, second);

        let decoded = Page::from_bytes(&page.to_bytes().unwrap()).unwrap();
        assert_eq!(decoded.get(first), Some(b"first".as_slice()));
        assert_eq!(decoded.get(replacement), Some(b"replacement".as_slice()));
        assert_eq!(decoded.get(third), Some(b"third record".as_slice()));
        assert_eq!(decoded.len(), 3);
    }

    #[test]
    fn removed_slots_survive_serialization() {
        let mut page = Page::new(PageId::new(1));
        let removed = page.insert(b"gone").unwrap();
        let kept = page.insert(b"kept").unwrap();
        page.remove(removed).unwrap();
        let decoded = Page::from_bytes(&page.to_bytes().unwrap()).unwrap();
        assert_eq!(decoded.get(removed), None);
        assert_eq!(decoded.get(kept), Some(b"kept".as_slice()));
        assert_eq!(decoded.slot_count(), 2);
    }

    #[test]
    fn insertion_fails_cleanly_when_page_is_full() {
        let mut page = Page::new(PageId::new(1));
        let capacity = page.free_space();
        page.insert(&vec![0xaa; capacity]).unwrap();
        assert_eq!(page.free_space(), 0);
        assert!(matches!(page.insert(b"x"), Err(NovaError::Storage(_))));
        assert!(Page::from_bytes(&page.to_bytes().unwrap()).is_ok());
    }

    #[test]
    fn corruption_and_unknown_versions_are_typed_errors() {
        let page = Page::new(PageId::new(1));
        let valid = page.to_bytes().unwrap();

        let mut corrupt = valid;
        corrupt[100] ^= 1;
        assert!(matches!(
            Page::from_bytes(&corrupt),
            Err(NovaError::Corruption(_))
        ));

        let mut unsupported = page.to_bytes().unwrap();
        put_u16(&mut unsupported[4..6], PAGE_FORMAT_VERSION + 1);
        assert!(matches!(
            Page::from_bytes(&unsupported),
            Err(NovaError::Unsupported(_))
        ));
    }

    #[test]
    fn malformed_page_never_panics_at_any_input_length() {
        let bytes = Page::new(PageId::new(9)).to_bytes().unwrap();
        for end in 0..PAGE_SIZE {
            let result = std::panic::catch_unwind(|| Page::from_bytes(&bytes[..end]));
            assert!(result.is_ok(), "page decoder panicked at length {end}");
            assert!(result.unwrap().is_err());
        }
    }
}
