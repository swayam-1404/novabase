# Storage Engine (Phase 4)

Phase 4 connects NovaDB's core document model, NBF codec, and page storage into
the first end-to-end persistent document path.

## API and lifecycle

`StorageEngine::open(path)` opens or creates one page file. `insert(document)`
encodes the document as NBF and places it in the first page with enough free
space, allocating a page when necessary. `get(id)` returns the decoded document
or `None`. `shutdown()` synchronizes the page file before consuming the engine.

The Phase 4 durability acceptance path is:

1. Open a new engine.
2. Insert one or more documents.
3. Explicitly shut down the engine.
4. Reopen the same page file.
5. Read documents equal to those inserted, including their `NovaId` values.

Duplicate document ids are rejected with `NovaError::AlreadyExists`; they never
silently overwrite stored data. A single encoded document must fit in one data
page. Multi-page records are deliberately not part of v1.

## Startup directory

The engine scans every validated page and NBF record at startup to rebuild an
in-memory `NovaId -> (PageId, SlotId)` directory. A duplicate id or malformed
record makes opening fail with a typed corruption error. This scan is simple
and correct for the early storage phase. The persistent B+ tree introduced in
Phases 8–9 replaces it as the scalable lookup path.

## Deliberate phase boundaries

- Updates and deletes are not exposed yet.
- The Phase 11 buffer pool will replace direct page reads for caching.
- Atomic commit and crash/torn-write recovery require the Phase 12 WAL.
- Phase 13 adds transaction isolation and locking.

Until WAL lands, Phase 4 guarantees explicit shutdown/restart persistence, not
recovery from a process or power failure during a write.
