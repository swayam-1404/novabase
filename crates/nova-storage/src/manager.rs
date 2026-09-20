use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use nova_core::error::{NovaError, Result};

use crate::page::{Page, PageId, PAGE_SIZE};

/// Direct, fixed-size page I/O over one database page file.
///
/// `PageManager` deliberately performs no caching; the buffer pool introduced
/// in a later phase will layer caching and eviction over this interface.
#[derive(Debug)]
pub struct PageManager {
    path: PathBuf,
    file: File,
    page_count: u64,
}

impl PageManager {
    /// Opens or creates a page file and validates its page alignment.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::Io`] for filesystem failures or
    /// [`NovaError::Corruption`] when the existing file length is not an exact
    /// multiple of the fixed page size.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let length = file.metadata()?.len();
        let page_size = PAGE_SIZE as u64;
        if length % page_size != 0 {
            return Err(NovaError::Corruption(format!(
                "page file length {length} is not a multiple of {PAGE_SIZE}"
            )));
        }
        Ok(Self {
            path,
            file,
            page_count: length / page_size,
        })
    }

    /// Returns the path backing this manager.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the number of allocated pages.
    #[must_use]
    pub const fn page_count(&self) -> u64 {
        self.page_count
    }

    /// Appends a new empty page, returning its identifier.
    ///
    /// # Errors
    ///
    /// Returns a typed error if the page cannot be encoded or written.
    pub fn allocate(&mut self) -> Result<PageId> {
        let id = PageId::new(self.page_count);
        let page = Page::new(id);
        let bytes = page.to_bytes()?;
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&bytes)?;
        self.page_count = self
            .page_count
            .checked_add(1)
            .ok_or_else(|| NovaError::Storage("page count overflow".to_owned()))?;
        Ok(id)
    }

    /// Reads and fully validates one allocated page.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::NotFound`] for an unallocated id, an I/O error for
    /// a failed read, or a typed page decoding error for corrupt bytes.
    pub fn read(&mut self, id: PageId) -> Result<Page> {
        self.ensure_allocated(id)?;
        let mut bytes = [0_u8; PAGE_SIZE];
        self.file.seek(SeekFrom::Start(page_offset(id)?))?;
        self.file.read_exact(&mut bytes)?;
        let page = Page::from_bytes(&bytes)?;
        if page.id() != id {
            return Err(NovaError::Corruption(format!(
                "page {} contains header for page {}",
                id.get(),
                page.id().get()
            )));
        }
        Ok(page)
    }

    /// Writes a page back to its already allocated location.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::NotFound`] when the page id has not been allocated,
    /// or a typed encoding/I/O error if the write fails.
    pub fn write(&mut self, page: &Page) -> Result<()> {
        self.ensure_allocated(page.id())?;
        let bytes = page.to_bytes()?;
        self.file.seek(SeekFrom::Start(page_offset(page.id())?))?;
        self.file.write_all(&bytes)?;
        Ok(())
    }

    /// Flushes file contents and metadata to the storage device.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::Io`] if the operating system cannot synchronize the
    /// file.
    pub fn sync(&self) -> Result<()> {
        self.file.sync_all()?;
        Ok(())
    }

    fn ensure_allocated(&self, id: PageId) -> Result<()> {
        if id.get() >= self.page_count {
            return Err(NovaError::NotFound(format!("page {}", id.get())));
        }
        Ok(())
    }
}

fn page_offset(id: PageId) -> Result<u64> {
    id.get()
        .checked_mul(PAGE_SIZE as u64)
        .ok_or_else(|| NovaError::Storage("page file offset overflow".to_owned()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::SlotId;

    static NEXT_TEST_FILE: AtomicU64 = AtomicU64::new(0);

    struct TestFile(PathBuf);

    impl TestFile {
        fn new(label: &str) -> Self {
            let sequence = NEXT_TEST_FILE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "novadb-{label}-{}-{sequence}.pages",
                std::process::id()
            ));
            Self(path)
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn allocate_write_reopen_and_read_pages() {
        let test_file = TestFile::new("roundtrip");
        {
            let mut manager = PageManager::open(&test_file.0).unwrap();
            assert_eq!(manager.page_count(), 0);
            let first = manager.allocate().unwrap();
            let second = manager.allocate().unwrap();
            assert_eq!(first, PageId::new(0));
            assert_eq!(second, PageId::new(1));

            let mut page = manager.read(second).unwrap();
            page.insert(b"persistent bytes").unwrap();
            manager.write(&page).unwrap();
            manager.sync().unwrap();
        }

        let mut reopened = PageManager::open(&test_file.0).unwrap();
        assert_eq!(reopened.page_count(), 2);
        let page = reopened.read(PageId::new(1)).unwrap();
        assert_eq!(
            page.get(SlotId::new(0)),
            Some(b"persistent bytes".as_slice())
        );
    }

    #[test]
    fn rejects_unallocated_pages_and_misaligned_files() {
        let test_file = TestFile::new("invalid");
        let mut manager = PageManager::open(&test_file.0).unwrap();
        assert!(matches!(
            manager.read(PageId::new(0)),
            Err(NovaError::NotFound(_))
        ));
        drop(manager);

        fs::write(&test_file.0, [0_u8; 17]).unwrap();
        assert!(matches!(
            PageManager::open(&test_file.0),
            Err(NovaError::Corruption(_))
        ));
    }

    #[test]
    fn detects_checksum_corruption_on_read() {
        let test_file = TestFile::new("corrupt");
        let mut manager = PageManager::open(&test_file.0).unwrap();
        manager.allocate().unwrap();
        drop(manager);

        let mut bytes = fs::read(&test_file.0).unwrap();
        bytes[100] ^= 1;
        fs::write(&test_file.0, bytes).unwrap();

        let mut manager = PageManager::open(&test_file.0).unwrap();
        assert!(matches!(
            manager.read(PageId::new(0)),
            Err(NovaError::Corruption(_))
        ));
    }
}
