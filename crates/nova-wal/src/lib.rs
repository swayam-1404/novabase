//! Write-ahead log (`WAL`) and crash recovery.
//!
//! Log records (`LSN`, transaction ID, operation), `WAL` append/flush, checkpoints
//! and startup recovery. Recovery guarantees are documented explicitly and are
//! enforced before dirty data pages are treated as durable.
//!
//! Implemented in a later phase (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
