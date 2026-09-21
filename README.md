# NovaDB

NovaDB is an **experimental / research database prototype** developed as a
4th-year B.Tech CSE final-year project. It is a Rust-based, SQL-free, document
oriented database with its own JSON-like documents, a custom query language
(NovaQL), a custom binary document format (NBF), page-based persistence, a
persistent B+ tree index, WAL-based crash recovery, and the **Nova Intelligence
Engine (NIE)** — a workload-aware index recommendation system.

> **NovaDB is not production software.** It is a research prototype being built
> incrementally, test-first, phase-by-phase.

## What NovaDB is NOT

- Not a wrapper around MongoDB / PostgreSQL / SQLite / RocksDB / Redis, or any
  other database.
- Not SQL. NovaDB speaks NovaQL and never translates it into SQL.
- Not a distributed system. No sharding, no Raft, no multi-node replication.
- Not production-ready, and makes no such claim.

## Architecture

See [`docs/architecture.md`](docs/architecture.md). In short:

```
CLI / SDK -> NovaDB Server -> Auth -> NovaQL Parser -> AST
         -> Semantic Validator -> Query Planner
         -> (CollectionScan | IndexScan -> B+ Tree)
         -> Executor -> Document Engine -> NBF
         -> Buffer Pool -> Page Manager -> (WAL | Data Pages) -> Disk
Query execution telemetry -> Nova Intelligence Engine (Analyze -> Recommend)
```

## Current status

Implemented through Phase 12 — WAL and recovery:

- Rust workspace with 14 library crates and the `nova` CLI binary.
- Formatting (`rustfmt`), linting (Clippy with `all` + `pedantic`,
  `unsafe_code` denied workspace-wide).
- Structured error conventions (`NovaError`) in `nova-core`.
- `tracing`-based logging setup in `nova-core`.
- On-disk format version registry (`MAGIC = b"NOVA"`, `FORMAT_VERSION = 1`).
- CI workflow (format + clippy + test on Linux and Windows).
- Core data model in `nova-core`: `NovaId` (128-bit time+random identifier),
  `NovaTimestamp` (millisecond-precision timestamps), `NovaValue` (typed
  scalar/collection values), and `Document` (typed field map keyed by sorted
  fields, always carrying an implicit `_id`).
- Deterministic NBF v1 encoding and bounds-checked decoding in `nova-nbf`, with
  minimal unsigned LEB128 lengths/counts, typed malformed-input errors, a
  nesting limit, canonical NaN encoding, and complete round-trip tests.
- Fixed-size checksummed slotted pages in `nova-storage`, with stable reusable
  slot identifiers, payload compaction, corruption validation, and direct
  allocate/read/write/sync page-file operations.
- An end-to-end storage engine that NBF-encodes documents, places them across
  data pages, rejects duplicate ids, rebuilds its lookup directory on open,
  and preserves documents across explicit shutdown/restart.
- A total, UTF-8-aware NovaQL lexer with typed literals and keywords,
  JSON-compatible string escapes, comments, operators, exact source spans, and
  typed errors for malformed input.
- A strongly typed, source-spanned NovaQL AST and precedence-aware parser for
  document-native `collection.operation { ... }` commands, collection/index
  DDL, pipeline stages, expressions, and nested document/array literals.
- A deterministic collection-scan executor with CRUD, collection DDL,
  filtering, projection, stable multi-key sorting, skip/limit, nested updates,
  checked expression evaluation, first-class array membership through
  `contains`, and an explicit backend boundary.
- A from-scratch persistent B+ tree in `nova-index` with typed integer, float,
  string, and NovaId keys; duplicate-key document ids; exact and inclusive
  range lookup; recursive leaf/internal/root splits; linked leaves; structural
  verification; and a checksummed, versioned snapshot format.
- A persistent index catalog with named single-field definitions, collection
  backfill, nested-path extraction, exact lookup, reopen/drop lifecycle, and an
  `IndexedBackend` decorator that maintains entries across NovaQL insert,
  update, delete, collection drop, and index DDL operations.
