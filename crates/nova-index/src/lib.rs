//! Persistent B+ tree indexes for NovaDB.
//!
//! Nodes are maintained by NovaDB itself and persisted in a checksummed,
//! versioned snapshot; tree operations do not wrap an external ordered map.

#![forbid(unsafe_code)]

mod catalog;

use std::cmp::Ordering;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use nova_core::error::{NovaError, Result};
use nova_core::{NovaId, NovaValue};

pub use catalog::{IndexCatalog, IndexDefinition};

const MAGIC: &[u8; 4] = b"NVIX";
const VERSION: u16 = 1;
const HEADER_SIZE: usize = 40;
const HEADER_SIZE_U16: u16 = 40;
const CHECKSUM_OFFSET: usize = 32;
const DEFAULT_MAX_KEYS: usize = 31;
const MIN_MAX_KEYS: usize = 3;
const NONE_NODE: u64 = u64::MAX;
const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;

/// A float with deterministic total ordering and canonical `NaN` representation.
#[derive(Debug, Copy, Clone)]
pub struct OrderedF64(u64);

impl OrderedF64 {
    /// Creates an ordered float. Every `NaN` is normalized to one quiet `NaN`.
    #[must_use]
    pub fn new(value: f64) -> Self {
        const CANONICAL_NAN: u64 = 0x7ff8_0000_0000_0000;
        Self(if value.is_nan() {
            CANONICAL_NAN
        } else {
            value.to_bits()
        })
    }

    /// Returns the represented floating-point value.
    #[must_use]
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl PartialEq for OrderedF64 {
    fn eq(&self, other: &Self) -> bool {
        self.get().total_cmp(&other.get()) == Ordering::Equal
    }
}

impl Eq for OrderedF64 {}

impl PartialOrd for OrderedF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.get().total_cmp(&other.get())
    }
}

/// Scalar values supported by a NovaDB B+ tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexKey {
    /// Signed 64-bit integer.
    Int64(i64),
    /// Totally ordered IEEE-754 value.
    Float64(OrderedF64),
    /// UTF-8 string, ordered by its bytes.
    String(String),
    /// NovaDB identifier.
    NovaId(NovaId),
}

impl IndexKey {
    /// Constructs a canonical floating-point key.
    #[must_use]
    pub fn float64(value: f64) -> Self {
        Self::Float64(OrderedF64::new(value))
    }
}

impl TryFrom<&NovaValue> for IndexKey {
    type Error = NovaError;

    fn try_from(value: &NovaValue) -> Result<Self> {
        match value {
            NovaValue::Int64(value) => Ok(Self::Int64(*value)),
            NovaValue::Float64(value) => Ok(Self::float64(*value)),
            NovaValue::String(value) => Ok(Self::String(value.clone())),
            NovaValue::NovaId(value) => Ok(Self::NovaId(*value)),
            other => Err(NovaError::InvalidArgument(format!(
                "{} values cannot be indexed",
                other.type_name()
            ))),
        }
    }
}

#[derive(Debug, Clone)]
enum Node {
    Leaf {
        entries: Vec<(IndexKey, Vec<NovaId>)>,
        next: Option<usize>,
    },
    Internal {
        keys: Vec<IndexKey>,
        children: Vec<usize>,
    },
}

/// A persistent B+ tree mapping typed keys to document identifiers.
#[derive(Debug)]
pub struct BPlusTree {
    path: PathBuf,
    max_keys: usize,
    root: usize,
    nodes: Vec<Node>,
    dirty: bool,
}

