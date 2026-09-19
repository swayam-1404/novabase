//! Nova Binary Format (`NBF`).
//!
//! Deterministic binary encoding and decoding of NovaDB documents. `NBF` is the
//! document representation stored inside data pages; it is not JSON text.
//!
//! Implemented in a later phase (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
