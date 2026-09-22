# NovaDB Execution Telemetry (Phase 17)

Telemetry is an optional one-way observation of query execution. Callers use
`execute_with_telemetry`; the ordinary executor API remains available without
analytics. A sink receives exactly one event for successful or failed
execution.

Events include a normalized command/stage fingerprint, collection, sorted
predicate paths, collection/index/command access type, candidate documents
examined, results or rows affected, elapsed microseconds, and success state.
Fingerprints retain collection and operator shape but omit all literal values.
Documents, credentials, tokens, and returned field values are never recorded.
Telemetry separately identifies exact integer, float, and string equality
paths that the current planner can use for an index. Literal values remain
excluded.

`TelemetrySink::record` has no result channel. The in-memory implementation
silently drops an event if its mutex was poisoned, ensuring telemetry failure
cannot change database correctness, durability, authorization, or a query's
returned result. Snapshot and clear operations expose typed errors to explicit
analytics callers.

Phase 17 records evidence only. Workload aggregation, recommendations,
confidence rules, and recommendation evaluation are Phases 18–20.
