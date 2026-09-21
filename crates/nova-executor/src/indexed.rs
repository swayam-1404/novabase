use std::path::Path;

use nova_core::error::{NovaError, Result};
use nova_core::{Document, NovaId};
use nova_index::{IndexCatalog, IndexDefinition, IndexKey};

use crate::ExecutionBackend;

/// Adds persistent index lifecycle and maintenance to any execution backend.
#[derive(Debug)]
pub struct IndexedBackend<B> {
    inner: B,
    indexes: IndexCatalog,
}

impl<B> IndexedBackend<B> {
    /// Opens an index catalog around `inner`.
    ///
    /// # Errors
    ///
    /// Returns a typed catalog I/O or corruption error.
    pub fn open(inner: B, index_directory: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            inner,
            indexes: IndexCatalog::open(index_directory)?,
        })
    }

    /// Borrows the persistent index catalog.
    #[must_use]
    pub const fn indexes(&self) -> &IndexCatalog {
        &self.indexes
    }

    /// Borrows the wrapped backend.
    #[must_use]
    pub const fn inner(&self) -> &B {
        &self.inner
    }

    /// Consumes the decorator and returns the wrapped backend.
    #[must_use]
    pub fn into_inner(self) -> B {
        self.inner
    }
}

impl<B: ExecutionBackend> ExecutionBackend for IndexedBackend<B> {
    fn scan(&mut self, collection: &str) -> Result<Vec<Document>> {
        self.inner.scan(collection)
    }

    fn insert(&mut self, collection: &str, document: Document) -> Result<NovaId> {
        self.indexes.validate_document(collection, &document)?;
        let id = self.inner.insert(collection, document.clone())?;
        self.indexes.insert_document(collection, &document)?;
        Ok(id)
    }

    fn replace(&mut self, collection: &str, document: Document) -> Result<()> {
        self.indexes.validate_document(collection, &document)?;
        let old = find_document(&mut self.inner, collection, document.id())?;
        self.inner.replace(collection, document.clone())?;
        self.indexes.replace_document(collection, &old, &document)
    }

    fn delete(&mut self, collection: &str, id: NovaId) -> Result<()> {
        let old = find_document(&mut self.inner, collection, id)?;
        self.inner.delete(collection, id)?;
        self.indexes.delete_document(collection, &old)
    }

    fn create_collection(&mut self, name: &str) -> Result<()> {
        self.inner.create_collection(name)
    }

    fn drop_collection(&mut self, name: &str) -> Result<()> {
        self.inner.drop_collection(name)?;
        self.indexes.drop_collection_indexes(name)
    }

    fn create_index(&mut self, name: &str, collection: &str, field: &[String]) -> Result<()> {
        let documents = self.inner.scan(collection)?;
        self.indexes
            .create_index(name, collection, field.to_vec(), &documents)
    }

    fn drop_index(&mut self, name: &str) -> Result<()> {
        self.indexes.drop_index(name)
    }

    fn index_definitions(&self) -> Vec<IndexDefinition> {
        self.indexes.definitions()
    }

    fn scan_index(
        &mut self,
        collection: &str,
        index: &str,
        key: &IndexKey,
    ) -> Result<Vec<Document>> {
        let ids: std::collections::BTreeSet<NovaId> =
            self.indexes.lookup(index, key)?.into_iter().collect();
        Ok(self
            .inner
            .scan(collection)?
            .into_iter()
            .filter(|document| ids.contains(&document.id()))
            .collect())
    }
}

fn find_document(
    backend: &mut impl ExecutionBackend,
    collection: &str,
    id: NovaId,
) -> Result<Document> {
    backend
        .scan(collection)?
        .into_iter()
        .find(|document| document.id() == id)
        .ok_or_else(|| NovaError::NotFound(format!("document {id}")))
}
