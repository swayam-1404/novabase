use std::collections::BTreeMap;
use std::path::Path;

use nova_core::document::Document;
use nova_core::error::{NovaError, Result};
use nova_core::nova_id::NovaId;

use crate::{Page, PageId, PageManager, SlotId};

#[derive(Debug, Clone, Copy)]
struct Location {
    page: PageId,
    slot: SlotId,
}

/// Persistent document engine backed by NBF records in slotted pages.
///
/// Phase 4 rebuilds its in-memory id-to-slot directory by scanning validated
/// pages during [`StorageEngine::open`]. Persistent B+ tree indexing is added in
/// later roadmap phases.
#[derive(Debug)]
pub struct StorageEngine {
    pages: PageManager,
    locations: BTreeMap<NovaId, Location>,
}

impl StorageEngine {
    /// Opens or creates a document store and rebuilds its location directory.
    ///
    /// # Errors
    ///
    /// Returns a typed I/O, page-corruption, or NBF-corruption error if the page
    /// file cannot be read completely and consistently.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut pages = PageManager::open(path)?;
        let mut locations = BTreeMap::new();
        for raw_id in 0..pages.page_count() {
            let page_id = PageId::new(raw_id);
            let page = pages.read(page_id)?;
            for (slot, bytes) in page.records() {
                let document = nova_nbf::decode(bytes)?;
                let previous = locations.insert(
                    document.id(),
                    Location {
                        page: page_id,
                        slot,
                    },
                );
                if previous.is_some() {
                    return Err(NovaError::Corruption(format!(
                        "duplicate document id {} in page file",
                        document.id()
                    )));
                }
            }
        }
        Ok(Self { pages, locations })
    }

    /// Returns the number of stored documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.locations.len()
    }

    /// Reports whether the store contains no documents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.locations.is_empty()
    }

    /// Inserts one document and returns its id.
    ///
    /// The write reaches the operating-system file cache immediately. Call
    /// [`StorageEngine::shutdown`] to explicitly synchronize it to the storage
    /// device before closing the engine.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::AlreadyExists`] for a duplicate id,
    /// [`NovaError::Storage`] when the encoded document cannot fit in an empty
    /// page, or a typed codec/page/I/O error.
    pub fn insert(&mut self, document: &Document) -> Result<NovaId> {
        let id = document.id();
        if self.locations.contains_key(&id) {
            return Err(NovaError::AlreadyExists(format!("document {id}")));
        }
        let encoded = nova_nbf::encode(document)?;
        let empty_page_capacity = Page::new(PageId::new(0)).free_space();
        if encoded.len() > empty_page_capacity {
            return Err(NovaError::Storage(format!(
                "encoded document {id} is {} bytes; page record capacity is {empty_page_capacity}",
                encoded.len()
            )));
        }

        for raw_id in 0..self.pages.page_count() {
            let page_id = PageId::new(raw_id);
            let mut page = self.pages.read(page_id)?;
            if encoded.len() <= page.free_space() {
                let slot = page.insert(&encoded)?;
                self.pages.write(&page)?;
                self.locations.insert(
                    id,
                    Location {
                        page: page_id,
                        slot,
                    },
                );
                return Ok(id);
            }
        }

        let page_id = self.pages.allocate()?;
        let mut page = self.pages.read(page_id)?;
        let slot = page.insert(&encoded)?;
        self.pages.write(&page)?;
        self.locations.insert(
            id,
            Location {
                page: page_id,
                slot,
            },
        );
        Ok(id)
    }

    /// Reads a document by id.
    ///
    /// # Errors
    ///
    /// Returns a typed page, NBF, or consistency error when persisted data is
    /// malformed. A genuinely missing id returns `Ok(None)`.
    pub fn get(&mut self, id: NovaId) -> Result<Option<Document>> {
        let Some(location) = self.locations.get(&id).copied() else {
            return Ok(None);
        };
        let page = self.pages.read(location.page)?;
        let bytes = page.get(location.slot).ok_or_else(|| {
            NovaError::Corruption(format!(
                "document {id} points to missing page {} slot {}",
                location.page.get(),
                location.slot.get()
            ))
        })?;
        let document = nova_nbf::decode(bytes)?;
        if document.id() != id {
            return Err(NovaError::Corruption(format!(
                "document index id {id} does not match stored id {}",
                document.id()
            )));
        }
        Ok(Some(document))
    }

    /// Synchronizes all page-file contents and consumes the engine.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::Io`] if the operating system cannot synchronize the
    /// page file.
    pub fn shutdown(self) -> Result<()> {
        self.pages.sync()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use nova_core::nova_timestamp::NovaTimestamp;
    use nova_core::nova_value::NovaValue;

    use super::*;

    static NEXT_TEST_FILE: AtomicU64 = AtomicU64::new(0);

    struct TestFile(PathBuf);

    impl TestFile {
        fn new(label: &str) -> Self {
            let sequence = NEXT_TEST_FILE.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "novadb-engine-{label}-{}-{sequence}.pages",
                std::process::id()
            )))
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn document(seed: u8, payload_size: usize) -> Document {
        let id = NovaId::new(u64::from(seed), [seed; 8]);
        let mut document = Document::new(id);
        document.insert("name", format!("document-{seed}").into());
        document.insert("sequence", i64::from(seed).into());
        document.insert(
            "created",
            NovaTimestamp::from_millis(-i64::from(seed)).into(),
        );
        document.insert("payload", "x".repeat(payload_size).into());
        document.insert(
            "values",
            NovaValue::Array(vec![NovaValue::Null, true.into(), (-7_i64).into()]),
        );
        document
    }

    #[test]
    fn insert_shutdown_restart_and_read() {
        let test_file = TestFile::new("restart");
        let expected = document(1, 100);
        {
            let mut engine = StorageEngine::open(&test_file.0).unwrap();
            assert!(engine.is_empty());
            assert_eq!(engine.insert(&expected).unwrap(), expected.id());
            assert_eq!(engine.get(expected.id()).unwrap(), Some(expected.clone()));
            engine.shutdown().unwrap();
        }

        let mut reopened = StorageEngine::open(&test_file.0).unwrap();
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened.get(expected.id()).unwrap(), Some(expected));
        reopened.shutdown().unwrap();
    }

    #[test]
    fn multiple_pages_survive_restart() {
        let test_file = TestFile::new("multipage");
        let expected: Vec<Document> = (1..=8).map(|seed| document(seed, 1_500)).collect();
        let mut engine = StorageEngine::open(&test_file.0).unwrap();
        for document in &expected {
            engine.insert(document).unwrap();
        }
        assert_eq!(engine.len(), expected.len());
        engine.shutdown().unwrap();

        let mut reopened = StorageEngine::open(&test_file.0).unwrap();
        for document in expected {
            assert_eq!(reopened.get(document.id()).unwrap(), Some(document));
        }
        reopened.shutdown().unwrap();
    }

    #[test]
    fn duplicate_ids_are_rejected_without_overwriting() {
        let test_file = TestFile::new("duplicate");
        let original = document(3, 10);
        let mut replacement = Document::new(original.id());
        replacement.insert("different", true.into());

        let mut engine = StorageEngine::open(&test_file.0).unwrap();
        engine.insert(&original).unwrap();
        assert!(matches!(
            engine.insert(&replacement),
            Err(NovaError::AlreadyExists(_))
        ));
        assert_eq!(engine.get(original.id()).unwrap(), Some(original));
    }

    #[test]
    fn missing_ids_return_none_and_oversized_documents_fail_cleanly() {
        let test_file = TestFile::new("limits");
        let mut engine = StorageEngine::open(&test_file.0).unwrap();
        assert_eq!(engine.get(NovaId::new(99, [0; 8])).unwrap(), None);

        let oversized = document(5, crate::PAGE_SIZE);
        assert!(matches!(
            engine.insert(&oversized),
            Err(NovaError::Storage(_))
        ));
        assert!(engine.is_empty());
    }
}
