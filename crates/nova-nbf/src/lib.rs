//! NBF — Nova Binary Format (v1) codec.
//!
//! NBF is NovaDB's deterministic binary representation of a [`Document`].
//! Streams contain a versioned header followed by one typed document value.
//! Decoding untrusted bytes is bounds-checked, depth-limited, and returns a
//! typed [`NovaError`] for every malformed input.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod varint;

use nova_core::document::Document;
use nova_core::error::{NovaError, Result};
use nova_core::nova_id::NovaId;
use nova_core::nova_timestamp::NovaTimestamp;
use nova_core::nova_value::NovaValue;
use nova_core::version::{FORMAT_VERSION, MAGIC};

use varint::{read_varint, write_varint};

/// Size of the NBF magic and version header in bytes.
pub const HEADER_LEN: usize = 8;

/// Maximum number of nested arrays and documents accepted by the codec.
pub const MAX_NESTING_DEPTH: usize = 254;

/// Maximum number of values or fields in one collection.
const MAX_COLLECTION_LENGTH: u64 = 1 << 20;

/// NBF v1 type tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Tag {
    /// Explicit null.
    Null = 0x00,
    /// Boolean false.
    False = 0x01,
    /// Boolean true.
    True = 0x02,
    /// Signed 64-bit integer.
    Int64 = 0x03,
    /// IEEE-754 64-bit float.
    Float64 = 0x04,
    /// UTF-8 string.
    String = 0x05,
    /// Ordered array.
    Array = 0x06,
    /// Document with an id and sorted fields.
    Document = 0x07,
    /// Signed Unix timestamp in milliseconds.
    Timestamp = 0x08,
    /// Raw 16-byte NovaId.
    NovaId = 0x09,
}

impl Tag {
    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x00 => Some(Self::Null),
            0x01 => Some(Self::False),
            0x02 => Some(Self::True),
            0x03 => Some(Self::Int64),
            0x04 => Some(Self::Float64),
            0x05 => Some(Self::String),
            0x06 => Some(Self::Array),
            0x07 => Some(Self::Document),
            0x08 => Some(Self::Timestamp),
            0x09 => Some(Self::NovaId),
            _ => None,
        }
    }
}

/// Encodes `document` as a deterministic NBF v1 byte stream.
///
/// # Errors
///
/// Returns [`NovaError::InvalidArgument`] if the document exceeds the codec's
/// nesting or per-collection limits.
pub fn encode(document: &Document) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    output.extend_from_slice(&MAGIC);
    output.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    encode_document(document, &mut output, 0)?;
    Ok(output)
}

fn encode_document(document: &Document, output: &mut Vec<u8>, depth: usize) -> Result<()> {
    check_encode_depth(depth)?;
    check_encode_count(document.len())?;

    output.push(Tag::Document as u8);
    output.extend_from_slice(document.id().as_bytes());
    write_varint(document.len() as u64, output);
    for (key, value) in document.iter() {
        write_varint(key.len() as u64, output);
        output.extend_from_slice(key.as_bytes());
        encode_value(value, output, depth + 1)?;
    }
    Ok(())
}

fn encode_value(value: &NovaValue, output: &mut Vec<u8>, depth: usize) -> Result<()> {
    match value {
        NovaValue::Null => output.push(Tag::Null as u8),
        NovaValue::Boolean(false) => output.push(Tag::False as u8),
        NovaValue::Boolean(true) => output.push(Tag::True as u8),
        NovaValue::Int64(value) => {
            output.push(Tag::Int64 as u8);
            output.extend_from_slice(&value.to_be_bytes());
        }
        NovaValue::Float64(value) => {
            output.push(Tag::Float64 as u8);
            let bits = if value.is_nan() {
                f64::NAN.to_bits()
            } else {
                value.to_bits()
            };
            output.extend_from_slice(&bits.to_be_bytes());
        }
        NovaValue::String(value) => {
            output.push(Tag::String as u8);
            write_varint(value.len() as u64, output);
            output.extend_from_slice(value.as_bytes());
        }
        NovaValue::Array(values) => {
            check_encode_depth(depth)?;
            check_encode_count(values.len())?;
            output.push(Tag::Array as u8);
            write_varint(values.len() as u64, output);
            for value in values {
                encode_value(value, output, depth + 1)?;
            }
        }
        NovaValue::Document(document) => encode_document(document, output, depth)?,
        NovaValue::Timestamp(value) => {
            output.push(Tag::Timestamp as u8);
            output.extend_from_slice(&value.millis().to_be_bytes());
        }
        NovaValue::NovaId(value) => {
            output.push(Tag::NovaId as u8);
            output.extend_from_slice(value.as_bytes());
        }
    }
    Ok(())
}

