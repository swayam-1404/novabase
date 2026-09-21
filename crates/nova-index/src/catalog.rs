use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use nova_core::error::{NovaError, Result};
use nova_core::{Document, NovaId, NovaValue};

use crate::{BPlusTree, IndexKey};

const META_MAGIC: &[u8; 4] = b"NVIM";
const META_VERSION: u16 = 1;

/// Persistent metadata describing one named index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDefinition {
    /// Database-wide index name.
    pub name: String,
    /// Collection whose documents are indexed.
    pub collection: String,
    /// One nested document path, represented as path segments.
    pub field: Vec<String>,
}

struct OpenIndex {
    definition: IndexDefinition,
    tree: BPlusTree,
}

/// Directory-backed collection of named persistent indexes.
pub struct IndexCatalog {
    directory: PathBuf,
    indexes: BTreeMap<String, OpenIndex>,
}

impl std::fmt::Debug for IndexCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IndexCatalog")
            .field("directory", &self.directory)
            .field("index_names", &self.indexes.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl IndexCatalog {
    /// Opens or creates an index directory and validates every index in it.
    ///
    /// # Errors
    ///
    /// Returns a typed I/O, metadata-corruption, or tree-corruption error.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir_all(&directory)?;
        let mut meta_paths = Vec::new();
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "meta")
            {
                meta_paths.push(path);
            }
        }
        meta_paths.sort();
        let mut indexes = BTreeMap::new();
        for meta_path in meta_paths {
            let definition = read_definition(&meta_path)?;
            validate_identifier(&definition.name, "index")?;
            validate_identifier(&definition.collection, "collection")?;
            validate_field(&definition.field)?;
            let tree = BPlusTree::open(tree_path(&directory, &definition.name))?;
            if indexes
                .insert(definition.name.clone(), OpenIndex { definition, tree })
                .is_some()
            {
                return Err(NovaError::Corruption(
                    "duplicate index definition".to_owned(),
                ));
            }
        }
        Ok(Self { directory, indexes })
    }

    /// Returns all definitions in deterministic name order.
    #[must_use]
    pub fn definitions(&self) -> Vec<IndexDefinition> {
        self.indexes
            .values()
            .map(|index| index.definition.clone())
            .collect()
    }

    /// Creates and backfills a named single-field index.
    ///
    /// # Errors
    ///
    /// Returns a typed error for invalid/duplicate metadata, an unindexable
    /// field value, or a persistence failure.
    pub fn create_index(
        &mut self,
        name: &str,
        collection: &str,
        field: Vec<String>,
        documents: &[Document],
    ) -> Result<()> {
        validate_identifier(name, "index")?;
        validate_identifier(collection, "collection")?;
        validate_field(&field)?;
        if self.indexes.contains_key(name) {
            return Err(NovaError::AlreadyExists(format!("index {name:?}")));
        }
        let definition = IndexDefinition {
            name: name.to_owned(),
            collection: collection.to_owned(),
            field,
        };
        let keys = documents
            .iter()
            .map(|document| Ok((document_key(&definition, document)?, document.id())))
            .collect::<Result<Vec<_>>>()?;
        let tree_file = tree_path(&self.directory, name);
        let meta_file = meta_path(&self.directory, name);
        let mut tree = BPlusTree::create(&tree_file)?;
        let result = (|| {
            for (key, id) in keys {
                if let Some(key) = key {
                    tree.insert(key, id)?;
                }
            }
            tree.sync()?;
            write_definition(&meta_file, &definition)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&tree_file);
            let _ = fs::remove_file(&meta_file);
            return Err(error);
        }
        self.indexes
            .insert(name.to_owned(), OpenIndex { definition, tree });
        Ok(())
    }

    /// Drops a named index and its persistent files.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::NotFound`] for an unknown index or an I/O error if
    /// either file cannot be removed.
    pub fn drop_index(&mut self, name: &str) -> Result<()> {
        if !self.indexes.contains_key(name) {
            return Err(NovaError::NotFound(format!("index {name:?}")));
        }
        fs::remove_file(meta_path(&self.directory, name))?;
        fs::remove_file(tree_path(&self.directory, name))?;
        self.indexes.remove(name);
        Ok(())
    }

    /// Drops every index belonging to a collection.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if an index file cannot be removed.
    pub fn drop_collection_indexes(&mut self, collection: &str) -> Result<()> {
        let names: Vec<String> = self
            .indexes
            .values()
            .filter(|index| index.definition.collection == collection)
            .map(|index| index.definition.name.clone())
            .collect();
        for name in names {
            self.drop_index(&name)?;
        }
        Ok(())
    }

    /// Adds a newly inserted document to all applicable indexes.
    ///
    /// # Errors
    ///
    /// Returns a typed unindexable-value or persistence error.
    pub fn insert_document(&mut self, collection: &str, document: &Document) -> Result<()> {
        let changes = self.keys_for_document(collection, document)?;
        for (name, key) in changes {
            if let Some(key) = key {
                self.indexes
                    .get_mut(&name)
                    .ok_or_else(|| NovaError::Internal("index disappeared".to_owned()))?
                    .tree
                    .insert(key, document.id())?;
            }
        }
        self.sync_collection(collection)
    }

    /// Replaces all applicable index entries for a document.
    ///
    /// # Errors
    ///
    /// Returns a typed unindexable-value or persistence error.
    pub fn replace_document(
        &mut self,
        collection: &str,
        old: &Document,
        new: &Document,
    ) -> Result<()> {
        if old.id() != new.id() {
            return Err(NovaError::InvalidArgument(
                "replacement document id must not change".to_owned(),
            ));
        }
        let old_keys = self.keys_for_document(collection, old)?;
        let new_keys = self.keys_for_document(collection, new)?;
        for ((old_name, old_key), (new_name, new_key)) in old_keys.into_iter().zip(new_keys) {
            if old_name != new_name {
                return Err(NovaError::Internal("index ordering changed".to_owned()));
            }
            let tree = &mut self
                .indexes
                .get_mut(&old_name)
                .ok_or_else(|| NovaError::Internal("index disappeared".to_owned()))?
                .tree;
            if old_key != new_key {
                if let Some(key) = old_key {
                    tree.remove(&key, old.id());
                }
                if let Some(key) = new_key {
                    tree.insert(key, new.id())?;
                }
            }
        }
        self.sync_collection(collection)
    }

    /// Removes a document from all applicable indexes.
    ///
    /// # Errors
    ///
    /// Returns a typed unindexable-value or persistence error.
    pub fn delete_document(&mut self, collection: &str, document: &Document) -> Result<()> {
        let changes = self.keys_for_document(collection, document)?;
        for (name, key) in changes {
            if let Some(key) = key {
                self.indexes
                    .get_mut(&name)
                    .ok_or_else(|| NovaError::Internal("index disappeared".to_owned()))?
                    .tree
                    .remove(&key, document.id());
            }
        }
        self.sync_collection(collection)
    }

    /// Looks up an exact key in a named index.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::NotFound`] for an unknown index.
    pub fn lookup(&self, name: &str, key: &IndexKey) -> Result<Vec<NovaId>> {
        self.indexes
            .get(name)
            .map(|index| index.tree.get(key))
            .ok_or_else(|| NovaError::NotFound(format!("index {name:?}")))
    }

    /// Validates every indexed value for a document without mutating trees.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::InvalidArgument`] for an unindexable value.
    pub fn validate_document(&self, collection: &str, document: &Document) -> Result<()> {
        self.keys_for_document(collection, document).map(|_| ())
    }

    fn keys_for_document(
        &self,
        collection: &str,
        document: &Document,
    ) -> Result<Vec<(String, Option<IndexKey>)>> {
        self.indexes
            .values()
            .filter(|index| index.definition.collection == collection)
            .map(|index| {
                Ok((
                    index.definition.name.clone(),
                    document_key(&index.definition, document)?,
                ))
            })
            .collect()
    }

    fn sync_collection(&mut self, collection: &str) -> Result<()> {
        for index in self
            .indexes
            .values_mut()
            .filter(|index| index.definition.collection == collection)
        {
            index.tree.sync()?;
        }
        Ok(())
    }
}

