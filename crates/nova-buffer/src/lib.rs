//! Bounded buffer pool.
//!
//! Caches pages in memory (LRU/Clock replacement), tracks hits/misses/
//! evictions/dirty pages, and never allows unbounded memory growth.
//!
//! Implemented in a later phase (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
