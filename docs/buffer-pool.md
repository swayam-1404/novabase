# NovaDB Buffer Pool (Phase 11)

`nova-buffer` places a fixed-capacity, write-back cache above `PageManager`.
Capacity is mandatory and positive, so the cache cannot grow without bound.

Each frame owns one validated `Page`, a dirty flag, a pin count, and a logical
last-used clock. Cache misses evict the unpinned frame with the oldest clock;
page id breaks ties deterministically. Dirty victims are written before reuse.
If every frame is pinned, the operation returns `NovaError::InvalidState` and
does not allocate a new disk page.

Mutable access marks a frame dirty. `flush_all` writes every dirty frame and
then synchronizes the page file; `shutdown` performs the same operation while
consuming the pool. Explicit `pin`/`unpin` calls protect frames during
multi-step operations, and unmatched unpins are typed errors.

The pool reports cumulative hits, misses, evictions, and physical writes.
Phase 12 supplies WAL ordering; Phase 11 by itself makes no crash-recovery
claim beyond synchronized page-file flushes.