/// Decodes exactly one NBF v1 document from `bytes`.
///
/// # Errors
///
/// Returns [`NovaError::Unsupported`] for an unknown format version and
/// [`NovaError::Corruption`] for all malformed, truncated, non-canonical, or
/// trailing input.
pub fn decode(bytes: &[u8]) -> Result<Document> {
    let mut decoder = Decoder::new(bytes);
    decoder.read_header()?;
    let document = decoder.read_document(0, true)?;
    if decoder.remaining() != 0 {
        return Err(corruption("trailing bytes after root document"));
    }
    Ok(document)
}

struct Decoder<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> Decoder<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self { input, position: 0 }
    }

    const fn remaining(&self) -> usize {
        self.input.len() - self.position
    }

    fn read_header(&mut self) -> Result<()> {
        if self.take(MAGIC.len())? != MAGIC {
            return Err(corruption("bad NBF magic"));
        }
        let version = u32::from_be_bytes(self.take_array()?);
        if version != FORMAT_VERSION {
            return Err(NovaError::Unsupported(format!(
                "unsupported NBF format version {version}"
            )));
        }
        Ok(())
    }

    fn read_document(&mut self, depth: usize, read_tag: bool) -> Result<Document> {
        check_decode_depth(depth)?;
        if read_tag && self.read_byte()? != Tag::Document as u8 {
            return Err(corruption("root or nested value is not a document"));
        }

        let id =
            NovaId::from_bytes(self.take(16)?).map_err(|_| corruption("invalid NovaId payload"))?;
        let field_count = self.read_count("document field count")?;
        let mut document = Document::new(id);
        for _ in 0..field_count {
            let key = self.read_string_payload("field key")?;
            let value = self.read_value(depth + 1)?;
            if document.insert(key, value).is_some() {
                return Err(corruption("duplicate document field"));
            }
        }
        Ok(document)
    }

    fn read_value(&mut self, depth: usize) -> Result<NovaValue> {
        let tag_byte = self.read_byte()?;
        let tag = Tag::from_byte(tag_byte)
            .ok_or_else(|| corruption(format!("unknown NBF tag 0x{tag_byte:02x}")))?;
        match tag {
            Tag::Null => Ok(NovaValue::Null),
            Tag::False => Ok(NovaValue::Boolean(false)),
            Tag::True => Ok(NovaValue::Boolean(true)),
            Tag::Int64 => Ok(NovaValue::Int64(i64::from_be_bytes(self.take_array()?))),
            Tag::Float64 => Ok(NovaValue::Float64(f64::from_bits(u64::from_be_bytes(
                self.take_array()?,
            )))),
            Tag::String => Ok(NovaValue::String(self.read_string_payload("string")?)),
            Tag::Array => {
                check_decode_depth(depth)?;
                let count = self.read_count("array count")?;
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.read_value(depth + 1)?);
                }
                Ok(NovaValue::Array(values))
            }
            Tag::Document => Ok(NovaValue::Document(self.read_document(depth, false)?)),
            Tag::Timestamp => Ok(NovaValue::Timestamp(NovaTimestamp::from_millis(
                i64::from_be_bytes(self.take_array()?),
            ))),
            Tag::NovaId => NovaId::from_bytes(self.take(16)?)
                .map(NovaValue::NovaId)
                .map_err(|_| corruption("invalid NovaId payload")),
        }
    }

    fn read_string_payload(&mut self, label: &str) -> Result<String> {
        let length = self.read_length(label)?;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| corruption(format!("{label} is not valid UTF-8")))
    }

    fn read_length(&mut self, label: &str) -> Result<usize> {
        let value = self.read_varint_value()?;
        let length = usize::try_from(value)
            .map_err(|_| corruption(format!("{label} length does not fit usize")))?;
        if length > self.remaining() {
            return Err(corruption(format!(
                "declared {label} length exceeds remaining input"
            )));
        }
        Ok(length)
    }

    fn read_count(&mut self, label: &str) -> Result<usize> {
        let value = self.read_varint_value()?;
        if value > MAX_COLLECTION_LENGTH {
            return Err(corruption(format!("declared {label} exceeds limit")));
        }
        let count = usize::try_from(value)
            .map_err(|_| corruption(format!("declared {label} does not fit usize")))?;
        if count > self.remaining() {
            return Err(corruption(format!(
                "declared {label} exceeds remaining input"
            )));
        }
        Ok(count)
    }

    fn read_varint_value(&mut self) -> Result<u64> {
        let (value, consumed) = read_varint(&self.input[self.position..])?;
        self.position += consumed;
        Ok(value)
    }

    fn read_byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.input.len())
            .ok_or_else(|| corruption("unexpected end of NBF input"))?;
        let bytes = &self.input[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn take_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| corruption("unexpected end of NBF input"))
    }
}

