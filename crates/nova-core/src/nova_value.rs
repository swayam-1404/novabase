//! `NovaValue` — NovaDB's typed document value.

use crate::document::Document;
use crate::nova_id::NovaId;
use crate::nova_timestamp::NovaTimestamp;

/// A typed value that can be stored inside a NovaDB [`Document`].
///
/// NovaDB's type system is JSON-like but explicit: there is no implicit
/// conversion between numeric or string variants (comparison semantics are
/// defined in the query layer, not here). The set of variants is designed so
/// that future types (e.g. a `Vector`) can be added to the model and to the
/// binary format without breaking existing on-disk data.
#[derive(Debug, Clone, PartialEq)]
pub enum NovaValue {
    /// The explicit null value.
    Null,
    /// A boolean.
    Boolean(bool),
    /// A 64-bit signed integer.
    Int64(i64),
    /// A 64-bit IEEE-754 float.
    Float64(f64),
    /// UTF-8 text.
    String(String),
    /// An ordered list of values.
    Array(Vec<NovaValue>),
    /// A nested document.
    Document(Document),
    /// A [`NovaTimestamp`]: milliseconds since the unix epoch.
    Timestamp(NovaTimestamp),
    /// A NovaDB document identifier.
    NovaId(NovaId),
}

impl NovaValue {
    /// Returns the human-readable name of the variant without reading it.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Boolean(_) => "boolean",
            Self::Int64(_) => "int64",
            Self::Float64(_) => "float64",
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Document(_) => "document",
            Self::Timestamp(_) => "timestamp",
            Self::NovaId(_) => "nova_id",
        }
    }

    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Borrows the boolean if this value is a boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Borrows the integer if this value is an integer.
    #[must_use]
    pub fn as_int64(&self) -> Option<i64> {
        match self {
            Self::Int64(v) => Some(*v),
            _ => None,
        }
    }

    /// Borrows the float if this value is a float.
    #[must_use]
    pub fn as_float64(&self) -> Option<f64> {
        match self {
            Self::Float64(v) => Some(*v),
            _ => None,
        }
    }

    /// Borrows the string if this value is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// Borrows the array if this value is an array.
    #[must_use]
    pub fn as_array(&self) -> Option<&[NovaValue]> {
        match self {
            Self::Array(v) => Some(v),
            _ => None,
        }
    }

    /// Borrows the nested document if this value is a document.
    #[must_use]
    pub fn as_document(&self) -> Option<&Document> {
        match self {
            Self::Document(d) => Some(d),
            _ => None,
        }
    }

    /// Borrows the timestamp if this value is a timestamp.
    #[must_use]
    pub fn as_timestamp(&self) -> Option<NovaTimestamp> {
        match self {
            Self::Timestamp(t) => Some(*t),
            _ => None,
        }
    }

    /// Borrows the NovaId if this value is a NovaId.
    #[must_use]
    pub fn as_nova_id(&self) -> Option<NovaId> {
        match self {
            Self::NovaId(id) => Some(*id),
            _ => None,
        }
    }
}

impl From<bool> for NovaValue {
    fn from(v: bool) -> Self {
        Self::Boolean(v)
    }
}

impl From<i64> for NovaValue {
    fn from(v: i64) -> Self {
        Self::Int64(v)
    }
}

impl From<f64> for NovaValue {
    fn from(v: f64) -> Self {
        Self::Float64(v)
    }
}

impl From<String> for NovaValue {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<&str> for NovaValue {
    fn from(v: &str) -> Self {
        Self::String(v.to_owned())
    }
}

impl From<Vec<NovaValue>> for NovaValue {
    fn from(v: Vec<NovaValue>) -> Self {
        Self::Array(v)
    }
}

impl From<Document> for NovaValue {
    fn from(v: Document) -> Self {
        Self::Document(v)
    }
}

impl From<NovaTimestamp> for NovaValue {
    fn from(v: NovaTimestamp) -> Self {
        Self::Timestamp(v)
    }
}

impl From<NovaId> for NovaValue {
    fn from(v: NovaId) -> Self {
        Self::NovaId(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_document() -> Document {
        let id = NovaId::new(1, [0u8; 8]);
        let mut doc = Document::new(id);
        doc.insert("name", "Swayam".into());
        doc.insert("cgpa", 8.7.into());
        doc
    }

    #[test]
    fn every_variant_reports_its_type_name() {
        let cases = [
            (NovaValue::Null, "null"),
            (NovaValue::Boolean(true), "boolean"),
            (NovaValue::Int64(7), "int64"),
            (NovaValue::Float64(2.5), "float64"),
            (NovaValue::String("x".to_owned()), "string"),
            (NovaValue::Array(vec![]), "array"),
            (
                NovaValue::Document(Document::new(NovaId::new(0, [0; 8]))),
                "document",
            ),
            (
                NovaValue::Timestamp(NovaTimestamp::from_millis(0)),
                "timestamp",
            ),
            (NovaValue::NovaId(NovaId::new(0, [0; 8])), "nova_id"),
        ];
        for (value, expected) in cases {
            assert_eq!(value.type_name(), expected);
        }
    }

    #[test]
    fn accessors_return_values_when_type_matches() {
        let int = NovaValue::Int64(99);
        assert_eq!(int.as_int64(), Some(99));
        assert_eq!(int.as_float64(), None);
        assert_eq!(int.as_str(), None);

        let s = NovaValue::from("hi");
        assert_eq!(s.as_str(), Some("hi"));

        let arr = NovaValue::Array(vec![NovaValue::Int64(1), NovaValue::Int64(2)]);
        assert_eq!(
            arr.as_array(),
            Some(&[NovaValue::Int64(1), NovaValue::Int64(2)][..])
        );

        let doc = NovaValue::Document(sample_document());
        assert!(doc.as_document().is_some());
        assert_eq!(
            doc.as_document().unwrap().get("name"),
            Some(&NovaValue::from("Swayam"))
        );
    }

    #[test]
    fn from_conversions_build_the_expected_variants() {
        assert_eq!(NovaValue::from(1i64), NovaValue::Int64(1));
        assert_eq!(NovaValue::from(1.5f64), NovaValue::Float64(1.5));
        assert_eq!(NovaValue::from(true), NovaValue::Boolean(true));
        assert_eq!(NovaValue::from("s"), NovaValue::String("s".to_owned()));
        let ts = NovaTimestamp::from_millis(-5);
        assert_eq!(NovaValue::from(ts), NovaValue::Timestamp(ts));
        let id = NovaId::new(2, [9; 8]);
        assert_eq!(NovaValue::from(id), NovaValue::NovaId(id));
    }

    #[test]
    fn nested_values_support_deep_equality() {
        let inner = NovaValue::Document(sample_document());
        let outer = NovaValue::Array(vec![inner.clone(), NovaValue::Null]);
        assert_eq!(NovaValue::Array(vec![inner, NovaValue::Null]), outer);
        assert_ne!(NovaValue::Array(vec![NovaValue::Int64(1)]), outer);
    }

    #[test]
    fn int_and_float_are_distinct_types() {
        assert_ne!(NovaValue::Int64(1), NovaValue::Float64(1.0));
    }
}
