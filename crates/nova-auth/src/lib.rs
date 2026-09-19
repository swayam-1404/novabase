//! Authentication and authorization.
//!
//! READ_ONLY / READ_WRITE / ADMIN roles, hashed passwords (never plaintext),
//! and deterministic authorization fully separate from the `NIE` analytics layer.
//!
//! Implemented in later phases (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
