//! Page-based storage engine.
//!
//! Slotted pages, page read/write, checksums, free-space management, collections
//! and database metadata. Documents are stored as `NBF` bytes inside pages.
//!
//! Implemented in later phases (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