impl BPlusTree {
    /// Creates a new empty tree using the production node capacity.
    ///
    /// # Errors
    ///
    /// Returns a typed error if the file exists or cannot be created.
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        Self::create_with_max_keys(path, DEFAULT_MAX_KEYS)
    }

    /// Creates a tree with a specific odd node capacity of at least three.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an invalid capacity or filesystem failure.
    pub fn create_with_max_keys(path: impl AsRef<Path>, max_keys: usize) -> Result<Self> {
        validate_max_keys(max_keys)?;
        let path = path.as_ref().to_path_buf();
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    NovaError::AlreadyExists(format!("index file {}", path.display()))
                } else {
                    error.into()
                }
            })?;
        let mut tree = Self {
            path,
            max_keys,
            root: 0,
            nodes: vec![Node::Leaf {
                entries: Vec::new(),
                next: None,
            }],
            dirty: true,
        };
        tree.sync()?;
        Ok(tree)
    }

    /// Opens and fully validates an existing tree snapshot.
    ///
    /// # Errors
    ///
    /// Returns a typed I/O, unsupported-version, or corruption error.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut bytes = Vec::new();
        File::open(&path)?.read_to_end(&mut bytes)?;
        let (max_keys, root, nodes) = decode_tree(&bytes)?;
        let tree = Self {
            path,
            max_keys,
            root,
            nodes,
            dirty: false,
        };
        tree.verify()?;
        Ok(tree)
    }

    /// Returns the path backing this tree.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Inserts a document id under `key`; duplicate pairs are idempotent.
    ///
    /// # Errors
    ///
    /// Returns a typed error if node growth exceeds representable limits.
    pub fn insert(&mut self, key: IndexKey, id: NovaId) -> Result<()> {
        if let Some((separator, right)) = self.insert_node(self.root, key, id)? {
            let old_root = self.root;
            self.nodes.push(Node::Internal {
                keys: vec![separator],
                children: vec![old_root, right],
            });
            self.root = self.nodes.len() - 1;
        }
        self.dirty = true;
        Ok(())
    }

    /// Returns all document ids stored for an exact key.
    #[must_use]
    pub fn get(&self, key: &IndexKey) -> Vec<NovaId> {
        match &self.nodes[self.find_leaf(key)] {
            Node::Leaf { entries, .. } => entries
                .binary_search_by(|(candidate, _)| candidate.cmp(key))
                .ok()
                .map_or_else(Vec::new, |index| entries[index].1.clone()),
            Node::Internal { .. } => unreachable!("find_leaf always returns a leaf"),
        }
    }

    /// Returns `(key, id)` pairs in an inclusive range, in key/id order.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::InvalidArgument`] when `start` exceeds `end`.
    pub fn range(&self, start: &IndexKey, end: &IndexKey) -> Result<Vec<(IndexKey, NovaId)>> {
        if start > end {
            return Err(NovaError::InvalidArgument(
                "index range start must not exceed end".to_owned(),
            ));
        }
        let mut result = Vec::new();
        let mut leaf = Some(self.find_leaf(start));
        while let Some(node_id) = leaf {
            let Node::Leaf { entries, next } = &self.nodes[node_id] else {
                return Err(NovaError::Internal(
                    "leaf chain reached internal node".to_owned(),
                ));
            };
            for (key, ids) in entries {
                if key < start {
                    continue;
                }
                if key > end {
                    return Ok(result);
                }
                result.extend(ids.iter().map(|id| (key.clone(), *id)));
            }
            leaf = *next;
        }
        Ok(result)
    }

    /// Removes one exact `(key, id)` pair, returning whether it existed.
    pub fn remove(&mut self, key: &IndexKey, id: NovaId) -> bool {
        let leaf = self.find_leaf(key);
        let Node::Leaf { entries, .. } = &mut self.nodes[leaf] else {
            unreachable!("find_leaf always returns a leaf");
        };
        let Ok(entry_index) = entries.binary_search_by(|(candidate, _)| candidate.cmp(key)) else {
            return false;
        };
        let ids = &mut entries[entry_index].1;
        let Ok(id_index) = ids.binary_search(&id) else {
            return false;
        };
        ids.remove(id_index);
        if ids.is_empty() {
            entries.remove(entry_index);
        }
        self.dirty = true;
        true
    }

    /// Writes and synchronizes the complete checksummed snapshot.
    ///
    /// # Errors
    ///
    /// Returns a typed verification, encoding, or filesystem error.
    pub fn sync(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.verify()?;
        let bytes = encode_tree(self.max_keys, self.root, &self.nodes)?;
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        self.dirty = false;
        Ok(())
    }

    /// Verifies capacities, ordering, references, separator bounds, leaf depth,
    /// id order, reachability, and the linked-leaf chain.
    ///
    /// # Errors
    ///
    /// Returns [`NovaError::Corruption`] for any invalid tree invariant.
    pub fn verify(&self) -> Result<()> {
        if self.nodes.is_empty() || self.root >= self.nodes.len() {
            return corruption("invalid root node");
        }
        validate_max_keys(self.max_keys)
            .map_err(|error| NovaError::Corruption(error.to_string()))?;
        let mut visited = vec![false; self.nodes.len()];
        let mut leaf_depth = None;
        self.verify_node(self.root, None, None, 0, &mut leaf_depth, &mut visited)?;
        if visited.iter().any(|seen| !seen) {
            return corruption("tree contains unreachable nodes");
        }
        self.verify_leaf_chain()
    }

    fn insert_node(
        &mut self,
        node_id: usize,
        key: IndexKey,
        id: NovaId,
    ) -> Result<Option<(IndexKey, usize)>> {
        match self.nodes[node_id].clone() {
            Node::Leaf { mut entries, next } => {
                match entries.binary_search_by(|(candidate, _)| candidate.cmp(&key)) {
                    Ok(index) => match entries[index].1.binary_search(&id) {
                        Ok(_) => return Ok(None),
                        Err(id_index) => entries[index].1.insert(id_index, id),
                    },
                    Err(index) => entries.insert(index, (key, vec![id])),
                }
                if entries.len() <= self.max_keys {
                    self.nodes[node_id] = Node::Leaf { entries, next };
                    return Ok(None);
                }
                let right_entries = entries.split_off(entries.len() / 2);
                let separator = right_entries[0].0.clone();
                let right = self.nodes.len();
                self.nodes[node_id] = Node::Leaf {
                    entries,
                    next: Some(right),
                };
                self.nodes.push(Node::Leaf {
                    entries: right_entries,
                    next,
                });
                Ok(Some((separator, right)))
            }
            Node::Internal {
                mut keys,
                mut children,
            } => {
                let index = child_index(&keys, &key);
                if let Some((separator, right)) = self.insert_node(children[index], key, id)? {
                    keys.insert(index, separator);
                    children.insert(index + 1, right);
                }
                if keys.len() <= self.max_keys {
                    self.nodes[node_id] = Node::Internal { keys, children };
                    return Ok(None);
                }
                let middle = keys.len() / 2;
                let right_keys = keys.split_off(middle + 1);
                let separator = keys
                    .pop()
                    .ok_or_else(|| NovaError::Internal("missing split separator".to_owned()))?;
                let right_children = children.split_off(middle + 1);
                let right = self.nodes.len();
                self.nodes[node_id] = Node::Internal { keys, children };
                self.nodes.push(Node::Internal {
                    keys: right_keys,
                    children: right_children,
                });
                Ok(Some((separator, right)))
            }
        }
    }

    fn find_leaf(&self, key: &IndexKey) -> usize {
        let mut node_id = self.root;
        loop {
            match &self.nodes[node_id] {
                Node::Leaf { .. } => return node_id,
                Node::Internal { keys, children } => {
                    node_id = children[child_index(keys, key)];
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_node(
        &self,
        node_id: usize,
        lower: Option<&IndexKey>,
        upper: Option<&IndexKey>,
        depth: usize,
        leaf_depth: &mut Option<usize>,
        visited: &mut [bool],
    ) -> Result<()> {
        if node_id >= self.nodes.len() || visited[node_id] {
            return corruption("node reference is invalid or cyclic");
        }
        visited[node_id] = true;
        match &self.nodes[node_id] {
            Node::Leaf { entries, next } => {
                if entries.len() > self.max_keys {
                    return corruption("leaf exceeds its key capacity");
                }
                if next.is_some_and(|next_id| next_id >= self.nodes.len()) {
                    return corruption("leaf next pointer is invalid");
                }
                if entries.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
                    return corruption("leaf keys are not strictly increasing");
                }
                for (key, ids) in entries {
                    if lower.is_some_and(|bound| key < bound)
                        || upper.is_some_and(|bound| key >= bound)
                    {
                        return corruption("leaf key violates parent separator bounds");
                    }
                    if ids.is_empty() || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
                        return corruption("leaf document ids are empty, duplicated, or unsorted");
                    }
                }
                match leaf_depth {
                    Some(expected) if *expected != depth => {
                        return corruption("leaves occur at different depths");
                    }
                    Some(_) => {}
                    None => *leaf_depth = Some(depth),
                }
            }
            Node::Internal { keys, children } => {
                if keys.is_empty() || keys.len() > self.max_keys || children.len() != keys.len() + 1
                {
                    return corruption("internal node has invalid key/child counts");
                }
                if keys.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return corruption("internal keys are not strictly increasing");
                }
                for (index, child) in children.iter().copied().enumerate() {
                    let child_lower = if index == 0 {
                        lower
                    } else {
                        Some(&keys[index - 1])
                    };
                    let child_upper = if index == keys.len() {
                        upper
                    } else {
                        Some(&keys[index])
                    };
                    self.verify_node(
                        child,
                        child_lower,
                        child_upper,
                        depth + 1,
                        leaf_depth,
                        visited,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn verify_leaf_chain(&self) -> Result<()> {
        let mut node_id = self.root;
        while let Node::Internal { children, .. } = &self.nodes[node_id] {
            node_id = children[0];
        }
        let mut seen = vec![false; self.nodes.len()];
        let mut previous: Option<&IndexKey> = None;
        let mut leaf_count = 0;
        loop {
            if seen[node_id] {
                return corruption("leaf chain contains a cycle");
            }
            seen[node_id] = true;
            leaf_count += 1;
            let Node::Leaf { entries, next } = &self.nodes[node_id] else {
                return corruption("leaf chain points to an internal node");
            };
            for (key, _) in entries {
                if previous.is_some_and(|prior| prior >= key) {
                    return corruption("leaf chain is not globally ordered");
                }
                previous = Some(key);
            }
            let Some(next_id) = next else { break };
            node_id = *next_id;
        }
        let actual = self
            .nodes
            .iter()
            .filter(|node| matches!(node, Node::Leaf { .. }))
            .count();
        if leaf_count != actual {
            return corruption("leaf chain does not cover every leaf");
        }
        Ok(())
    }
}

fn child_index(keys: &[IndexKey], key: &IndexKey) -> usize {
    keys.partition_point(|separator| key >= separator)
}

fn validate_max_keys(max_keys: usize) -> Result<()> {
    if max_keys < MIN_MAX_KEYS || max_keys % 2 == 0 || max_keys > u32::MAX as usize {
        return Err(NovaError::InvalidArgument(format!(
            "B+ tree max_keys must be an odd value from {MIN_MAX_KEYS} through {}",
            u32::MAX
        )));
    }
    Ok(())
}

fn encode_tree(max_keys: usize, root: usize, nodes: &[Node]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_be_bytes());
    bytes.extend_from_slice(&HEADER_SIZE_U16.to_be_bytes());
    bytes.extend_from_slice(&u32::try_from(max_keys).map_err(size_error)?.to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&u64::try_from(root).map_err(size_error)?.to_be_bytes());
    bytes.extend_from_slice(
        &u64::try_from(nodes.len())
            .map_err(size_error)?
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    for node in nodes {
        match node {
            Node::Leaf { entries, next } => {
                bytes.push(0);
                let next =
                    next.map_or(Ok(NONE_NODE), |id| u64::try_from(id).map_err(size_error))?;
                bytes.extend_from_slice(&next.to_be_bytes());
                write_len(&mut bytes, entries.len())?;
                for (key, ids) in entries {
                    encode_key(&mut bytes, key)?;
                    write_len(&mut bytes, ids.len())?;
                    for id in ids {
                        bytes.extend_from_slice(id.as_bytes());
                    }
                }
            }
            Node::Internal { keys, children } => {
                bytes.push(1);
                write_len(&mut bytes, keys.len())?;
                for key in keys {
                    encode_key(&mut bytes, key)?;
                }
                write_len(&mut bytes, children.len())?;
                for child in children {
                    bytes.extend_from_slice(
                        &u64::try_from(*child).map_err(size_error)?.to_be_bytes(),
                    );
                }
            }
        }
    }
    let checksum = crc32_with_zeroed_checksum(&bytes);
    bytes[CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4].copy_from_slice(&checksum.to_be_bytes());
    Ok(bytes)
}

fn decode_tree(bytes: &[u8]) -> Result<(usize, usize, Vec<Node>)> {
    if bytes.len() < HEADER_SIZE {
        return corruption("index file is shorter than its header");
    }
    if &bytes[..4] != MAGIC {
        return corruption("invalid index magic");
    }
    let mut cursor = Cursor::new(bytes);
    cursor.take(4)?;
    let version = cursor.u16()?;
    if version != VERSION {
        return Err(NovaError::Unsupported(format!(
            "index format version {version}"
        )));
    }
    if usize::from(cursor.u16()?) != HEADER_SIZE {
        return corruption("invalid index header size");
    }
    let max_keys = usize::try_from(cursor.u32()?).map_err(size_error)?;
    validate_max_keys(max_keys).map_err(|error| NovaError::Corruption(error.to_string()))?;
    if cursor.u32()? != 0 {
        return corruption("index header reserved field is non-zero");
    }
    let root = usize::try_from(cursor.u64()?).map_err(size_error)?;
    let node_count = usize::try_from(cursor.u64()?).map_err(size_error)?;
    let stored_checksum = cursor.u32()?;
    if cursor.u32()? != 0 {
        return corruption("index header reserved field is non-zero");
    }
    if stored_checksum != crc32_with_zeroed_checksum(bytes) {
        return corruption("index checksum mismatch");
    }
    if node_count == 0 || node_count > bytes.len().saturating_sub(HEADER_SIZE) {
        return corruption("impossible index node count");
    }
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        match cursor.u8()? {
            0 => {
                let raw_next = cursor.u64()?;
                let next = if raw_next == NONE_NODE {
                    None
                } else {
                    Some(usize::try_from(raw_next).map_err(size_error)?)
                };
                let entry_count = cursor.count(max_keys)?;
                let mut entries = Vec::with_capacity(entry_count);
                for _ in 0..entry_count {
                    let key = decode_key(&mut cursor)?;
                    let id_count = cursor.count(bytes.len() / 16)?;
                    if id_count == 0 {
                        return corruption("index key has no document ids");
                    }
                    let mut ids = Vec::with_capacity(id_count);
                    for _ in 0..id_count {
                        ids.push(NovaId::from_bytes(cursor.take(16)?)?);
                    }
                    entries.push((key, ids));
                }
                nodes.push(Node::Leaf { entries, next });
            }
            1 => {
                let key_count = cursor.count(max_keys)?;
                let mut keys = Vec::with_capacity(key_count);
                for _ in 0..key_count {
                    keys.push(decode_key(&mut cursor)?);
                }
                let child_count = cursor.count(max_keys + 1)?;
                let mut children = Vec::with_capacity(child_count);
                for _ in 0..child_count {
                    children.push(usize::try_from(cursor.u64()?).map_err(size_error)?);
                }
                nodes.push(Node::Internal { keys, children });
            }
            tag => return corruption(&format!("unknown index node tag {tag}")),
        }
    }
    if cursor.remaining() != 0 {
        return corruption("trailing bytes after index nodes");
    }
    Ok((max_keys, root, nodes))
}

fn encode_key(output: &mut Vec<u8>, key: &IndexKey) -> Result<()> {
    match key {
        IndexKey::Int64(value) => {
            output.push(0);
            output.extend_from_slice(&value.to_be_bytes());
        }
        IndexKey::Float64(value) => {
            output.push(1);
            output.extend_from_slice(&value.0.to_be_bytes());
        }
        IndexKey::String(value) => {
            output.push(2);
            write_len(output, value.len())?;
            output.extend_from_slice(value.as_bytes());
        }
        IndexKey::NovaId(value) => {
            output.push(3);
            output.extend_from_slice(value.as_bytes());
        }
    }
    Ok(())
}

fn decode_key(cursor: &mut Cursor<'_>) -> Result<IndexKey> {
    match cursor.u8()? {
        0 => Ok(IndexKey::Int64(i64::from_be_bytes(
            cursor
                .take(8)?
                .try_into()
                .map_err(|_| fixed_width_error())?,
        ))),
        1 => Ok(IndexKey::Float64(OrderedF64::new(f64::from_bits(
            cursor.u64()?,
        )))),
        2 => {
            let length = cursor.count(MAX_STRING_BYTES)?;
            let text = std::str::from_utf8(cursor.take(length)?)
                .map_err(|_| NovaError::Corruption("index string is not UTF-8".to_owned()))?;
            Ok(IndexKey::String(text.to_owned()))
        }
        3 => Ok(IndexKey::NovaId(NovaId::from_bytes(cursor.take(16)?)?)),
        tag => corruption(&format!("unknown index key tag {tag}")),
    }
}

fn write_len(output: &mut Vec<u8>, length: usize) -> Result<()> {
    output.extend_from_slice(&u32::try_from(length).map_err(size_error)?.to_be_bytes());
    Ok(())
}

struct Cursor<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self { input, position: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| NovaError::Corruption("index offset overflow".to_owned()))?;
        let value = self
            .input
            .get(self.position..end)
            .ok_or_else(|| NovaError::Corruption("truncated index file".to_owned()))?;
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().map_err(|_| fixed_width_error())?,
        ))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().map_err(|_| fixed_width_error())?,
        ))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().map_err(|_| fixed_width_error())?,
        ))
    }

    fn count(&mut self, maximum: usize) -> Result<usize> {
        let count = usize::try_from(self.u32()?).map_err(size_error)?;
        if count > maximum || count > self.remaining() {
            return corruption("impossible index collection count");
        }
        Ok(count)
    }

    fn remaining(&self) -> usize {
        self.input.len() - self.position
    }
}

