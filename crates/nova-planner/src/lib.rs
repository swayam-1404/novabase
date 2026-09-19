//! Query planner.
//!
//! Deterministic plan generation choosing between `IndexScan` and
//! `CollectionScan`, plus inspectable execution plans (`nova.explain`).
//!
//! Implemented in a later phase (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
