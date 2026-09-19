//! `NovaQL` front end.
//!
//! Typed tokens, a strongly typed AST, precise error reporting with source
//! spans, and a semantic validator. Parser and executor stay independent.
//!
//! Implemented in later phases (see `docs/architecture.md`). This file locks in
//! the crate as an architectural unit so the workspace builds from Phase 0.

#![forbid(unsafe_code)]
