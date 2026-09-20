//! Document — NovaDB's JSON-like, schema-flexible document.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::nova_id::{NovaId, NOVA_ID_LENGTH, NOVA_ID_PREFIX};
use crate::nova_value::NovaValue;

/// A NovaDB document: a typed field map plus a globally unique [`NovaId`].
///
/// Fields are kept in a `BTreeMap` so that iteration is always in sorted
/// key order. That determinism is what makes NovaDB's binary encoding (NBF,
/// Phase 2) deterministic and its exports reproducible.
///
/// ```text
/// Document {
///     _id: NovaId,             // implicit, always present
///     name: "Swayam",
///     address: { state: "Odisha" }
/// }
/// ```
///
/// The `_id` is a first-class part of the type rather than a normal field; it
/// is accessible through [`Document::id`]. `NovaValue` documents may nest.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    id: NovaId,
    fields: BTreeMap<String, NovaValue>,
}

impl Document {
    /// Creates an empty document with the given id.
    #[must_use]
    pub const fn new(id: NovaId) -> Self {
        Self {
            id,
            fields: BTreeMap::new(),
        }
    }

    /// Creates an empty document with a freshly generated id.
    ///
    /// # Errors
    ///
    /// Returns an [`crate::error::NovaError`] if the OS entropy source is
    /// unavailable.
    pub fn generated() -> Result<Self> {
        Ok(Self::new(NovaId::generate()?))
    }

    /// The document's unique id.
    #[must_use]
    pub const fn id(&self) -> NovaId {
        self.id
    }

    /// Overwrites the document's id.
    pub fn set_id(&mut self, id: NovaId) {
        self.id = id;
    }

    /// Inserts (or replaces) a field, returning the previous value, if any.
    pub fn insert(&mut self, field: impl Into<String>, value: NovaValue) -> Option<NovaValue> {
        self.fields.insert(field.into(), value)
    }

    /// Returns the value for a field name.
    #[must_use]
    pub fn get(&self, field: &str) -> Option<&NovaValue> {
        self.fields.get(field)
    }

    /// Returns a mutable reference to the value for a field name.
    #[must_use]
    pub fn get_mut(&mut self, field: &str) -> Option<&mut NovaValue> {
        self.fields.get_mut(field)
    }

    /// Removes a field, returning its previous value, if any.
    pub fn remove(&mut self, field: &str) -> Option<NovaValue> {
        self.fields.remove(field)
    }

    /// Reports whether a field is present.
    #[must_use]
    pub fn contains_key(&self, field: &str) -> bool {
        self.fields.contains_key(field)
    }

    /// The number of fields (not counting the implicit id).
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Reports whether the document has no fields.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Iterates fields in sorted key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &NovaValue)> {
        self.fields.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Looks up nested values by a dotted path, e.g. `"address.state"`.
    ///
    /// Field names that themselves contain `.` cannot be addressed with this
    /// helper; use [`Document::get`] for those.
    #[must_use]
    pub fn lookup(&self, path: &str) -> Option<&NovaValue> {
        let parts: Vec<&str> = path.split('.').collect();
        self.lookup_parts(&parts)
    }

    /// Looks up nested values by an explicit path segment list.
    ///
    /// Returns `None` when any segment is missing, or when an intermediate
    /// value is not a document. Arrays are not traversed in Phase 1.
    #[must_use]
    pub fn lookup_parts<'a>(&'a self, parts: &[&str]) -> Option<&'a NovaValue> {
        let mut value: Option<&'a NovaValue> = None;
        for (i, part) in parts.iter().enumerate() {
            value = if i == 0 {
                self.fields.get(*part)
            } else {
                value?.as_document()?.get(part)
            };
        }
        value
    }

    /// Consumes the document, returning its field map.
    #[must_use]
    pub fn into_fields(self) -> BTreeMap<String, NovaValue> {
        self.fields
    }

    /// The id's 16 raw bytes, useful for stable keys.
    #[must_use]
    pub fn id_bytes(&self) -> [u8; NOVA_ID_LENGTH] {
        self.id.to_bytes()
    }

    /// The id in human-readable `nova_...` form.
    #[must_use]
    pub fn id_string(&self) -> String {
        format!("{NOVA_ID_PREFIX}{}", self.id.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(ms: u64) -> NovaId {
        NovaId::new(ms, [0u8; 8])
    }

    #[test]
    fn insert_get_remove_cycle() {
        let mut doc = Document::new(id(1));
        assert!(doc.is_empty());
        assert!(doc.insert("name", "Swayam".into()).is_none());
        assert_eq!(doc.len(), 1);
        assert_eq!(doc.get("name"), Some(&NovaValue::from("Swayam")));
        assert!(doc.contains_key("name"));

        // Replacing returns the previous value.
        let prev = doc.insert("name", "Swaya m".into());
        assert_eq!(prev, Some(NovaValue::from("Swayam")));

        let removed = doc.remove("name");
        assert_eq!(removed, Some(NovaValue::from("Swaya m")));
        assert!(!doc.contains_key("name"));
        assert!(doc.is_empty());
    }

    #[test]
    fn iteration_is_deterministic_key_order() {
        let mut doc = Document::new(id(7));
        for (k, v) in [("zeta", 1i64), ("alpha", 2), ("mid", 3)] {
            doc.insert(k, v.into());
        }
        let order: Vec<(&str, i64)> = doc
            .iter()
            .map(|(k, v)| (k, v.as_int64().unwrap()))
            .collect();
        assert_eq!(order, vec![("alpha", 2), ("mid", 3), ("zeta", 1)]);
    }

    #[test]
    fn lookup_traverses_nested_documents() {
        let mut doc = Document::new(id(3));
        let mut address = Document::new(id(4));
        address.insert("state", "Odisha".into());
        address.insert("country", "India".into());
        doc.insert("name", "Swayam".into());
        doc.insert("address", NovaValue::Document(address));

        assert_eq!(doc.lookup("name"), Some(&NovaValue::from("Swayam")));
        assert_eq!(
            doc.lookup("address.state"),
            Some(&NovaValue::from("Odisha"))
        );
        assert_eq!(
            doc.lookup("address.country"),
            Some(&NovaValue::from("India"))
        );

        // Missing segment and path through a non-document both yield None.
        assert!(doc.lookup("address.city").is_none());
        assert!(doc.lookup("name.length").is_none());
        assert!(doc.lookup("missing").is_none());
        assert!(doc.lookup("").is_none());

        // lookup_parts mirrors lookup.
        let parts: Vec<&str> = "address.country".split('.').collect();
        assert_eq!(doc.lookup_parts(&parts), Some(&NovaValue::from("India")));
    }

    #[test]
    fn equality_includes_the_id() {
        let mut a = Document::new(id(1));
        let mut b = Document::new(id(2));
        a.insert("x", NovaValue::Int64(1));
        b.insert("x", NovaValue::Int64(1));
        assert_ne!(a, b);
        b.set_id(id(1));
        assert_eq!(a, b);
    }

    #[test]
    fn generated_documents_are_unique() {
        let a = Document::generated().unwrap();
        let b = Document::generated().unwrap();
        assert_ne!(a.id(), b.id());
        assert!(a.is_empty());
    }

    #[test]
    fn id_string_and_bytes() {
        let id = id(42);
        let doc = Document::new(id);
        assert_eq!(doc.id(), id);
        assert_eq!(doc.id_bytes(), id.to_bytes());
        assert_eq!(doc.id_string(), format!("{NOVA_ID_PREFIX}{}", id.to_hex()));
        assert_eq!(doc.id_string().len(), NOVA_ID_PREFIX.len() + 32);
    }
}
