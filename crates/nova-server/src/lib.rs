//! NovaDB server.
//!
//! Single-node server exposing a simple versioned protocol: multiple client
//! connections, bounded resources, request IDs, timeouts, structured errors
//! and graceful shutdown. No clustering, sharding or replication (out of scope).
//!
//! Implemented in a later phase (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
