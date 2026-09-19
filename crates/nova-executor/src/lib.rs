//! Query executor.
//!
//! Stream-oriented operators (`CollectionScan`, `IndexScan`, `Filter`, `Sort`,
//! `Limit`) working page -> record -> decode -> predicate -> result, with early
//! termination support for `.limit N`.
//!
//! Implemented in a later phase (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
