//! Transactions and locking.
//!
//! Deliberately limited transaction support (begin/commit/rollback) with
//! collection/database-level locking: multiple readers, synchronized writers.
//! Exact isolation semantics are documented, not assumed.
//!
//! Implemented in a later phase (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
