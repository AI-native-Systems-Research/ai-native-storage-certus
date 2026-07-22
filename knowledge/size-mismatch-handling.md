# Size-mismatch handling on cache lookups (cross-cutting design note)

**Status:** finding + proposed change. **Not** part of the `002-remote-lookup-rdma` feature —
tracked separately so it does not churn that feature's specs.

## Context

`CacheKey` (`interfaces/src/idispatch_map.rs`) and `ExtentKey`
(`interfaces/src/iextent_manager.rs`) are both bare `u64`. Keys are supplied by the client/KV layer
and are already content hashes. A *size collision* — the same `u64` key presented for two different
value lengths — is therefore extremely rare, and in practice all lengths are identical except in the
mixed-model case.

**Decision (agreed):** do **not** fold length into the key. Hashing length on top of an
already-hashed, fixed-width key cannot reduce collision probability and slightly increases it
(pigeonholing a fixed-width digest onto the same width loses information); it would also force an
extent-manager on-disk format bump (`FORMAT_VERSION`) and touch every serialization site for no
benefit. Use the keys as given; treat a size mismatch as a **cache miss**. There is no correctness
problem in declining to cache a colliding key — the client recomputes (rare, so negligible cache
value impact).

## What the code does today (verified)

1. **Production `dispatch_map::lookup(key)` takes only the key and never returns `MismatchSize`.**
   It returns `NotExist | BlockDevice | MemoryTier` keyed purely by `u64`
   (`dispatch-map/src/lib.rs`, `fn lookup`). The `LookupResult::MismatchSize` variant is emitted
   *only* by the dispatchers' own unit-test mocks (`dispatcher/src/lib.rs:2650`,
   `dispatcher-p2p/src/lib.rs:2680`).

2. **The real read path does not reject a size mismatch — it truncates.** After `dm.lookup(key)`
   returns `MemoryTier { pointer, size }`, the dispatcher computes
   `copy_size = ipc_handle.size.min(size)` (`dispatcher/src/lib.rs:1427`/`:1863`,
   `dispatcher-p2p/src/lib.rs:1522`/`:1893`). So a genuine collision today yields a **partial /
   truncated copy**, not a miss. The `MismatchSize` match arms that would turn it into an error are
   dead against the production dispatch-map.

3. **First-writer-wins already holds — no churn.** `create_memory_tier_entry` guards with
   `contains_key(&key) → AlreadyExists(key)` *before* inserting and never evicts/replaces
   (`dispatch-map/src/lib.rs`; invariant P7 "create-no-duplicates"). It rejects *any* pre-existing
   key, so a colliding key is silently left uncached and the original entry is untouched. This is
   exactly the desired no-churn behavior, already enforced by the interface.

## Proposed change (separate from 002)

Make "wrong length ⇒ miss" real in production, without a disk-format or `IDispatchMap` signature
change: on the dispatcher read path, compare the requested size against `dm.entry_size(key)` (method
already exists) and return `KeyNotFound` on mismatch instead of min-copying. Applies to both
`dispatcher` and `dispatcher-p2p`. First-writer-wins needs no change (already holds per (3)).

## Interaction with remote-lookup (handled inside 002)

remote-lookup's publish-on-success step (task T016) treats `create_memory_tier_entry` returning
`AlreadyExists` as success. Under a real collision the pre-existing entry has a *different* size, so
that must be qualified: on `AlreadyExists`, check `entry_size(key)` — **matches ⇒ genuine success;
differs ⇒ collision ⇒ report the key unsatisfied (NotFound), reclaim the private landing slot, do
not evict** (preserving first-writer-wins). Server-side classification (T019) and serve (T021)
already treat a size mismatch as not-available. This is remote-lookup-local; no cross-component code
change.
