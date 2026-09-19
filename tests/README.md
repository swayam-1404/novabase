# Integration tests

Workspace-level integration tests exercising NovaDB end-to-end. Populated
starting with Phase 4 (storage engine restart persistence).

Subdirectories are populated by later phases:

- `recovery/` — WAL/crash recovery tests (Phase 12)
- `corruption/` — checksum/corruption detection tests (Phase 3+)
- `concurrency/` — locking/concurrency tests (Phase 13)
- `fuzz/` — NovaQL + NBF fuzz harnesses (Phase 22, harnesses from Phase 2/5)
- `performance/` — reproducible benchmark-style tests (Phase 10+)