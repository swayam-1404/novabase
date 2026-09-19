//! Core infrastructure shared across NovaDB crates.
//!
//! This crate owns NovaDB's cross-cutting foundations:
//!
//! - [`error`] — structured error and result conventions used at crate boundaries.
//! - [`log`] — process-wide `tracing` subscriber setup.
//! - [`version`] — the on-disk format version registry.
//! - [`nova_id`] / [`nova_timestamp`] / [`nova_value`] / [`document`] — the core
//!   data model.
//!
//! # Data model
//!
//! NovaDB stores JSON-like documents (see [`Document`]). Every document carries
//! a globally unique [`NovaId`]. Values are typed [`NovaValue`]s: `Null`,
//! `Boolean`, `Int64`, `Float64`, `String`, `Array`, nested `Document`,
//! `Timestamp`, or `NovaId`. The type system is designed so new types (e.g. a
//! future `Vector`) can be added without breaking the on-disk format.
//!
//! There is no implicit conversion between value types: comparison semantics
//! are defined explicitly and later enforced by the query layer.

#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub mod document;
pub mod error;
pub mod log;
pub mod nova_id;
pub mod nova_timestamp;
pub mod nova_value;
pub mod version;

pub use document::Document;
pub use nova_id::NovaId;
pub use nova_timestamp::NovaTimestamp;
pub use nova_value::NovaValue;
