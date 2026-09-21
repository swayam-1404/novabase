# NovaDB B+ Tree Snapshot Format (v1)

This document is the authoritative Phase 8 format for files owned by
`nova-index`. A file contains one complete B+ tree snapshot. Phase 8 provides
explicit synchronized persistence; WAL-coordinated crash recovery is added in
Phase 12.

## Tree semantics

- Supported keys are `Int64`, `Float64`, UTF-8 strings, and `NovaId`.
- Key variants have a deterministic cross-type order in that sequence.
- Floats use IEEE-754 total order. All NaNs are canonicalized; negative and
  positive zero remain distinct.
- Each key maps to a strictly ordered, duplicate-free list of document ids.
- Leaf nodes carry a forward link for ordered range scans.
- Internal separators are inclusive lower bounds for their right child.
- Inserts recursively split overflowing leaves and internal nodes and create a
  new root when necessary.
- Deletes remove exact `(key, document-id)` pairs. Empty nodes remain valid;
  compaction and merge policy are outside the Phase 8 contract.

## Header

All integer fields are unsigned big-endian unless stated otherwise.

| Offset | Size | Field | Meaning |
|--------|------|-------|---------|
| 0 | 4 | magic | ASCII `NVIX` |
| 4 | 2 | version | `1` |
| 6 | 2 | header size | `40` |
| 8 | 4 | max keys | odd node capacity, at least 3 |
| 12 | 4 | reserved | zero |
| 16 | 8 | root | root node number |
| 24 | 8 | node count | number of serialized nodes |
| 32 | 4 | checksum | CRC-32/ISO-HDLC of the entire file with this field zeroed |
| 36 | 4 | reserved | zero |

Nodes follow in node-number order. Every persisted node must be reachable once
from the root.

## Nodes and keys

A leaf uses tag `0`, followed by an eight-byte next-leaf node number (all ones
means none), a four-byte entry count, and its entries. Each entry contains a
key, a four-byte id count, and that many 16-byte NovaIds.

An internal node uses tag `1`, followed by a four-byte key count, the keys, a
four-byte child count, and that many eight-byte node numbers. Child count is
always key count plus one.

Keys use a one-byte tag and payload:

| Tag | Type | Payload |
|-----|------|---------|
| 0 | Int64 | 8-byte signed two's-complement integer |
| 1 | Float64 | 8-byte IEEE-754 bit pattern |
| 2 | String | 4-byte byte length, then UTF-8 bytes |
| 3 | NovaId | 16 raw bytes |

## Validation

Opening a snapshot validates bounds before every read, magic, version, header
and reserved fields, checksum, counts before allocation, node/key tags, UTF-8,
root and child references, cycles, reachability, capacities, strict key/id
ordering, separator bounds, equal leaf depth, and complete ordered leaf links.
Malformed input returns a typed `NovaError` and never panics. Unknown format
versions return `NovaError::Unsupported`.

## Index catalog metadata (Phase 9)

Each named index has a sibling `<name>.meta` file. It contains ASCII magic
`NVIM`, a big-endian `u16` version (`1`), length-prefixed UTF-8 index and
collection names, a segment count, each length-prefixed field-path segment,
and a trailing CRC-32/ISO-HDLC checksum over all preceding bytes.

Catalog open validates every metadata file and its corresponding tree before
making the catalog available. Index creation validates all existing documents,
backfills the tree, synchronizes it, then persists metadata. Missing and null
fields are omitted. Arrays, documents, booleans, and timestamps are rejected
as unindexable rather than coerced.

Format v1 defines one field path per index. NovaQL parses multi-field index DDL
for forward compatibility, but execution returns `NovaError::Unsupported`
until a composite-key byte ordering is specified. CRUD maintenance validates
values before the backend mutation and then synchronizes affected trees.
Atomic coordination of data, metadata, and index writes requires the WAL and
transaction phases; Phase 9 does not claim crash atomicity.
