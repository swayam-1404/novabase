//! NovaDB command-line interface.
//!
//! Interactive `NovaQL` shell and administration commands
//! (`nova status`, `nova analyze`, `nova recommendations`, ...).
//!
//! Functional CLI commands are implemented in later phases
//! (see `docs/architecture.md`); the binary currently prints version info.

#![forbid(unsafe_code)]

fn main() {
    println!("NovaDB {} (scaffolding)", env!("CARGO_PKG_VERSION"));
}
