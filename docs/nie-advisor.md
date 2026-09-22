# Nova Intelligence Engine Index Advisor (Phase 19)

The index advisor consumes a Phase 18 workload report and the current index
catalog. It proposes a single-field index only when all configured evidence
thresholds are met:

- the predicate is an exact integer, float, or string equality that the current
  planner can use;
- enough successful executions and collection scans were observed;
- collection scans examined enough documents;
- the ratio of returned to examined documents is at or below the configured
  selectivity ceiling; and
- the collection/path pair is not already indexed.

Defaults require three successful executions, three collection scans, 100
examined documents, and a return ratio no greater than 25%. Ratios use integer
basis points and overflow-safe arithmetic. Output is ordered by collection and
path because it follows the analyzer's deterministic aggregates.

Every proposal includes the exact scan count, examined and returned totals,
elapsed microseconds, prior index-scan count, and calculated return ratio.
Invalid policy values return a typed error. The advisor is read-only: it never
creates an index, changes a plan, or participates in database correctness.

Phase 20 adds an explicit human-controlled lifecycle and before/after
evaluation; the advisor itself remains read-only.
