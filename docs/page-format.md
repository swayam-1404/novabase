# NovaDB Page Format (v1)

This document is the authoritative format for Phase 3 data pages. Page files
are sequences of fixed-size pages with no gaps or file-level prefix. Page `n`
starts at byte offset `n * 4096` and its header must contain page id `n`.

## Constants

- Page size: 4096 bytes.
- Header size: 32 bytes.
- Slot entry size: 4 bytes.
- Magic: ASCII `NVPG`.
- Format version: unsigned big-endian `u16` value `1`.
- Checksum: CRC-32/ISO-HDLC over all 4096 bytes while treating the checksum
  field itself as four zero bytes.

All multi-byte integers are unsigned and big-endian.

## Header

| Offset | Size | Field | Meaning |
|--------|------|-------|---------|
| 0 | 4 | magic | `NVPG` |
| 4 | 2 | version | page format version |
| 6 | 2 | header size | always 32 |
| 8 | 8 | page id | zero-based position in the page file |
| 16 | 4 | checksum | CRC-32 of the complete page |
| 20 | 2 | slot count | live plus deleted slots |
| 22 | 2 | live count | non-deleted slots |
| 24 | 2 | free start | byte after the slot directory |
| 26 | 2 | free end | first byte of compacted payload data |
| 28 | 4 | reserved | zero in v1 |

The free region is the half-open range `[free_start, free_end)`. The slot
directory grows upward from offset 32 and payload bytes grow downward from
offset 4096.

## Slot directory

Slots are stable, zero-based identifiers. Each four-byte entry contains a
two-byte payload offset followed by a two-byte payload length. Live payloads
must lie wholly in `[free_end, 4096)`, must not overlap, and collectively fill
that area without gaps. Zero-length records are valid.

A deleted slot uses offset `0xffff` and length `0`. Inserting a new record
reuses the lowest deleted slot before extending the directory. Serialization
compacts payload bytes but never renumbers slots.

## Validation and errors

Page decoding validates the exact input size, magic, version, header size,
checksum, reserved bytes, counts, free-space boundaries, every slot range,
payload overlap/gaps, and live count. Unknown versions return
`NovaError::Unsupported`; all malformed page bytes return
`NovaError::Corruption`. Decoding malformed input must never panic.

The page manager rejects page files whose length is not a multiple of 4096,
rejects reads/writes outside the allocated page count, and verifies that a
decoded page id matches its requested file position.
