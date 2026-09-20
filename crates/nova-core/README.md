# nova-core

Core infrastructure shared across NovaDB crates.

Implemented in Phase 0:

- `error` — `NovaError`, a structured error enum, and the `Result<T>` alias used
  at crate boundaries.
- `log` — idempotent `tracing`/`tracing-subscriber` startup helper.
- `version` — the on-disk format registry (`MAGIC = b"NOVA"`, `FORMAT_VERSION = 1`).

Implemented in Phase 1 (core data model):

- `nova_id` — `NovaId`, a 128-bit time-prefixed identifier (64-bit big-endian
  epoch milliseconds + 64-bit CSPRNG randomness), with hex `Display`/`FromStr`
  and byte-level `from_bytes`/`as_bytes` round-trips.
- `nova_timestamp` — `NovaTimestamp`, millisecond-precision timestamps with
  ISO-8601 formatting.
- `nova_value` — `NovaValue`, a typed `Null`/`Bool`/`Int64`/`Float64`/
  `String`/`Timestamp`/`NovaId`/`Document`/`Array` value.
- `document` — `Document`, a typed field map always carrying an implicit
  `_id`, with nested dotted-path lookup.

Not yet implemented (later phases):

- Typed `NovaQL` error codes with source spans (Phase 5+).
- NBF binary serialization of the data model (Phase 2, `nova-nbf`).