- A deterministic physical planner that selects exact-key index scans for
  matching equality predicates, falls back safely to collection scans, keeps
  residual filters for correctness, and exposes stable `explain` output.
- A bounded LRU buffer pool over the page manager with hit/miss/eviction/write
  counters, explicit page pinning, dirty write-back on eviction, flush and
  shutdown synchronization, and deterministic all-frames-pinned failure.
- A checksummed write-ahead log with monotonic LSNs, transaction-tagged begin,
  full-page write, commit, abort, and checkpoint records; explicit flush
  durability; torn-tail handling; and idempotent committed-only redo recovery.

Phase 0 shipped the workspace scaffolding, Phase 1 shipped the data model, and
Phase 2 shipped NBF. Phase 3 shipped page storage, and Phase 4 shipped the first
persistent document path. Phase 5 shipped lexical analysis, Phase 6 shipped the
AST and parser, and Phase 7 shipped collection-scan query execution. Phase 8
shipped the persistent B+ tree, Phase 9 integrated index lifecycle, and Phase
10 shipped plan construction and index-scan selection. Phase 11 shipped the
bounded page buffer, and Phase 12 shipped write-ahead logging and startup redo.
Next: Phase 13 (transactions and locking).

## Build

Requires a stable Rust toolchain (see `rust-toolchain.toml`).

```sh
cargo build --workspace
```

## Run

The CLI binary only prints version info until Phase 16.

```sh
cargo run -p nova-cli
```

## Testing

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Or run the cross-platform gate directly (`scripts/check.ps1` on Windows,
`scripts/check.sh` on Linux/macOS).

## Research objective

> Can workload-aware index recommendations significantly reduce document scans
> and query latency in a custom document-oriented database while maintaining
> acceptable storage and write overhead?

NovaDB does **not** claim to be faster or better than existing databases. All
performance claims will be backed by reproducible benchmarks
(`docs/benchmark-methodology.md`).

## Project structure

```
crates/
  nova-core          Core data model, errors, logging, format registry
  nova-nbf           Nova Binary Format (Phase 2)
  nova-storage       Page-based storage (Phase 3-4)
  nova-buffer        Buffer pool (Phase 11)
  nova-wal           WAL + recovery (Phase 12)
  nova-index         B+ tree (Phase 8-9)
  nova-query         NovaQL lexer/parser (Phase 5-6)
  nova-planner       Query planner (Phase 10)
  nova-executor      Execution operators (Phase 7)
  nova-transaction   Transactions/locking (Phase 13)
  nova-intelligence  NIE (Phase 17-20)
  nova-auth          Auth/roles (Phase 15)
  nova-server        Server (Phase 14)
  nova-client        SDK
cli/nova-cli         CLI (Phase 16)
tests/               integration, recovery, corruption, concurrency, fuzz, perf
benchmarks/          reproducible benchmarks
datasets/            reproducible datasets
```

## Roadmap

Phase 0 ✓ workspace — Phase 1 ✓ core data model — Phase 2 ✓ NBF — Phase 3 ✓ page
storage — Phase 4 ✓ storage engine (insert/shutdown/restart/read) —
Phase 5 ✓ NovaQL lexer — Phase 6 ✓ NovaQL parser — Phase 7 ✓ query executor —
Phase 8 ✓ B+ tree — Phase 9 ✓ index integration — Phase 10 ✓ planner — Phase 11 ✓ buffer
pool — Phase 12 ✓ WAL/recovery — Phase 13 transactions — Phase 14 server —
Phase 15 auth —
Phase 16 CLI — Phase 17 telemetry — Phase 18-20 NIE — Phase 21 optional
AI assistant — Phase 22 hardening.

Future work (out of scope for v1): SQL compatibility, distributed consensus,
sharding, replication, full MVCC, graph engine, HNSW/ANN, LSM trees, learned
indexes, Kubernetes operator.

## License

MIT — see [LICENSE](LICENSE).