fn crc32_with_zeroed_checksum(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for (index, byte) in bytes.iter().copied().enumerate() {
        let value = if (CHECKSUM_OFFSET..CHECKSUM_OFFSET + 4).contains(&index) {
            0
        } else {
            byte
        };
        crc ^= u32::from(value);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320_u32 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn size_error(error: impl std::fmt::Display) -> NovaError {
    NovaError::Storage(format!("index size is not representable: {error}"))
}

fn fixed_width_error() -> NovaError {
    NovaError::Internal("fixed-width index decode failed".to_owned())
}

fn corruption<T>(message: &str) -> Result<T> {
    Err(NovaError::Corruption(message.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    use super::*;

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    struct TestFile(PathBuf);

    impl TestFile {
        fn new(label: &str) -> Self {
            let sequence = NEXT_FILE.fetch_add(1, AtomicOrdering::Relaxed);
            Self(std::env::temp_dir().join(format!(
                "novadb-index-{label}-{}-{sequence}.nvix",
                std::process::id()
            )))
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn id(value: u8) -> NovaId {
        NovaId::new(u64::from(value), [value; 8])
    }

    #[test]
    fn inserts_split_multiple_levels_and_lookup_exactly() {
        let file = TestFile::new("splits");
        let mut tree = BPlusTree::create_with_max_keys(&file.0, 3).unwrap();
        for value in (0_u8..100).rev() {
            tree.insert(IndexKey::Int64(i64::from(value)), id(value))
                .unwrap();
        }
        tree.verify().unwrap();
        for value in 0_u8..100 {
            assert_eq!(
                tree.get(&IndexKey::Int64(i64::from(value))),
                vec![id(value)]
            );
        }
        assert!(tree.get(&IndexKey::Int64(101)).is_empty());
    }

    #[test]
    fn duplicate_keys_are_sorted_idempotent_and_removable() {
        let file = TestFile::new("duplicates");
        let mut tree = BPlusTree::create_with_max_keys(&file.0, 3).unwrap();
        let key = IndexKey::String("CSE".to_owned());
        tree.insert(key.clone(), id(3)).unwrap();
        tree.insert(key.clone(), id(1)).unwrap();
        tree.insert(key.clone(), id(2)).unwrap();
        tree.insert(key.clone(), id(2)).unwrap();
        assert_eq!(tree.get(&key), vec![id(1), id(2), id(3)]);
        assert!(tree.remove(&key, id(2)));
        assert!(!tree.remove(&key, id(2)));
        assert_eq!(tree.get(&key), vec![id(1), id(3)]);
        tree.verify().unwrap();
    }

    #[test]
    fn inclusive_range_crosses_leaf_boundaries() {
        let file = TestFile::new("range");
        let mut tree = BPlusTree::create_with_max_keys(&file.0, 3).unwrap();
        for value in 0_u8..20 {
            tree.insert(IndexKey::Int64(i64::from(value)), id(value))
                .unwrap();
        }
        let rows = tree
            .range(&IndexKey::Int64(6), &IndexKey::Int64(13))
            .unwrap();
        assert_eq!(rows.len(), 8);
        assert_eq!(rows.first().unwrap(), &(IndexKey::Int64(6), id(6)));
        assert_eq!(rows.last().unwrap(), &(IndexKey::Int64(13), id(13)));
        assert!(tree
            .range(&IndexKey::Int64(2), &IndexKey::Int64(1))
            .is_err());
    }

    #[test]
    fn every_key_type_has_deterministic_ordering() {
        assert!(IndexKey::Int64(9) < IndexKey::float64(-100.0));
        assert!(IndexKey::float64(f64::NEG_INFINITY) < IndexKey::float64(-0.0));
        assert!(IndexKey::float64(-0.0) < IndexKey::float64(0.0));
        assert!(IndexKey::float64(f64::INFINITY) < IndexKey::float64(f64::NAN));
        assert_eq!(IndexKey::float64(f64::NAN), IndexKey::float64(-f64::NAN));
        assert!(IndexKey::String("z".to_owned()) < IndexKey::NovaId(id(0)));
    }

    #[test]
    fn sync_reopen_preserves_tree_and_future_splits() {
        let file = TestFile::new("reopen");
        {
            let mut tree = BPlusTree::create_with_max_keys(&file.0, 3).unwrap();
            for value in 0_u8..40 {
                tree.insert(IndexKey::Int64(i64::from(value)), id(value))
                    .unwrap();
            }
            tree.sync().unwrap();
        }
        let mut tree = BPlusTree::open(&file.0).unwrap();
        for value in 40_u8..80 {
            tree.insert(IndexKey::Int64(i64::from(value)), id(value))
                .unwrap();
        }
        tree.sync().unwrap();
        let reopened = BPlusTree::open(&file.0).unwrap();
        reopened.verify().unwrap();
        assert_eq!(reopened.get(&IndexKey::Int64(0)), vec![id(0)]);
        assert_eq!(reopened.get(&IndexKey::Int64(79)), vec![id(79)]);
    }

    #[test]
    fn unsupported_values_and_bad_capacities_are_typed_errors() {
        assert!(matches!(
            IndexKey::try_from(&NovaValue::Boolean(true)),
            Err(NovaError::InvalidArgument(_))
        ));
        let file = TestFile::new("capacity");
        assert!(matches!(
            BPlusTree::create_with_max_keys(&file.0, 2),
            Err(NovaError::InvalidArgument(_))
        ));
    }

    #[test]
    fn corrupted_and_truncated_files_never_panic() {
        let file = TestFile::new("corruption");
        let valid = {
            let mut tree = BPlusTree::create_with_max_keys(&file.0, 3).unwrap();
            for value in 0_u8..10 {
                tree.insert(IndexKey::Int64(i64::from(value)), id(value))
                    .unwrap();
            }
            tree.sync().unwrap();
            fs::read(&file.0).unwrap()
        };
        for length in 0..valid.len() {
            let result = catch_unwind(AssertUnwindSafe(|| decode_tree(&valid[..length])));
            assert!(result.is_ok(), "decode panicked at length {length}");
            assert!(result.unwrap().is_err(), "truncation {length} was accepted");
        }
        let mut corrupted = valid;
        let index = corrupted.len() - 1;
        corrupted[index] ^= 1;
        fs::write(&file.0, corrupted).unwrap();
        assert!(matches!(
            BPlusTree::open(&file.0),
            Err(NovaError::Corruption(_))
        ));
    }
}
