# NovaDB Server Protocol (Phase 14)

NovaDB uses a small binary TCP protocol owned by `nova-server`. Each connection
carries one request and one response. Phase 15 advances the protocol to v2 by
adding authentication to the request envelope. Integers are unsigned
big-endian and payload text is UTF-8.

## Frames

Both frames have a 24-byte header: four-byte magic (`NVRQ` for requests,
`NVRS` for responses), `u16` version `2`, one status byte, one reserved zero
byte, `u64` request id, `u32` payload length, and a four-byte
CRC-32/ISO-HDLC checksum. The checksum covers header and payload while treating
its own bytes as zero.

Request status is zero. Its payload starts with a `u32` token byte length and
the UTF-8 bearer token, followed by NovaQL; length `0xffffffff` represents an
explicitly anonymous request. A successful response status
is zero and contains the execution result text. Error responses use stable
codes: protocol `1`, parse `2`, execution `3`, busy `4`, internal `5`,
authentication `6`, and authorization `7`.
Responses copy the trusted request id so clients can correlate work.

## Resource and lifecycle guarantees

Configuration requires positive connection and payload limits. Each accepted
socket receives read and write deadlines. Connections above the configured cap
receive a structured busy response. Accepted clients run in separate workers;
backend access is serialized because Phase 14 exposes one single-node backend,
not a distributed or multi-writer storage claim.

`bind_authenticated` requires a valid session and checks the parsed command's
permission before locking or invoking the backend. Reads require `Read`, CRUD
mutations require `Write`, and collection/index DDL requires `Manage`.
The original `bind` constructor remains an explicitly anonymous embedding mode.

The listener polls a cloneable shutdown flag. After shutdown it stops accepting
connections and joins every accepted worker before returning. Bad magic,
versions, reserved bytes, lengths, checksums, UTF-8, parser input, and execution
failures become structured responses or typed server errors and never panic.
