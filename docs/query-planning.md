# NovaDB Query Planning (Phase 10)

The `nova-planner` crate converts a parsed NovaQL query plus visible index
definitions into an inspectable physical `QueryPlan`.

## Access paths

- `Command` for DDL and inserts that do not scan an existing collection.
- `CollectionScan` as the deterministic correctness fallback.
- `IndexScan` for an exact equality between an indexed path and an `Int64`,
  `Float64`, or string literal.

The planner searches equality terms recursively through logical `and`. It does
not currently infer ranges, reorder boolean expressions, coerce types, select
composite indexes, or estimate costs. If more than one index matches, the
lexicographically first index name wins, making plans reproducible.

All parsed pipeline stages appear in source order in the plan. Filters are
never removed after index selection: they execute as residual checks over index
candidates. `explain` uses the same planning function as execution and returns
stable text such as:

```text
IndexScan(students, by_score, Int64(90)) -> Filter -> Project
```

This phase establishes deterministic rule-based planning. Cost statistics and
workload-aware recommendations belong to the later telemetry/NIE phases and
must never override correctness checks.
