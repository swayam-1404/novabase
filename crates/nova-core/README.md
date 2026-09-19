# nova-core

Core infrastructure shared across NovaDB crates.

Implemented in Phase 0:

- `error` — `NovaError`, a structured error enum, and the `Result<T>` alias used
  at crate boundaries.
- `log` — idempotent `tracing`/`tracing-subscriber` startup helper.
- `version` — the on-disk format registry (`MAGIC = b"NOVA"`, `FORMAT_VERSION = 1`).

Not yet implemented (later phases):

- `NovaValue`, `Document`, `NovaId` (Phase 1).
- Typed `NovaQL` error codes with source spans (Phase 5+).