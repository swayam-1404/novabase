//! Cross-crate hardening tests for NovaDB.

#![forbid(unsafe_code)]

#[cfg(test)]
#[path = "concurrency/telemetry.rs"]
mod telemetry;

#[cfg(test)]
#[path = "fuzz/totality.rs"]
mod totality;

#[cfg(test)]
#[path = "integration/end_to_end.rs"]
mod end_to_end;
