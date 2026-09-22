# Nova Intelligence Engine Workload Analysis (Phase 18)

`analyze_workload` converts a stable telemetry snapshot into deterministic,
read-only evidence. It reports global totals and ordered aggregates for each
literal-free fingerprint and each collection/predicate-path pair.

Successful events contribute examined-document, returned-result, elapsed-time,
and access-path metrics. Failed events remain visible through failure counters
but their potentially partial work is excluded from successful-performance
evidence. Repeated predicate paths in one event are counted once.

Metrics attached to a query containing multiple predicate paths are copied to
each distinct path. This makes each path independently useful to the later
advisor, but per-path totals must not be summed to reconstruct global totals.
All counters saturate instead of overflowing, and ordered maps make reports
stable across runs.

Phase 18 performs no recommendation, schema change, or automatic index
creation. Recommendation policy and explainable thresholds belong to Phase 19.