fn check_encode_depth(depth: usize) -> Result<()> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(NovaError::InvalidArgument(format!(
            "NBF nesting exceeds {MAX_NESTING_DEPTH}"
        )));
    }
    Ok(())
}

fn check_decode_depth(depth: usize) -> Result<()> {
    if depth >= MAX_NESTING_DEPTH {
        return Err(corruption(format!(
            "NBF nesting exceeds {MAX_NESTING_DEPTH}"
        )));
    }
    Ok(())
}

fn check_encode_count(count: usize) -> Result<()> {
    if count as u64 > MAX_COLLECTION_LENGTH {
        return Err(NovaError::InvalidArgument(format!(
            "NBF collection exceeds {MAX_COLLECTION_LENGTH} elements"
        )));
    }
    Ok(())
}

fn corruption(message: impl Into<String>) -> NovaError {
    NovaError::Corruption(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(seed: u8) -> NovaId {
        NovaId::new(u64::from(seed), [seed; 8])
    }

    fn encoded_prefix() -> Vec<u8> {
        let mut bytes = Vec::from(MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
        bytes
    }

    fn sample_document() -> Document {
        let mut nested = Document::new(id(2));
        nested.insert("city", "Bhubaneswar".into());

        let mut document = Document::new(id(1));
        document.insert(
            "array",
            NovaValue::Array(vec![NovaValue::Null, true.into()]),
        );
        document.insert("false", false.into());
        document.insert("float", (-42.5).into());
        document.insert("id", id(3).into());
        document.insert("int", i64::MIN.into());
        document.insert("nested", nested.into());
        document.insert("null", NovaValue::Null);
        document.insert("string", "नमस्ते 🌍".into());
        document.insert("timestamp", NovaTimestamp::from_millis(-1).into());
        document.insert("true", true.into());
        document
    }

    fn assert_corruption(bytes: &[u8]) {
        assert!(matches!(decode(bytes), Err(NovaError::Corruption(_))));
    }

    #[test]
    fn every_value_variant_round_trips() {
        let document = sample_document();
        let bytes = encode(&document).unwrap();
        assert_eq!(decode(&bytes).unwrap(), document);
        assert_eq!(encode(&decode(&bytes).unwrap()).unwrap(), bytes);
    }

    #[test]
    fn empty_document_round_trips() {
        let document = Document::new(id(0));
        assert_eq!(decode(&encode(&document).unwrap()).unwrap(), document);
    }

    #[test]
    fn floats_preserve_special_values_and_canonicalize_nan() {
        let mut document = Document::new(id(1));
        document.insert("negative_infinity", f64::NEG_INFINITY.into());
        document.insert("positive_infinity", f64::INFINITY.into());
        document.insert("negative_zero", (-0.0_f64).into());
        document.insert(
            "nan",
            NovaValue::Float64(f64::from_bits(0x7ff0_0000_0000_0001)),
        );

        let decoded = decode(&encode(&document).unwrap()).unwrap();
        assert!(decoded.get("nan").unwrap().as_float64().unwrap().is_nan());
        assert_eq!(
            decoded
                .get("negative_zero")
                .unwrap()
                .as_float64()
                .unwrap()
                .to_bits(),
            (-0.0_f64).to_bits()
        );
        assert_eq!(
            decoded.get("negative_infinity"),
            Some(&f64::NEG_INFINITY.into())
        );
        assert_eq!(
            decoded.get("positive_infinity"),
            Some(&f64::INFINITY.into())
        );

        let bytes = encode(&document).unwrap();
        let nan_bits = f64::NAN.to_bits().to_be_bytes();
        assert!(bytes
            .windows(nan_bits.len())
            .any(|window| window == nan_bits));
    }

    #[test]
    fn encoding_is_independent_of_insertion_order() {
        let mut first = Document::new(id(1));
        first.insert("z", 1_i64.into());
        first.insert("a", 2_i64.into());
        let mut second = Document::new(id(1));
        second.insert("a", 2_i64.into());
        second.insert("z", 1_i64.into());
        assert_eq!(encode(&first).unwrap(), encode(&second).unwrap());
    }

    #[test]
    fn rejects_bad_magic_version_root_tag_and_trailing_bytes() {
        let valid = encode(&Document::new(id(1))).unwrap();
        let mut bad_magic = valid.clone();
        bad_magic[0] ^= 1;
        assert_corruption(&bad_magic);

        let mut bad_version = valid.clone();
        bad_version[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_be_bytes());
        assert!(matches!(
            decode(&bad_version),
            Err(NovaError::Unsupported(_))
        ));

        let mut bad_root = valid.clone();
        bad_root[HEADER_LEN] = Tag::Null as u8;
        assert_corruption(&bad_root);

        let mut trailing = valid;
        trailing.push(0);
        assert_corruption(&trailing);
    }

    #[test]
    fn every_truncation_boundary_is_an_error_and_never_panics() {
        let bytes = encode(&sample_document()).unwrap();
        for end in 0..bytes.len() {
            let result = std::panic::catch_unwind(|| decode(&bytes[..end]));
            assert!(result.is_ok(), "decoder panicked at boundary {end}");
            assert!(
                result.unwrap().is_err(),
                "truncation {end} decoded successfully"
            );
        }
    }

    #[test]
    fn rejects_non_minimal_overlong_overflowing_and_truncated_varints() {
        for varint in [
            vec![0x80, 0x00],
            vec![0x80; 10],
            vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02],
            vec![0x80],
        ] {
            let mut bytes = encoded_prefix();
            bytes.push(Tag::Document as u8);
            bytes.extend_from_slice(id(1).as_bytes());
            bytes.extend_from_slice(&varint);
            assert_corruption(&bytes);
        }
    }

    #[test]
    fn rejects_unknown_tag_invalid_utf8_and_impossible_lengths_or_counts() {
        let mut unknown = encoded_prefix();
        unknown.push(Tag::Document as u8);
        unknown.extend_from_slice(id(1).as_bytes());
        unknown.push(1);
        unknown.push(1);
        unknown.push(b'x');
        unknown.push(0xff);
        assert_corruption(&unknown);

        let mut invalid_utf8 = encoded_prefix();
        invalid_utf8.push(Tag::Document as u8);
        invalid_utf8.extend_from_slice(id(1).as_bytes());
        invalid_utf8.push(1);
        invalid_utf8.push(1);
        invalid_utf8.push(0xff);
        invalid_utf8.push(Tag::Null as u8);
        assert_corruption(&invalid_utf8);

        let mut bad_count = encoded_prefix();
        bad_count.push(Tag::Document as u8);
        bad_count.extend_from_slice(id(1).as_bytes());
        write_varint(MAX_COLLECTION_LENGTH + 1, &mut bad_count);
        assert_corruption(&bad_count);

        let mut bad_length = encoded_prefix();
        bad_length.push(Tag::Document as u8);
        bad_length.extend_from_slice(id(1).as_bytes());
        bad_length.push(1);
        bad_length.push(100);
        assert_corruption(&bad_length);
    }

    #[test]
    fn rejects_duplicate_fields() {
        let mut bytes = encoded_prefix();
        bytes.push(Tag::Document as u8);
        bytes.extend_from_slice(id(1).as_bytes());
        bytes.push(2);
        for _ in 0..2 {
            bytes.push(1);
            bytes.push(b'x');
            bytes.push(Tag::Null as u8);
        }
        assert_corruption(&bytes);
    }

    #[test]
    fn rejects_depth_bombs_without_panicking() {
        let mut bytes = encoded_prefix();
        bytes.push(Tag::Document as u8);
        bytes.extend_from_slice(id(1).as_bytes());
        bytes.push(1);
        bytes.push(1);
        bytes.push(b'x');
        for _ in 0..MAX_NESTING_DEPTH {
            bytes.push(Tag::Array as u8);
            bytes.push(1);
        }
        bytes.push(Tag::Null as u8);

        let result = std::panic::catch_unwind(|| decode(&bytes));
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Err(NovaError::Corruption(_))));
    }
}
