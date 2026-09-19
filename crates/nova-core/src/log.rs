//! Logging configuration for NovaDB.
//!
//! NovaDB uses the `tracing` crate for structured, span-aware logging. The
//! process-wide subscriber is configured once at startup; all other crates log
//! through [`tracing::info`], [`tracing::debug`], etc. and never manage the
//! subscriber themselves.

use tracing_subscriber::EnvFilter;

/// Default filter used when `RUST_LOG` is not set.
pub const DEFAULT_LOG_FILTER: &str = "info";

/// Initializes the process-wide `tracing` subscriber.
///
/// Filter directives are read from the `RUST_LOG` environment variable and fall
/// back to [`DEFAULT_LOG_FILTER`] (overridable via `filter`) when unset. This is
/// idempotent: if a subscriber is already installed, later calls are ignored.
pub fn init_logging(filter: Option<&str>) {
    let default = filter.unwrap_or(DEFAULT_LOG_FILTER).to_owned();
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_logging_is_idempotent() {
        // Calling twice (e.g. from multiple test threads) must not panic.
        init_logging(Some("nova_core=debug"));
        init_logging(Some("nova_core=trace"));
        tracing::debug!("logging initialized");
    }
}
