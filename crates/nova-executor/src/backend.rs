use std::collections::BTreeMap;

use nova_core::document::Document;
use nova_core::error::{NovaError, Result};
use nova_core::nova_id::NovaId;

/// Storage operations required by the Phase 7 query executor.
///
/// Implementations decide how collections are persisted. Scans must return a
/// deterministic order; later planner/index phases can supply alternative
/// access paths without changing expression or pipeline semantics.
pub trait ExecutionBackend {
    /// Returns every document in a collection in deterministic scan order.
    ///
    /// # Errors
    ///
    /// Returns a typed backend error when the collection is unavailable.
    fn scan(&mut self, collection: &str) -> Result<Vec<Document>>;

    /// Inserts one document.
    ///
    /// # Errors
    ///
    /// Returns a typed backend error for a missing collection, duplicate id,
    /// or persistence failure.
    fn insert(&mut self, collection: &str, document: Document) -> Result<NovaId>;

    /// Replaces an existing document with the same id.
    ///
    /// # Errors
    ///
    /// Returns a typed backend error when the collection/document is missing
    /// or persistence fails.
    fn replace(&mut self, collection: &str, document: Document) -> Result<()>;

    /// Deletes one existing document.
    ///
    /// # Errors
    ///
    /// Returns a typed backend error when the collection/document is missing
    /// or persistence fails.
    fn delete(&mut self, collection: &str, id: NovaId) -> Result<()>;

    /// Creates an empty collection.
    ///
    /// # Errors
    ///
    /// Returns a typed backend error if the collection exists or creation
    /// fails.
    fn create_collection(&mut self, name: &str) -> Result<()>;

    /// Drops a collection and all of its documents.
    ///
    /// # Errors
    ///
    /// Returns a typed backend error if the collection is missing or removal
    /// fails.
    fn drop_collection(&mut self, name: &str) -> Result<()>;

    /// Creates and backfills a single-field index.
    ///
    /// # Errors
    ///
    /// The default returns [`NovaError::Unsupported`]. Index-capable backends
    /// return typed catalog, value, or persistence errors.
    fn create_index(&mut self, _name: &str, _collection: &str, _field: &[String]) -> Result<()> {
        Err(NovaError::Unsupported(
            "backend does not provide persistent indexes".to_owned(),
        ))
    }

    /// Drops a named index.
    ///
    /// # Errors
    ///
    /// The default returns [`NovaError::Unsupported`].
    fn drop_index(&mut self, _name: &str) -> Result<()> {
        Err(NovaError::Unsupported(
            "backend does not provide persistent indexes".to_owned(),
        ))
    }
}

/// Deterministic in-memory backend for embedding and executor verification.
#[derive(Debug, Default)]
pub struct MemoryBackend {
    collections: BTreeMap<String, BTreeMap<NovaId, Document>>,
}

impl MemoryBackend {
    /// Creates an empty backend.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            collections: BTreeMap::new(),
        }
    }

    /// Returns the number of collections.
    #[must_use]
    pub fn collection_count(&self) -> usize {
        self.collections.len()
    }

    /// Returns the number of documents in a collection, or `None` if absent.
    #[must_use]
    pub fn collection_len(&self, name: &str) -> Option<usize> {
        self.collections.get(name).map(BTreeMap::len)
    }

    fn collection_mut(&mut self, name: &str) -> Result<&mut BTreeMap<NovaId, Document>> {
        self.collections
            .get_mut(name)
            .ok_or_else(|| NovaError::NotFound(format!("collection {name:?}")))
    }
}

impl ExecutionBackend for MemoryBackend {
    fn scan(&mut self, collection: &str) -> Result<Vec<Document>> {
        let documents = self
            .collections
            .get(collection)
            .ok_or_else(|| NovaError::NotFound(format!("collection {collection:?}")))?;
        Ok(documents.values().cloned().collect())
    }

    fn insert(&mut self, collection: &str, document: Document) -> Result<NovaId> {
        let documents = self.collection_mut(collection)?;
        let id = document.id();
        if documents.contains_key(&id) {
            return Err(NovaError::AlreadyExists(format!("document {id}")));
        }
        documents.insert(id, document);
        Ok(id)
    }

    fn replace(&mut self, collection: &str, document: Document) -> Result<()> {
        let documents = self.collection_mut(collection)?;
        let id = document.id();
        if !documents.contains_key(&id) {
            return Err(NovaError::NotFound(format!("document {id}")));
        }
        documents.insert(id, document);
        Ok(())
    }

    fn delete(&mut self, collection: &str, id: NovaId) -> Result<()> {
        if self.collection_mut(collection)?.remove(&id).is_none() {
            return Err(NovaError::NotFound(format!("document {id}")));
        }
        Ok(())
    }

    fn create_collection(&mut self, name: &str) -> Result<()> {
        if self.collections.contains_key(name) {
            return Err(NovaError::AlreadyExists(format!("collection {name:?}")));
        }
        self.collections.insert(name.to_owned(), BTreeMap::new());
        Ok(())
    }

    fn drop_collection(&mut self, name: &str) -> Result<()> {
        if self.collections.remove(name).is_none() {
            return Err(NovaError::NotFound(format!("collection {name:?}")));
        }
        Ok(())
    }
}
