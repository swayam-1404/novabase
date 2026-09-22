# NovaDB Architecture

This document describes the intended architecture of NovaDB and tracks what is
actually implemented. It is updated as phases land; it never documents
unbuilt features as completed.

## Layers

```
Applications        (nova-cli, future SDK apps)
  |
  +--- CLI (nova-cli) ---------- SDK (nova-client) ---+
  |                                                    |
  +------------------ NovaDB Server -------------------+
                          |
                     Authentication (nova-auth)
                          |
              NovaQL Parser (nova-query: lexer/parser/AST)
                          |
                   Semantic Validator
                          |
                    Query Planner (nova-planner)
                          |
              +---------------+---------------+
              |                               |
         CollectionScan                IndexScan
                                          |
                                       B+ Tree (nova-index)
              |                               |
              +---------------+---------------+
                          |
                       Executor (nova-executor)
                          |
                    Document Engine
                          |
                   NBF (nova-nbf)
                          |
                 Buffer Pool (nova-buffer)
                          |
                  Page Manager (nova-storage)
                          |
              +----------------+---------+
              |                          |
            WAL (nova-wal)          Data Pages
              |                          |
              +-----------+--------------+
                          |
                         Disk
```

Separate analytical path (never part of correctness paths):

```
Query Execution
      |
   Telemetry
      |
      v
Nova Intelligence Engine (nova-intelligence)
      |
 +----+-------------+
 |                  |
 Analyze          Recommend
 |                  |
 +--------+---------+
          |
     Index Advisor
```

## Design principles

- **Deterministic correctness.** AI/ML/heuristics never decide transaction
  correctness, WAL recovery correctness, authorization, data durability, or
  whether committed data exists.
- **No SQL.** NovaQL is parsed and planned by NovaDB's own pipeline.
- **No external storage engine.** Pages, WAL, B+ tree, buffer pool and
  documents all live in this repository.
- **Structured errors.** `NovaError` (nova-core) is the shared boundary error;
  crates may use narrow types internally.
- **Unsafe-free.** `unsafe_code` is denied workspace-wide.
- **Test-first.** Each phase ships implementation + tests + failure handling +
  docs, and must pass the CI gate before the next phase starts.

## Implementation status

| Component                | Crate              | Status   | Phase |
|--------------------------|--------------------|----------|-------|
| Errors, logging, version | nova-core          | shipped  | 0     |
| Data model               | nova-core          | shipped  | 1     |
| NBF                      | nova-nbf           | shipped  | 2     |
| Pages                    | nova-storage       | shipped  | 3     |
| Storage engine           | nova-storage       | shipped  | 4     |
| NovaQL lexer             | nova-query         | shipped  | 5     |
| NovaQL parser            | nova-query         | shipped  | 6     |
| Executor                 | nova-executor      | shipped  | 7     |
| B+ tree                  | nova-index         | shipped  | 8     |
| Index integration        | nova-index         | shipped  | 9     |
| Planner                  | nova-planner       | shipped  | 10    |
| Buffer pool              | nova-buffer        | shipped  | 11    |
| WAL / recovery           | nova-wal           | shipped  | 12    |
| Transactions             | nova-transaction   | shipped  | 13    |
| Server                   | nova-server        | shipped  | 14    |
| Auth                     | nova-auth          | shipped  | 15    |
| CLI + client SDK         | nova-cli/client    | shipped  | 16    |
| Telemetry                | nova-intelligence  | shipped  | 17    |
| NIE workload analysis    | nova-intelligence  | shipped  | 18    |
| NIE index advisor        | nova-intelligence  | shipped  | 19    |
| NIE impact evaluation    | nova-intelligence  | shipped  | 20    |
| Optional AI assistant    | nova-intelligence  | shipped  | 21    |
| Hardening/validation     | workspace          | shipped  | 22    |

## Cargo dependency layering (planned, subject to change)

Dependencies flow downward; cycles are forbidden:

```
        nova-server
       /     |      \
 nova-auth   nova-planner  nova-transaction
    |          /   |    \
nova-core  nova-executor nova-query
             /   |   \        \
     nova-storage nova-index nova-core
        |      |      |
   nova-wal nova-buffer nova-core
        |      |    \
    nova-core nova-storage nova-core
```

Final wiring is decided per phase when dependencies are actually needed.
