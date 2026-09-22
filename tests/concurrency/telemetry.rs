use std::sync::Arc;
use std::thread;

use nova_intelligence::{InMemoryTelemetry, TelemetryAccess, TelemetryEvent, TelemetrySink};

#[test]
fn concurrent_telemetry_recording_preserves_every_complete_event() {
    let telemetry = Arc::new(InMemoryTelemetry::new());
    let workers: Vec<_> = (0..8)
        .map(|worker| {
            let telemetry = Arc::clone(&telemetry);
            thread::spawn(move || {
                for sequence in 0..250 {
                    telemetry.record(TelemetryEvent {
                        fingerprint: format!("get:items|filter:{worker}:{sequence}"),
                        collection: Some("items".to_owned()),
                        predicate_paths: vec!["kind".to_owned()],
                        index_candidate_paths: vec!["kind".to_owned()],
                        access: TelemetryAccess::CollectionScan,
                        examined: 10,
                        returned: 1,
                        elapsed_micros: 1,
                        succeeded: true,
                    });
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let snapshot = telemetry.snapshot().unwrap();
    assert_eq!(snapshot.len(), 2_000);
    assert!(snapshot.iter().all(|event| event.succeeded));
}
