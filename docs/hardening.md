# Hardening and Reproducible Validation (Phase 22)

Phase 22 makes the repository's claimed validation executable. The
`nova-integration-tests` workspace package is part of every full test run and
covers boundaries that crate-local unit tests cannot cover alone.

## Cross-crate checks

- A complete NovaQL workflow creates data, records collection scans, analyzes
  telemetry, recommends an index, generates a deterministic assistant command,
  applies it explicitly, observes index scans, and evaluates the impact.
- Deterministic malformed-input sweeps feed every length from 0 through 256 to
  NBF decoding and NovaQL parsing under panic capture.
- Eight threads record 2,000 telemetry events into one sink and verify that
  every complete event is retained.

Format corruption, torn WAL recovery, storage restart, index reopen, protocol
bounds, authentication, and transaction locking remain tested in their owning
crates, where private invariants are visible.

## Required gate

Every phase and release candidate must pass, with zero warnings:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same commands on Windows and Linux. `scripts/check.ps1` and
`scripts/check.sh` are local equivalents.

## Performance discipline

Timing is deliberately excluded from CI pass/fail decisions. Shared runners do
not provide a stable performance environment. Performance work must follow
`docs/benchmark-methodology.md` and `datasets/workload-spec.md`, retain raw
samples, and report negative or inconclusive results.

NovaDB remains an experimental single-node research prototype. Hardening does
not imply production readiness, distributed operation, or compatibility beyond
the documented versioned formats and protocol.
