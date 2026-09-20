# NBF — Nova Binary Format (v1)

This document is the authoritative specification for **NBF version 1**, the
deterministic binary encoding NovaDB uses for documents. NBF is *not* JSON
text; it is the document representation stored inside data pages.

## Guiding principles

- **Deterministic.** The same logical document always encodes to the same byte
  sequence. Field ordering is derived from the sorted key order of a
  [`Document`] (a `BTreeMap`), so encoding never depends on hash-seed or
  insertion order.
- **Versioned.** Every NBF stream starts with a fixed header that carries the
  format magic and the on-disk format version (see [`nova_core::version`]).
  Readers reject unknown magic/version combinations cleanly.
- **Safe on malformed input.** Every decode operation is total: it returns a
  typed [`NovaError`] rather than panicking, even for truncated, over-long or
  semantically corrupt byte sequences.
- **Bounded.** Decoding enforces an explicit nesting depth limit and validates
  every declared length/count against the remaining input before allocating or
  recursing.

## Wire layout

### 1. Header (8 bytes, always present)

| offset | size | field            | value                                  |
|--------|------|------------------|----------------------------------------|
| 0      | 4    | magic            | `b"NOVA"`                              |
| 4      | 4    | format version   | `u32` big-endian = 1                   |

The magic and version are re-exported from `nova_core::version` so the
registry is a single source of truth. A stream whose magic or version does not
match is rejected with a typed error before any value is read.

### 2. Root value

Immediately after the header, NBF stores exactly **one** typed value (tag + a
`Document` payload). The root of an encoded NovaDB document is always a
[`Document`], and the document's [`NovaId`] is stored as part of the payload so
`decode(encode(d)) == d` preserves the id.

### 3. Type tags (1 byte)

| tag | type         | payload                                            |
|-----|--------------|----------------------------------------------------|
| 0x00 | Null        | —                                                  |
| 0x01 | False       | —                                                  |
| 0x02 | True        | —                                                  |
| 0x03 | Int64       | 8 bytes, big-endian two's-complement              |
| 0x04 | Float64     | 8 bytes, big-endian IEEE-754 bit pattern          |
| 0x05 | String      | varint **byte length** + UTF-8 bytes              |
| 0x06 | Array       | varint **count** + count × (tag + value)          |
| 0x07 | Document    | 16-byte [`NovaId`] + varint **field count** +      |
|      |             | field count × (string key + value)                |
| 0x08 | Timestamp   | 8 bytes, big-endian `i64` (unix epoch millisecond) |
| 0x09 | NovaId      | 16 raw bytes                                      |

The root tag **must** be `0x07` (Document); decoding any other root tag is an
error. Inside documents, field keys are stored as a length-prefixed UTF-8
string (varint byte length + bytes) followed by the field's typed value. Keys
are written in the sorted key order of the source document, making encoding
reproducible.

### 4. Varint (LEB128, unsigned)

Lengths and counts use unsigned LEB128, little-endian base-128 groups:

- Low 7 bits of each byte carry data; the high bit marks continuation.
- Values 0..=127 encode in a single byte, which covers the common short case.
- Encoders emit the **minimal** form (no redundant leading zero groups).
- Decoders accept at most 10 bytes (a full `u64`) and reject:
  - streams longer than 10 bytes;
  - non-minimal encodings (a trailing `0x00` group that a minimal form would
    omit);
  - any varint that would overflow the 64-bit accumulator.

## Decoder invariants (malformed-data protection)

The decoder below guarantees, for *every* byte string, either a successful
decode or a typed [`NovaError`] — never a panic:

1. **Bounds-first reads.** Every byte read is checked against the remaining
   input before dereferencing.
2. **Declared-length checks.** A string's byte length, an array's count and a
   document's field count are all validated to fit within the remaining input
   before the corresponding allocation/recursion. A count greater than the
   remaining bytes is rejected immediately (`Corruption`).
3. **Depth limit.** Recursive document/array nesting is capped at
   [`MAX_NESTING_DEPTH`]. Input exceeding the limit returns `Corruption`
   instead of overflowing the stack.
4. **Exact consumption.** The decoder consumes the entire input for a top-level
   document; trailing bytes are rejected (`Corruption`), preventing
   lenient-parser ambiguity.
5. **Canonical forms.** Over-long varints and non-minimal length prefixes are
   rejected, so a given byte sequence decodes to exactly one value
   (no aliasing).
6. **Float NaN canonicalisation.** Non-canonical NaN payloads on **encode** are
   written as the platform's single canonical quiet-NaN bit pattern, so
   encoding stays deterministic; the corresponding float round-trips
   bit-for-bit for every non-NaN value.

## Error mapping

| condition                                        | NovaError variant           |
|--------------------------------------------------|-----------------------------|
| malformed or truncated bytes, bad tag, bad magic | `NovaError::Corruption`     |
| unknown format version                           | `NovaError::Unsupported`    |
| nesting deeper than the limit                    | `NovaError::Corruption`     |
| trailing bytes after the top-level document      | `NovaError::Corruption`     |

## API

```rust
pub fn encode(document: &Document) -> Result<Vec<u8>>
pub fn decode(bytes: &[u8]) -> Result<Document>
```

`Result<T>` is [`nova_core::error::Result<T>`], so every failure is typed.

## Round-trip guarantee

For any `Document` `d`:

```text
decode(encode(d)) == d
```

This is exercised by round-trip tests covering every [`NovaValue`] variant,
nested documents, arrays, Unicode text, negative integers, finite floats,
timestamps with negative and positive millisecond values, and explicit
[`NovaId`] values.

[`Document`]: ../crates/nova-core/src/document.rs
[`NovaValue`]: ../crates/nova-core/src/nova_value.rs
[`NovaId`]: ../crates/nova-core/src/nova_id.rs
[`NovaError`]: ../crates/nova-core/src/error.rs
[`nova_core::version`]: ../crates/nova-core/src/version.rs
[`MAX_NESTING_DEPTH`]: ../crates/nova-nbf/src/lib.rs