fn document_key(definition: &IndexDefinition, document: &Document) -> Result<Option<IndexKey>> {
    let value = if definition.field.as_slice() == ["_id"] {
        return Ok(Some(IndexKey::NovaId(document.id())));
    } else {
        document.lookup(&definition.field.join("."))
    };
    match value {
        None | Some(NovaValue::Null) => Ok(None),
        Some(value) => IndexKey::try_from(value).map(Some).map_err(|error| {
            NovaError::InvalidArgument(format!(
                "index {:?} field {}: {error}",
                definition.name,
                definition.field.join(".")
            ))
        }),
    }
}

fn validate_identifier(value: &str, kind: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|character| character == '_' || character.is_alphanumeric())
    {
        return Err(NovaError::InvalidArgument(format!(
            "invalid {kind} identifier {value:?}"
        )));
    }
    Ok(())
}

fn validate_field(field: &[String]) -> Result<()> {
    if field.is_empty() {
        return Err(NovaError::InvalidArgument(
            "index field path cannot be empty".to_owned(),
        ));
    }
    for segment in field {
        validate_identifier(segment, "field")?;
    }
    Ok(())
}

fn meta_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}.meta"))
}

fn tree_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}.nvix"))
}

fn write_definition(path: &Path, definition: &IndexDefinition) -> Result<()> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(META_MAGIC);
    bytes.extend_from_slice(&META_VERSION.to_be_bytes());
    write_text(&mut bytes, &definition.name)?;
    write_text(&mut bytes, &definition.collection)?;
    write_u32(&mut bytes, definition.field.len())?;
    for segment in &definition.field {
        write_text(&mut bytes, segment)?;
    }
    let checksum = crc32(&bytes);
    bytes.extend_from_slice(&checksum.to_be_bytes());
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_definition(path: &Path) -> Result<IndexDefinition> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    if bytes.len() < 10 || &bytes[..4] != META_MAGIC {
        return Err(NovaError::Corruption(
            "invalid index metadata header".to_owned(),
        ));
    }
    let payload_len = bytes.len() - 4;
    let stored = u32::from_be_bytes(
        bytes[payload_len..]
            .try_into()
            .map_err(|_| NovaError::Corruption("truncated metadata checksum".to_owned()))?,
    );
    if stored != crc32(&bytes[..payload_len]) {
        return Err(NovaError::Corruption(
            "index metadata checksum mismatch".to_owned(),
        ));
    }
    let mut cursor = MetaCursor::new(&bytes[..payload_len]);
    cursor.take(4)?;
    let version = cursor.u16()?;
    if version != META_VERSION {
        return Err(NovaError::Unsupported(format!(
            "index metadata version {version}"
        )));
    }
    let name = cursor.text()?;
    let collection = cursor.text()?;
    let count =
        usize::try_from(cursor.u32()?).map_err(|error| NovaError::Corruption(error.to_string()))?;
    if count == 0 || count > cursor.remaining() {
        return Err(NovaError::Corruption(
            "impossible index field count".to_owned(),
        ));
    }
    let mut field = Vec::with_capacity(count);
    for _ in 0..count {
        field.push(cursor.text()?);
    }
    if cursor.remaining() != 0 {
        return Err(NovaError::Corruption(
            "trailing index metadata bytes".to_owned(),
        ));
    }
    Ok(IndexDefinition {
        name,
        collection,
        field,
    })
}

