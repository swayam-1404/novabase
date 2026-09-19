//! Nova Intelligence Engine (`NIE`).
//!
//! Analytical layer over query telemetry. `NIE` recommends physical-design
//! actions (index creation) with explainable evidence. `NIE` is never involved in
//! transaction correctness, recovery, authorization, or durability decisions.
//!
//! Implemented in later phases (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
