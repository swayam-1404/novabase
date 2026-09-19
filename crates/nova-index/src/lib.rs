//! Indexes.
//!
//! Persistent B+ tree mapping keys (`Int64`, `Float64`, `String`, `NovaId`) to
//! document `RecordPointer`s / `NovaId`s, plus index creation/drop/verification.
//!
//! Implemented in a later phase (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
