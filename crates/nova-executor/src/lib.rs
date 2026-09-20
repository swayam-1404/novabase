//! Collection-scan execution for parsed `NovaQL` queries.
//!
//! Phase 7 evaluates expressions and pipeline operators against a backend
//! abstraction. Index scans and planning are added in later roadmap phases.

#![forbid(unsafe_code)]

mod backend;
mod evaluate;
mod executor;

pub use backend::{ExecutionBackend, MemoryBackend};
pub use executor::{execute, ExecutionResult};
