# NovaDB Write-Ahead Log Format (v1)

The Phase 12 WAL is an append-only sequence of independently checksummed
records. Integers are unsigned big-endian.

## Record header

| Offset | Size | Field |
|--------|------|-------|
| 0 | 4 | ASCII magic `NVWL` |
| 4 | 2 | version `1` |
| 6 | 1 | operation tag |
| 7 | 1 | reserved zero |
| 8 | 4 | payload length |
| 12 | 8 | monotonically increasing LSN |
| 20 | 8 | transaction id |
| 28 | 4 | CRC-32/ISO-HDLC checksum, treated as zero while calculating |
| 32 | 4 | reserved zero |

Tags are begin (`0`), full-page write (`1`), commit (`2`), abort (`3`), and
checkpoint (`4`). Marker records have no payload. A page-write payload contains
an eight-byte page id followed by one complete 4096-byte encoded page; its id
must match the page header.

## Durability and recovery

Append writes records in LSN order. `flush` synchronizes the WAL file and moves
the reported durable LSN to the final appended record. Data-page users must
flush the relevant log record before flushing its dirty page (write-ahead
rule).

Open and recovery validate every complete record. An incomplete final header or
payload is a torn tail: open truncates it to the last complete record and
recovery ignores it. Bad magic, versions, reserved fields, lengths, LSN order,
checksums, page encodings, and page-id mismatches are typed errors, never
panics.

Recovery first identifies committed and aborted transaction ids, then replays
full-page after-images only for committed, non-aborted transactions in LSN
order. Missing target pages are allocated. Replacing complete pages makes redo
idempotent. Undo, isolation, and lock management are Phase 13 concerns.
