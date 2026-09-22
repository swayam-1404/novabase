# Integration tests

Workspace-level hardening tests are compiled as the `nova-integration-tests`
package and run by `cargo test --workspace`.

- `integration/` — parser, executor, persistent index, telemetry, advisor,
  assistant suggestion, lifecycle, and impact evaluation in one workflow
- `concurrency/` — multi-threaded telemetry recording
- `fuzz/` — deterministic malformed NBF and NovaQL totality sweeps

Recovery, corruption, protocol, authentication, locking, storage, and index
tests remain colocated with their crates so they can exercise private format
invariants. Performance methodology is documented separately; CI does not use
timing thresholds because shared runners are nondeterministic.
