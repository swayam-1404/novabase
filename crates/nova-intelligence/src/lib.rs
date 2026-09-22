//! Nova Intelligence Engine telemetry and workload analysis.
//!
//! Analytics are observational only and never participate in correctness.

#![forbid(unsafe_code)]

use std::sync::Mutex;

use nova_core::error::{NovaError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryAccess {
    Command,
    CollectionScan,
    IndexScan { index: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryEvent {
    pub fingerprint: String,
    pub collection: Option<String>,
    pub predicate_paths: Vec<String>,
    pub access: TelemetryAccess,
    pub examined: usize,
    pub returned: usize,
    pub elapsed_micros: u64,
    pub succeeded: bool,
}

pub trait TelemetrySink: Send + Sync {
    /// Records an event without affecting query execution.
    fn record(&self, event: TelemetryEvent);
}

#[derive(Debug, Default)]
pub struct InMemoryTelemetry {
    events: Mutex<Vec<TelemetryEvent>>,
}

impl InMemoryTelemetry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// Returns a stable snapshot.
    ///
    /// # Errors
    /// Returns a typed error if the mutex was poisoned by external panic.
    pub fn snapshot(&self) -> Result<Vec<TelemetryEvent>> {
        self.events
            .lock()
            .map(|events| events.clone())
            .map_err(|_| NovaError::Internal("telemetry mutex poisoned".to_owned()))
    }

    /// Removes all observations.
    ///
    /// # Errors
    /// Returns a typed error if the mutex was poisoned.
    pub fn clear(&self) -> Result<()> {
        self.events
            .lock()
            .map(|mut events| events.clear())
            .map_err(|_| NovaError::Internal("telemetry mutex poisoned".to_owned()))
    }
}

impl TelemetrySink for InMemoryTelemetry {
    fn record(&self, event: TelemetryEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_preserves_order_and_can_be_cleared() {
        let sink = InMemoryTelemetry::new();
        sink.record(TelemetryEvent {
            fingerprint: "get:students|filter".to_owned(),
            collection: Some("students".to_owned()),
            predicate_paths: vec!["branch".to_owned()],
            access: TelemetryAccess::CollectionScan,
            examined: 10,
            returned: 2,
            elapsed_micros: 5,
            succeeded: true,
        });
        assert_eq!(sink.snapshot().unwrap().len(), 1);
        sink.clear().unwrap();
        assert!(sink.snapshot().unwrap().is_empty());
    }
}
