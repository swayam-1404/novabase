//! Structured error conventions for NovaDB.
//!
//! Every NovaDB crate returns typed errors. Crates may define narrow local
//! error types, but the outermost service boundaries surface [`NovaError`].
//! Errors are always structured: never silently swallowed, and never converted
//! into panics, even for malformed external input.

use std::io;

/// Result alias used across NovaDB.
pub type Result<T> = std::result::Result<T, NovaError>;

/// Top-level structured NovaDB error.
#[derive(Debug, thiserror::Error)]
pub enum NovaError {
    /// Unexpected internal state; indicates a bug rather than bad input.
    #[error("internal error: {0}")]
    Internal(String),

    /// Invalid argument / malformed input from an external caller.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Referenced resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// A resource cannot be created because it already exists.
    #[error("already exists: {0}")]
    AlreadyExists(String),

    /// The requested operation is not supported.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// The storage layer rejected an operation.
    #[error("storage error: {0}")]
    Storage(String),

    /// Corruption was detected in persisted data or in input that must be valid.
    #[error("corruption: {0}")]
    Corruption(String),

    /// Authentication or authorization is required and/or failed.
    #[error("authorization error: {0}")]
    Auth(String),

    /// A permission check failed for an authenticated principal.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// An operation was attempted in a state where it is not valid.
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// An operation exceeded its allowed time budget.
    #[error("operation timed out")]
    Timeout,

    /// An I/O error occurred.
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

impl NovaError {
    /// Returns `true` if the error indicates detected data corruption.
    #[must_use]
    pub fn is_corruption(&self) -> bool {
        matches!(self, Self::Corruption(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn io_error_converts_into_nova_error() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file missing");
        let err: NovaError = io_err.into();
        assert!(matches!(err, NovaError::Io(_)));
    }

    #[test]
    fn corruption_flag_is_detected() {
        assert!(NovaError::Corruption("bad checksum".into()).is_corruption());
        assert!(!NovaError::Timeout.is_corruption());
    }

    #[test]
    fn structured_errors_display_clearly() {
        let err = NovaError::NotFound("collection 'students'".into());
        assert_eq!(err.to_string(), "not found: collection 'students'");
    }

    #[test]
    fn result_type_is_usable_across_variants() {
        fn lookup(key: &str) -> Result<u64> {
            if key == "known" {
                Ok(42)
            } else {
                Err(NovaError::NotFound(key.to_owned()))
            }
        }
        assert_eq!(lookup("known").unwrap(), 42);
        assert!(lookup("unknown").is_err());
    }
}
