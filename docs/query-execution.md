# Query Execution (Phases 7–9)

Phase 7 executes the typed NovaQL AST using deterministic collection scans.
The executor depends on the `ExecutionBackend` trait, which separates query
semantics from the later server/catalog persistence design. A deterministic
`MemoryBackend` is included for embedding and end-to-end verification.

## Supported commands

- `<collection>.get { ... }` returns documents after applying its pipeline in
  source order.
- `insert` evaluates an object literal, generates root/nested `NovaId` values,
  and inserts one document.
- `update` scans and selects documents, applies `set` assignments in source
  order, and replaces the selected documents.
- `delete` scans and selects documents, then deletes them by id.
- Collection creation and removal delegate to the backend.

When a backend is wrapped by `IndexedBackend`, `create index` backfills and
persists a named single-field index and `drop index` removes it. Inserts,
updates, deletes, and collection drops maintain the catalog. A backend without
that capability returns `NovaError::Unsupported`; composite index execution is
also explicitly unsupported in index format v1. `explain` returns
`Unsupported` until the Phase 10 planner produces execution plans.

## Pipeline operators

The brace predicate runs first, followed by `project`, `sort`, `skip`, `limit`,
and `set` stages from left to right. A predicate requires a boolean result.
Projection preserves the document id, supports dotted paths, omits missing
paths, and retains only requested nested fields. Sorting is stable and supports
multiple keys and directions. Missing values sort before null, and null sorts
before concrete values in ascending order.

`set` is valid only for updates. It can create missing intermediate documents,
but cannot traverse an existing scalar or modify the implicit `_id`.

## Expression semantics

NovaDB performs no implicit numeric or string conversion:

- Arithmetic requires two `Int64` values or two `Float64` values.
- Checked integer arithmetic reports overflow and division by zero.
- Float arithmetic rejects division by zero and non-finite results.
- Ordering requires matching scalar types. Arrays and documents are not
  orderable.
- Equality across unlike types is false; inequality is true.
- `contains` requires an array on the left and uses exact typed equality for
  membership.
- A missing path is distinct from explicit null.
- `and` and `or` require booleans and short-circuit; `not`/`!` also requires a
  boolean.

Execution failures are typed `NovaError` values and malformed/runtime-invalid
queries never panic. Transactional atomicity across multiple backend writes is
introduced in Phase 13; Phase 7 defines operator semantics only.