fn write_text(output: &mut Vec<u8>, value: &str) -> Result<()> {
    write_u32(output, value.len())?;
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_u32(output: &mut Vec<u8>, value: usize) -> Result<()> {
    output.extend_from_slice(
        &u32::try_from(value)
            .map_err(|error| NovaError::Storage(error.to_string()))?
            .to_be_bytes(),
    );
    Ok(())
}

struct MetaCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> MetaCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| NovaError::Corruption("index metadata offset overflow".to_owned()))?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| NovaError::Corruption("truncated index metadata".to_owned()))?;
        self.position = end;
        Ok(result)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().map_err(
            |_| NovaError::Internal("fixed metadata decode failed".to_owned()),
        )?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().map_err(
            |_| NovaError::Internal("fixed metadata decode failed".to_owned()),
        )?))
    }

    fn text(&mut self) -> Result<String> {
        let length = usize::try_from(self.u32()?)
            .map_err(|error| NovaError::Corruption(error.to_string()))?;
        if length > self.remaining() {
            return Err(NovaError::Corruption(
                "impossible index metadata string length".to_owned(),
            ));
        }
        let text = std::str::from_utf8(self.take(length)?)
            .map_err(|_| NovaError::Corruption("metadata string is not UTF-8".to_owned()))?;
        Ok(text.to_owned())
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "novadb-catalog-{label}-{}-{sequence}",
                std::process::id()
            )))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn id(seed: u8) -> NovaId {
        NovaId::new(u64::from(seed), [seed; 8])
    }

    fn student(seed: u8, branch: &str, score: i64) -> Document {
        let mut profile = Document::new(id(seed + 20));
        profile.insert("score", score.into());
        let mut document = Document::new(id(seed));
        document.insert("branch", branch.into());
        document.insert("profile", profile.into());
        document
    }

    #[test]
    fn create_backfill_mutate_reopen_and_drop() {
        let directory = TestDirectory::new("lifecycle");
        let ada = student(1, "CSE", 90);
        let bob = student(2, "ECE", 70);
        let mut catalog = IndexCatalog::open(&directory.0).unwrap();
        catalog
            .create_index(
                "by_branch",
                "students",
                vec!["branch".to_owned()],
                &[ada.clone(), bob.clone()],
            )
            .unwrap();
        catalog
            .create_index(
                "by_score",
                "students",
                vec!["profile".to_owned(), "score".to_owned()],
                &[ada.clone(), bob.clone()],
            )
            .unwrap();
        assert_eq!(
            catalog
                .lookup("by_branch", &IndexKey::String("CSE".to_owned()))
                .unwrap(),
            vec![ada.id()]
        );

        let cara = student(3, "CSE", 85);
        catalog.insert_document("students", &cara).unwrap();
        let updated = student(2, "CSE", 75);
        catalog
            .replace_document("students", &bob, &updated)
            .unwrap();
        catalog.delete_document("students", &ada).unwrap();
        drop(catalog);

        let mut reopened = IndexCatalog::open(&directory.0).unwrap();
        assert_eq!(reopened.definitions().len(), 2);
        assert_eq!(
            reopened
                .lookup("by_branch", &IndexKey::String("CSE".to_owned()))
                .unwrap(),
            vec![updated.id(), cara.id()]
        );
        assert_eq!(
            reopened.lookup("by_score", &IndexKey::Int64(75)).unwrap(),
            vec![updated.id()]
        );
        reopened.drop_collection_indexes("students").unwrap();
        assert!(reopened.definitions().is_empty());
        assert!(IndexCatalog::open(&directory.0)
            .unwrap()
            .definitions()
            .is_empty());
    }

    #[test]
    fn missing_null_and_unindexable_values_are_explicit() {
        let directory = TestDirectory::new("values");
        let mut missing = Document::new(id(1));
        missing.insert("other", true.into());
        let mut null = Document::new(id(2));
        null.insert("tag", NovaValue::Null);
        let mut bad = Document::new(id(3));
        bad.insert("tag", NovaValue::Array(vec![]));
        let mut catalog = IndexCatalog::open(&directory.0).unwrap();
        catalog
            .create_index("by_tag", "items", vec!["tag".to_owned()], &[missing, null])
            .unwrap();
        assert!(catalog
            .lookup("by_tag", &IndexKey::String("x".to_owned()))
            .unwrap()
            .is_empty());
        assert!(matches!(
            catalog.insert_document("items", &bad),
            Err(NovaError::InvalidArgument(_))
        ));
    }

    #[test]
    fn corrupt_metadata_and_duplicate_names_are_rejected() {
        let directory = TestDirectory::new("corrupt");
        let mut catalog = IndexCatalog::open(&directory.0).unwrap();
        catalog
            .create_index("by_id", "items", vec!["_id".to_owned()], &[])
            .unwrap();
        assert!(matches!(
            catalog.create_index("by_id", "items", vec!["_id".to_owned()], &[]),
            Err(NovaError::AlreadyExists(_))
        ));
        drop(catalog);
        let path = meta_path(&directory.0, "by_id");
        let mut bytes = fs::read(&path).unwrap();
        bytes[6] ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(matches!(
            IndexCatalog::open(&directory.0),
            Err(NovaError::Corruption(_))
        ));
    }
}
