use std::collections::{BTreeMap, BTreeSet};

use crate::{TelemetryAccess, TelemetryEvent};

/// Deterministic aggregate of a telemetry snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkloadReport {
    pub total_events: u64,
    pub successful_events: u64,
    pub failed_events: u64,
    pub command_executions: u64,
    pub collection_scans: u64,
    pub index_scans: u64,
    pub total_examined: u64,
    pub total_returned: u64,
    pub total_elapsed_micros: u64,
    pub fingerprints: Vec<FingerprintStats>,
    pub predicate_paths: Vec<PredicatePathStats>,
    pub index_candidates: Vec<IndexCandidateStats>,
}

/// Aggregate for one collection and literal-free query shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FingerprintStats {
    pub collection: Option<String>,
    pub fingerprint: String,
    pub successful_executions: u64,
    pub failed_executions: u64,
    pub collection_scans: u64,
    pub index_scans: u64,
    pub total_examined: u64,
    pub total_returned: u64,
    pub total_elapsed_micros: u64,
}

/// Aggregate for one predicate path within a collection.
///
/// Successful metrics are copied to every distinct predicate path observed in
/// an event. They therefore describe per-path workload evidence and must not be
/// summed across paths to calculate global totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PredicatePathStats {
    pub collection: String,
    pub path: String,
    pub successful_executions: u64,
    pub failed_executions: u64,
    pub collection_scans: u64,
    pub index_scans: u64,
    pub total_examined: u64,
    pub total_returned: u64,
    pub total_elapsed_micros: u64,
}

/// Aggregate for one planner-usable equality path within a collection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexCandidateStats {
    pub collection: String,
    pub path: String,
    pub successful_executions: u64,
    pub failed_executions: u64,
    pub collection_scans: u64,
    pub index_scans: u64,
    pub collection_scan_examined: u64,
    pub collection_scan_returned: u64,
    pub collection_scan_elapsed_micros: u64,
}

/// Aggregates a stable telemetry snapshot into deterministic workload evidence.
#[must_use]
pub fn analyze_workload(events: &[TelemetryEvent]) -> WorkloadReport {
    let mut report = WorkloadReport::default();
    let mut fingerprints = BTreeMap::new();
    let mut predicate_paths = BTreeMap::new();
    let mut index_candidates = BTreeMap::new();

    for event in events {
        report.total_events = report.total_events.saturating_add(1);
        match event.access {
            TelemetryAccess::Command => {
                report.command_executions = report.command_executions.saturating_add(1);
            }
            TelemetryAccess::CollectionScan => {
                report.collection_scans = report.collection_scans.saturating_add(1);
            }
            TelemetryAccess::IndexScan { .. } => {
                report.index_scans = report.index_scans.saturating_add(1);
            }
        }

        let fingerprint = fingerprints
            .entry((event.collection.clone(), event.fingerprint.clone()))
            .or_insert_with(|| FingerprintStats {
                collection: event.collection.clone(),
                fingerprint: event.fingerprint.clone(),
                ..FingerprintStats::default()
            });

        if event.succeeded {
            report.successful_events = report.successful_events.saturating_add(1);
            add_successful_metrics(&mut report, event);
            add_fingerprint_success(fingerprint, event);
        } else {
            report.failed_events = report.failed_events.saturating_add(1);
            fingerprint.failed_executions = fingerprint.failed_executions.saturating_add(1);
        }

        if let Some(collection) = &event.collection {
            for path in event.predicate_paths.iter().collect::<BTreeSet<_>>() {
                let stats = predicate_paths
                    .entry((collection.clone(), path.clone()))
                    .or_insert_with(|| PredicatePathStats {
                        collection: collection.clone(),
                        path: path.clone(),
                        ..PredicatePathStats::default()
                    });
                if event.succeeded {
                    add_path_success(stats, event);
                } else {
                    stats.failed_executions = stats.failed_executions.saturating_add(1);
                }
            }
            for path in event.index_candidate_paths.iter().collect::<BTreeSet<_>>() {
                let stats = index_candidates
                    .entry((collection.clone(), path.clone()))
                    .or_insert_with(|| IndexCandidateStats {
                        collection: collection.clone(),
                        path: path.clone(),
                        ..IndexCandidateStats::default()
                    });
                if event.succeeded {
                    add_candidate_success(stats, event);
                } else {
                    stats.failed_executions = stats.failed_executions.saturating_add(1);
                }
            }
        }
    }

    report.fingerprints = fingerprints.into_values().collect();
    report.predicate_paths = predicate_paths.into_values().collect();
    report.index_candidates = index_candidates.into_values().collect();
    report
}

fn add_successful_metrics(report: &mut WorkloadReport, event: &TelemetryEvent) {
    report.total_examined = report.total_examined.saturating_add(as_u64(event.examined));
    report.total_returned = report.total_returned.saturating_add(as_u64(event.returned));
    report.total_elapsed_micros = report
        .total_elapsed_micros
        .saturating_add(event.elapsed_micros);
}

