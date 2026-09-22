//! Query execution for parsed `NovaQL` queries.
//!
//! Phase 7 evaluates expressions and pipeline operators against a backend
//! abstraction. Phase 9 adds persistent index lifecycle and CRUD maintenance;
//! plan-selected index scans are introduced in Phase 10.

#![forbid(unsafe_code)]

mod backend;
mod evaluate;
mod executor;
mod indexed;

pub use backend::{ExecutionBackend, MemoryBackend};
pub use executor::{execute, execute_with_telemetry, ExecutionResult};
pub use indexed::IndexedBackend;
