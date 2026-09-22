# Nova Intelligence Engine Recommendation Evaluation (Phase 20)

`RecommendationLifecycle` records each proposal by collection and field path.
Its state machine requires explicit transitions:

```text
Proposed -> Accepted -> Applied -> Evaluated
         `-> Dismissed
```

Accepting and dismissing represent human decisions. `mark_applied` records that
an external database operation created the index; it does not change the index
catalog itself. Invalid transitions, unknown recommendations, duplicate
proposals, and zero observation thresholds return typed errors.

Evaluation compares the proposal's collection-scan baseline with a later
telemetry window containing indexed executions of the same collection/path.
The default requires at least three index scans. Reports include baseline and
observed execution counts, average documents examined, average elapsed
microseconds, and signed basis-point reductions. A negative reduction clearly
reports a regression. Failed events and collection scans in the later window
do not count as post-index observations.

The lifecycle is deliberately in-memory and advisory. Persisting administrative
decisions can be added later at the server boundary, but recommendation state
never participates in query correctness, durability, recovery, or authorization.