fn add_fingerprint_success(stats: &mut FingerprintStats, event: &TelemetryEvent) {
    stats.successful_executions = stats.successful_executions.saturating_add(1);
    add_access(&mut stats.collection_scans, &mut stats.index_scans, event);
    stats.total_examined = stats.total_examined.saturating_add(as_u64(event.examined));
    stats.total_returned = stats.total_returned.saturating_add(as_u64(event.returned));
    stats.total_elapsed_micros = stats
        .total_elapsed_micros
        .saturating_add(event.elapsed_micros);
}

fn add_path_success(stats: &mut PredicatePathStats, event: &TelemetryEvent) {
    stats.successful_executions = stats.successful_executions.saturating_add(1);
    add_access(&mut stats.collection_scans, &mut stats.index_scans, event);
    stats.total_examined = stats.total_examined.saturating_add(as_u64(event.examined));
    stats.total_returned = stats.total_returned.saturating_add(as_u64(event.returned));
    stats.total_elapsed_micros = stats
        .total_elapsed_micros
        .saturating_add(event.elapsed_micros);
}

fn add_candidate_success(stats: &mut IndexCandidateStats, event: &TelemetryEvent) {
    stats.successful_executions = stats.successful_executions.saturating_add(1);
    add_access(&mut stats.collection_scans, &mut stats.index_scans, event);
    if event.access == TelemetryAccess::CollectionScan {
        stats.collection_scan_examined = stats
            .collection_scan_examined
            .saturating_add(as_u64(event.examined));
        stats.collection_scan_returned = stats
            .collection_scan_returned
            .saturating_add(as_u64(event.returned));
        stats.collection_scan_elapsed_micros = stats
            .collection_scan_elapsed_micros
            .saturating_add(event.elapsed_micros);
    }
}

fn add_access(collection_scans: &mut u64, index_scans: &mut u64, event: &TelemetryEvent) {
    match event.access {
        TelemetryAccess::Command => {}
        TelemetryAccess::CollectionScan => {
            *collection_scans = collection_scans.saturating_add(1);
        }
        TelemetryAccess::IndexScan { .. } => {
            *index_scans = index_scans.saturating_add(1);
        }
    }
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(
        fingerprint: &str,
        collection: Option<&str>,
        paths: &[&str],
        access: TelemetryAccess,
        succeeded: bool,
    ) -> TelemetryEvent {
        TelemetryEvent {
            fingerprint: fingerprint.to_owned(),
            collection: collection.map(str::to_owned),
            predicate_paths: paths.iter().map(ToString::to_string).collect(),
            index_candidate_paths: paths.iter().map(ToString::to_string).collect(),
            access,
            examined: 100,
            returned: 10,
            elapsed_micros: 25,
            succeeded,
        }
    }

    #[test]
    fn aggregates_and_sorts_workload_dimensions() {
        let events = vec![
            event(
                "get:students|filter",
                Some("students"),
                &["name", "address.state"],
                TelemetryAccess::CollectionScan,
                true,
            ),
            event(
                "get:students|filter",
                Some("students"),
                &["name"],
                TelemetryAccess::IndexScan {
                    index: "by_name".to_owned(),
                },
                true,
            ),
        ];

        let report = analyze_workload(&events);
        assert_eq!((report.total_events, report.successful_events), (2, 2));
        assert_eq!((report.collection_scans, report.index_scans), (1, 1));
        assert_eq!((report.total_examined, report.total_returned), (200, 20));
        assert_eq!(report.fingerprints.len(), 1);
        assert_eq!(report.fingerprints[0].successful_executions, 2);
        assert_eq!(report.predicate_paths.len(), 2);
        assert_eq!(report.predicate_paths[0].path, "address.state");
        assert_eq!(report.predicate_paths[1].path, "name");
        assert_eq!(report.predicate_paths[1].index_scans, 1);
        assert_eq!(report.index_candidates.len(), 2);
    }

    #[test]
    fn failures_are_counted_but_excluded_from_success_metrics() {
        let events = vec![event(
            "get:students|filter",
            Some("students"),
            &["score", "score"],
            TelemetryAccess::CollectionScan,
            false,
        )];

        let report = analyze_workload(&events);
        assert_eq!((report.successful_events, report.failed_events), (0, 1));
        assert_eq!((report.total_examined, report.total_elapsed_micros), (0, 0));
        assert_eq!(report.fingerprints[0].failed_executions, 1);
        assert_eq!(report.predicate_paths.len(), 1);
        assert_eq!(report.predicate_paths[0].failed_executions, 1);
        assert_eq!(report.predicate_paths[0].collection_scans, 0);
    }

    #[test]
    fn empty_and_large_snapshots_are_total() {
        assert_eq!(analyze_workload(&[]), WorkloadReport::default());
        let mut first = event(
            "get:a|filter",
            Some("a"),
            &["x"],
            TelemetryAccess::CollectionScan,
            true,
        );
        first.elapsed_micros = u64::MAX;
        let second = first.clone();
        let report = analyze_workload(&[first, second]);
        assert_eq!(report.total_elapsed_micros, u64::MAX);
        assert_eq!(report.fingerprints[0].total_elapsed_micros, u64::MAX);
    }
}
