# NovaDB Benchmark Methodology

NovaDB is a research prototype. Performance claims are valid only when produced
by the procedure below and accompanied by raw results. CI never asserts wall
clock thresholds.

## Environment record

Record the NovaDB commit, Rust version, build target, operating system, CPU,
logical core count, memory, storage device, filesystem, power profile, and any
virtualization. Stop unrelated heavy workloads. Use the same machine and data
directory type for every compared configuration.

Build with `cargo build --workspace --release`. Run the full correctness gate
before collecting timings. Use a new data directory for each independent run
and retain server/client logs without credentials or bearer tokens.

## Dataset and workload

Generate data from [`../datasets/workload-spec.md`](../datasets/workload-spec.md)
with a recorded document count and seed. The primary experiment uses 10,000,
100,000, 500,000, and 1,000,000 documents. Query literals come from the same
seeded distribution and are saved with the raw results.

Measure these configurations separately:

1. collection scan with no candidate index;
2. indexed execution after backfill and reopen;
3. steady-state indexed execution;
4. insert/update/delete throughput without and with the index; and
5. on-disk bytes before and after index creation.

The query mix must include existing and missing keys and fixed selectivity
bands (approximately 0.1%, 1%, 10%, and 50%). Do not compare different query
results or different durability settings.

## Measurement protocol

- Warm up with at least 100 untimed operations or one complete workload pass.
- Run at least 30 measured repetitions per query/configuration pair.
- Use a monotonic clock around the client-visible operation.
- Restart and reopen NovaDB for cold-start measurements; label them separately.
- Alternate configuration order between repetitions to reduce drift.
- Record documents examined and returned from telemetry alongside latency.
- Record failures, timeouts, and discarded runs; never silently remove them.

Report median, p95, minimum, maximum, and sample count. Report throughput only
with the concurrency, request count, and elapsed interval. For ratios, publish
both underlying values. Any statistical test and confidence interval must be
named. A result is inconclusive when correctness differs or the run procedure
cannot be reproduced.

## NIE evaluation

The NIE experiment freezes a baseline telemetry window, generates proposals
with recorded advisor thresholds, applies accepted indexes explicitly, and
collects a disjoint post-application window. Report examined-document and
latency changes, storage overhead, and write overhead. Negative results and
dismissed recommendations are part of the result set.
