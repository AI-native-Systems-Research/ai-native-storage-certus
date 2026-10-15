# Drift Resolution Proposals — memory-tier

Generated: 2026-10-14T16:47:49Z
Based on: `drift-report.json` from 2026-10-14T16:47:49Z (branch `fix/memory-tier-insert-track-panic`, `78f60ab8`)
Mode: BATCH. **Every proposal is `approved: false`. Nothing has been applied.**
(The previous proposal set, from the 2026-08-20 Phase B pass, was fully applied. It is kept in git history: `git show 78f60ab8:components/memory-tier/.specify/sync/proposals.md`.)

All paths are relative to the repository root unless noted. `spec.md` = `components/memory-tier/specs/001-memory-tier/spec.md`; `lib.rs` / `allocator.rs` = `components/memory-tier/src/…`.

## Summary

| Resolution Type | Count | Proposals |
|-----------------|-------|-----------|
| Backfill (Code → Spec) | 6 | P1, P2, P5, P6, P7, P8 |
| Align (Spec → Code) | 2 | P3, P4 |
| Human Decision | 0 standalone (P1 and P9 each contain a decision point) | — |
| New Specs (NEW_SPEC) | 1 | P9 |
| Supersede | 1 | P10 |
| Remove from Spec | 0 | — |
| **Total** | **10** | |

Suggested order: P1 + P2 together (the fix on this branch), then P3/P4 (each needs a failing test first), then the wording backfills P5–P8, then P9 and P10.

---

### Proposal P1: 001-memory-tier / U1 — refused `track` in `insert` (new FR-030)

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · `approved: false`

**Requirement**: the error path added by this branch's fix (`21dbc787`) is described nowhere in the spec.

**What the spec says**:
- `spec.md:157-159`: "FR-008 | insert() rejects zero size with InvalidSize", "FR-009 | insert() rejects duplicate keys with AlreadyExists", "FR-010 | insert() returns PoolFull when the allocator cannot satisfy the request". No requirement covers the eviction policy refusing to track a key.
- `spec.md:38-41`: the US1 Test Coverage list does not include the new regression test.

**What the code does**:
- `lib.rs:360-364` checks for room first (`can_allocate`, `allocator.rs:36-43`, same predicate as `allocate` `:47-53`) and returns `PoolFull` before contacting the policy.
- `lib.rs:366-372`: `ep.track(...)` errors are mapped to `MemoryTierError::NotInitialized(format!("eviction policy refused to track key {key}: {e}"))`. Nothing has been allocated or recorded at that point.
- `lib.rs:374-378`: allocation after `track`, plus a rollback that is unreachable under the same write guard.
- Test `src/repair_test_insert_track_failure.rs:21-52`: no panic, `is_err`, `used()` unchanged, `!contains(2)`, and a later insert succeeds.

**Exact spec change** (`spec.md:178`, add a row after FR-029):

Old:
```
| FR-029 | `free_capacity()` returns `capacity() - used()` for proactive-eviction triggers | Implementation *(backfilled)* |
```
New:
```
| FR-029 | `free_capacity()` returns `capacity() - used()` for proactive-eviction triggers | Implementation *(backfilled)* |
| FR-030 | insert() checks for free space (FR-010, `PoolFull`) before registering the key with the eviction policy. If the policy then refuses to track the key (`IEvictionPolicy::track` returns an error), insert() returns `NotInitialized` with a message naming the key and the policy's error, reserves no pool space, records no slot, does not panic, and leaves the component usable | Unit test *(backfilled)* |
```

`spec.md:41`: add a Test Coverage line after it.

Old:
```
- `peek_does_not_update_eviction_order` - verifies peek does not update eviction order
```
New (to be combined with P8, which rewrites the first line):
```
- `peek_does_not_update_eviction_order` - verifies peek does not update eviction order
- `insert_fails_cleanly_when_policy_refuses_to_track` - verifies that a refused `track` makes insert fail without a panic, reserved space, or a recorded slot, and that the component stays usable (FR-030)
```

**Rationale**: this is the intended, tested behaviour of the fix. The spec should record it, including the ordering (PoolFull is decided before the policy is contacted).

**Decision point (does not block approval of this text)**: the variant `NotInitialized` is misleading on an initialized pool, and `dispatcher` passes it through as `AllocationFailed("not initialized: …")` (`components/dispatcher/src/lib.rs:1219-1221`). If a dedicated variant (e.g. `EvictionPolicyRefused(CacheKey)`) is preferred, that becomes an ALIGN change to `components/interfaces/src/imemory_tier.rs:20-35` and `lib.rs:368-371`, and FR-030 names that variant instead.

---

### Proposal P2: interfaces / D2 — `IMemoryTier::insert` and `NotInitialized` doc comments

**Type**: BACKFILL (Code → interface doc) · **Confidence**: HIGH · `approved: false`

**Requirement**: the `IMemoryTier` contract doc (in this component's sync hash) should list the errors `insert` actually returns.

**What the spec/interface says**:
- `components/interfaces/src/imemory_tier.rs:66`: "Returns `PoolFull` if insufficient contiguous space is available."
- `components/interfaces/src/imemory_tier.rs:33`: "Component not initialized."

**What the code does**: `insert` returns `InvalidSize` (`lib.rs:329-331`), `NotInitialized("pool not initialized")` (`:334-338`), `AlreadyExists` (`:356-358`), `PoolFull` (`:362-364`) and `NotInitialized("eviction policy refused to track key …")` (`:366-372`).

**Exact change**:

`imemory_tier.rs:66` old:
```
        /// Returns `PoolFull` if insufficient contiguous space is available.
```
New:
```
        /// Errors: `InvalidSize` if `size` is zero; `NotInitialized` if the
        /// pool is not initialized, or if the bound eviction policy refuses to
        /// track `key` (the message says which); `AlreadyExists` if `key`
        /// already has a slot; `PoolFull` if insufficient contiguous space is
        /// available. On every error no space is reserved and no slot is
        /// recorded.
```

`imemory_tier.rs:33` old:
```
    /// Component not initialized.
```
New:
```
    /// Component not initialized, or a required dependency (the bound
    /// eviction policy) refused the operation; the message says which.
```

**Rationale**: documentation only, matching the code. If P1's dedicated-variant option is chosen, drop the second change and document the new variant instead.

---

### Proposal P3: 001-memory-tier / FR-013 — `evict_next` picks the victim outside the pool lock

**Type**: ALIGN (Spec → Code) · **Confidence**: MEDIUM (PLAUSIBLE; found by reading the code, not reproduced) · `approved: false`

**Requirement**: FR-013.

**What the spec says** (`spec.md:162`): "evict_next() delegates victim selection to the eviction policy (`identify_next_to_evict(pool_id)`), then removes that slot and frees its allocation; there is no shard round-robin counter".

**What the code does** (`lib.rs:445-477`):
1. `ep.identify_next_to_evict(state.pool_id)` at `:452` pops the victim's LRU node, holding only the outer `state` read lock. The `pool` write lock is taken later, at `:465`.
2. If a concurrent `remove(key)` got there first, `pool.slots.remove(&key)` yields `None`. Nothing is freed, yet `Some(key)` is returned (`:473`) and `evictions` is bumped (`:470-471`, outside the `if let`).
3. The popped index is recycled (`eviction-policy-lru/src/lru_list.rs:143` → `:38-46`), and `EvictionHandle` has no generation counter. An `insert(J)` in the window can receive K's old index. `remove(K)` (`lib.rs:511`) then removes **J's** node, and J can no longer be evicted.

**Proposed code change** (spec stays as is):
- Take the `pool` write guard **before** calling `identify_next_to_evict`, so that selection, slot removal and deallocation happen under one guard. The lock order stays `pool` → policy, the same as `insert`/`remove`/`clear`, so no new deadlock is introduced.
- Return `Some(key)` and bump `evictions` only when a slot was actually removed. If the policy returns a key without a slot, loop to the next victim or return `None`.
- Task: first add a test that reproduces it. Interleave `evict_next` with `remove(K)` + `insert(J)` (a test-only hook, or a policy wrapper that pauses inside `identify_next_to_evict`). It should show J missing from `oldest_keys(usize::MAX)` while `contains(J)` is true. Then apply the fix.

**Rationale**: the spec states the correct contract; the code breaks it under concurrency. Rewriting the spec to "may return a key it did not free" would document a bug.

---

### Proposal P4: 001-memory-tier / SC-3 — touches after dropping the pool lock race with `clear`

**Type**: ALIGN (Spec → Code) · **Confidence**: MEDIUM (PLAUSIBLE; found by reading the code, not reproduced) · `approved: false`

**Requirement**: SC-3. NFR-002 is related.

**What the spec says**:
- `spec.md:231`: "Concurrent access from 16+ threads does not deadlock or corrupt state".
- `spec.md:185` (NFR-002): "data-path touches are applied outside the pool lock via the eviction policy's own synchronization".

**What the code does**: `get` (`lib.rs:417-419`), `touch` (`:525-527`) and `batch_touch` (`:561-565`) copy the handle, `drop(pool)`, then call the policy. A concurrent `clear` (`lib.rs:613-617`) calls `ep.clear_pool`, which calls `LruList::clear()` and empties `nodes` (`lru_list.rs:147-153`). The stale handle then reaches `move_to_back(idx)`, which indexes `self.nodes[idx]` unchecked (`lru_list.rs:70-71`). That is an out-of-bounds **panic** while holding the policy's per-pool `Mutex` (`eviction-policy-lru/src/lib.rs:84-85`), which poisons it, so every later policy call on that pool panics. No multi-threaded test exists (`tasks.md:20` is still open).

**Proposed code change** (pick one; the spec stays as is, and NFR-002's wording changes only for option a):
- (a) In `get`/`touch`/`batch_touch`, call the policy while still holding the `pool` **read** guard. `clear` holds the write guard across `clear_pool`, so a stale handle can no longer reach it. Cost: the policy's mutex is taken inside the pool read lock (same order as `insert`), and NFR-002 must change from "outside the pool lock" to "under the shared pool lock".
- (b) Make the policy robust to stale handles: bounds-check `idx` in `move_to_back`/`remove` (`lru_list.rs:70,121`) and add a generation counter to `EvictionHandle` that is checked on every use. This also fixes P3's ABA. It is a change in `eviction-policy-lru` + `interfaces`, outside this component.
- Task: add the 16-thread mixed `insert`/`get`/`touch`/`evict_next`/`remove`/`clear` stress test from `tasks.md:20`. Assert no panic, and that `used()` equals the sum of the aligned sizes of `contains`-true keys at quiescence.

**Rationale**: SC-3 is the agreed contract, and a panic that poisons the policy breaks it. Severity is major, likelihood is low (`clear` racing a hot read).

---

### Proposal P5: 001-memory-tier / FR-023 — "All operations check initialized flag"

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · `approved: false`

**What the spec says** (`spec.md:172`): "FR-023 | All operations check initialized flag before proceeding".

**What the code does**: the check is present in 14 operations (`lib.rs:334,394,425,437,447,485,518,536,570,580,589,598,607,182`). Exceptions:
- `insert` returns `InvalidSize` for size 0 before the check (`:329-331`).
- `batch_touch([])` returns early (`:532`).
- `is_dma_capable` (`:621-624`) and `telemetry_snapshot` (`:626-646`) never check it.
- `initialize` checks it inverted (`:271`).

All exceptions are benign.

**Exact spec change** (`spec.md:172`):

Old:
```
| FR-023 | All operations check initialized flag before proceeding | Implementation |
```
New:
```
| FR-023 | Every operation that reads or changes pool state checks the initialized flag first, and returns its "not initialized" result when the flag is unset (`NotInitialized` for `insert`/`remove`/`clear`; `None`, `false`, `0`, an empty `Vec`, or a no-op for the others). Exceptions: `insert` rejects `size == 0` with `InvalidSize`, and `batch_touch` returns on an empty slice, before the check; `is_dma_capable()` and `telemetry_snapshot()` do not check the flag (their results are well-defined before initialization); `initialize()` rejects an already-set flag | Implementation |
```

**Rationale**: the behaviour is intended and harmless; the spec's "All" is just too strong.

---

### Proposal P6: 001-memory-tier / FR-027 — telemetry instrumentation is partial

**Type**: BACKFILL (Code → Spec) · **Confidence**: MEDIUM · `approved: false`

**What the spec says**:
- `spec.md:176`: "FR-027 | When compiled with the optional `telemetry` feature, the component tracks eviction count, write-lock contention count, and read-lock contention count".
- `spec.md:239`: "When the `telemetry` feature is enabled, a failed `try_read`/`try_write` first attempt bumps the corresponding lock-contention counter before blocking."

**What the code does**:
- `try_write` + counter only in `insert` (`lib.rs:342-352`), `evict_next` (`:453-463`) and `remove` (`:493-503`); `clear` uses plain `write()` (`:613`).
- `try_read` + counter only in `get` (`:400-410`) and `batch_touch` (`:544-554`); `peek` `:429`, `touch` `:523`, `contains` `:574`, `capacity` `:583`, `used` `:592` and `free_capacity` `:185` use plain `read()`.
- `evictions` is bumped whenever the policy returns a victim (`:470-471`).

**Exact spec change**:

`spec.md:176` old:
```
| FR-027 | When compiled with the optional `telemetry` feature, the component tracks eviction count, write-lock contention count, and read-lock contention count | Implementation *(backfilled)* |
```
New:
```
| FR-027 | When compiled with the optional `telemetry` feature, the component tracks eviction count (one per `evict_next` call for which the policy returned a victim), write-lock contention count (measured in `insert`, `remove`, `evict_next`), and read-lock contention count (measured in `get`, `batch_touch`); other lock acquisitions (`clear`, `peek`, `touch`, `contains`, `capacity`, `used`, `free_capacity`) are not measured | Implementation *(backfilled)* |
```

`spec.md:239`: in that bullet, replace this sentence.

Old:
```
When the `telemetry` feature is enabled, a failed `try_read`/`try_write` first attempt bumps the corresponding lock-contention counter before blocking.
```
New:
```
When the `telemetry` feature is enabled, the measured paths (`insert`, `remove`, `evict_next` for the write lock; `get`, `batch_touch` for the read lock) first try `try_write`/`try_read` and, if that fails, bump the corresponding lock-contention counter before blocking.
```

**Rationale**: instrumenting only the hot paths looks deliberate. If P3 is applied, change "for which the policy returned a victim" to "that freed a slot".

---

### Proposal P7: 001-memory-tier / NFR-005 — first-fit search is linear, not O(log n)

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · `approved: false`

**What the spec says** (`spec.md:188`): "NFR-005 | Allocator uses BTreeMap for O(log n) first-fit search".

**What the code does**: `allocate` does `self.free_regions.iter().find(|(_, &s)| s >= aligned_size)` (`allocator.rs:53`), and `can_allocate` does `self.free_regions.values().any(|&s| s >= aligned_size)` (`:42`). Both are linear scans in offset order. Only `remove`/`insert`/`range`/`get` on the map are O(log k) (`:55,59,76,80,86-88,91`). `insert` now runs both scans (`lib.rs:362,375`).

**Exact spec change** (`spec.md:188`):

Old:
```
| NFR-005 | Allocator uses BTreeMap for O(log n) first-fit search | Implementation |
```
New:
```
| NFR-005 | Allocator keeps free regions in a BTreeMap keyed by offset: map updates and coalescing lookups are O(log k), and the first-fit search (`allocate`, `can_allocate`) is a linear scan, O(k), over the k free regions in offset order | Implementation |
```

**Rationale**: this states the real complexity. If O(log k) search is actually wanted (e.g. a size-indexed map), that would be a separate ALIGN with a Criterion benchmark, per CLAUDE.md.

---

### Proposal P8: 001-memory-tier / D1 — US1 `peek` test-coverage claim

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · `approved: false`

**What the spec says** (`spec.md:41`): "`peek_does_not_update_eviction_order` - verifies peek does not update eviction order".

**What the code does**: the test (`lib.rs:778-785`) inserts key 0 and asserts only `mt.peek(0).is_some()`; it never compares eviction order. FR-012 itself holds by inspection (`peek` makes no policy call, `lib.rs:423-433`).

**Exact spec change** (`spec.md:41`):

Old:
```
- `peek_does_not_update_eviction_order` - verifies peek does not update eviction order
```
New:
```
- `peek_does_not_update_eviction_order` - verifies peek returns the entry; it does not yet check that eviction order is unchanged (FR-012 holds by inspection: `peek` makes no eviction-policy call)
```

**Rationale**: the test-coverage claim should be accurate. Alternative (ALIGN, test only): strengthen the test to insert 0 and 1, `peek(0)`, then assert `evict_next() == Some(0)`, and keep the current wording.

---

### Proposal P9: 001-memory-tier / U2 — behaviour when the `eviction_policy` receptacle is disconnected

**Type**: NEW_SPEC (new requirement; needs a human decision before it is written into the spec) · **Confidence**: LOW (decision needed) · `approved: false`

**What the spec says**:
- `spec.md:173`: "FR-024 | Eviction policy is an external receptacle (IEvictionPolicy)".
- `spec.md:210`: "`IEvictionPolicy` | Receptacle | External eviction strategy (required)".
- Nothing about what happens if it is disconnected after `initialize`.

**What the code does**: `insert` `lib.rs:340`, `get` `:398`, `oldest_keys` `:441`, `evict_next` `:451`, `remove` `:491`, `touch` `:522` and `clear` `:612` call `self.eviction_policy.get().unwrap()`, which **panics** when not connected. `batch_touch` `:539-542` returns silently. `initialize` returns `NotInitialized("eviction_policy receptacle not connected")` (`:266-268`). Disconnecting is possible via `Receptacle::disconnect` (`lib/component-framework/crates/component-core/src/receptacle.rs:110-117`). This is the same class of panic that this branch removed for `track`.

**Draft requirement** (option A, recommended):
```
| FR-031 | If the `eviction_policy` receptacle is not connected when an operation runs, the operation does not panic: `insert`/`remove`/`clear` return `NotInitialized("eviction_policy receptacle not connected")` without changing pool state; `get`/`peek`/`evict_next`/`evict_next_for_key`/`oldest_keys`/`touch`/`batch_touch` return `None`, an empty `Vec`, or do nothing | Unit test |
```
Option B: document that the receptacle must stay connected for the component's lifetime, and that disconnecting it after `initialize` is a programming error that panics.

**Questions to decide**: Is re-binding the eviction policy at runtime a supported operation (the new regression test's comment suggests it is considered possible, `repair_test_insert_track_failure.rs:30-32`)? If option A, the code needs a follow-up ALIGN (replace the seven `unwrap()`s) plus a test.

---

### Proposal P10: 001-memory-tier / plan.md + tasks.md — sharded design superseded

**Type**: SUPERSEDE · **Confidence**: HIGH · `approved: false`

**What the spec says**:
- `spec.md:247-253`: "Phase B decision: **backfill this spec to match the working implementation.** … the sharded/formally-verified design described in earlier revisions was never built."
- FR-005/006/007/013/014/021 and NFR-002 describe the single `RwLock<Pool>`.

**What plan.md/tasks.md say**:
- `plan.md:8,24-48,60,65,75,121,142-149,169,175` describe 16 shards, key % 16, per-shard `Mutex`, an `evict_counter` round-robin and Creusot P4/P5.
- `plan.md:184` says the sharded text was left "pending a decision on whether sharding is unfinished work or was intentionally dropped".
- `tasks.md:16` ("Verify Creusot verification conditions still discharge"), `:24` ("Add test for evict_next_for_key targeting correct shard"), `:37` ("Add architecture diagram showing shard layout and pointer arithmetic") and `:43` ("Evaluate configurable shard count based on hardware thread count") refer to things that do not exist.

**What the code does**: a single `RwLock<Pool>` (`lib.rs:76-85`), `evict_next_for_key` is an alias (`:479-481`), and there is no `verif/` directory.

**Exact change**:

`plan.md:184` old:
```
- The 16-way sharding design described throughout this plan (Memory Layout, Pointer Arithmetic, Concurrency Model, Key Design Decision #1 and #6) does **not** match the current single-pool implementation. This plan has been left as-is (describing the original sharded design) rather than rewritten, pending a decision on whether sharding is unfinished work or was intentionally dropped. See `.specify/sync/align-tasks.md` ("sharding-not-implemented") before treating this plan's architecture sections as current.
```
New:
```
- **SUPERSEDED (2026-08-20, Spec-Sync Phase B).** The 16-way sharding design described throughout this plan (Memory Layout, Pointer Arithmetic, Concurrency Model, Key Design Decision #1 and #6, and the Creusot P4/P5 properties) was never built and has been dropped. The single `RwLock<Pool>` implementation is authoritative, and `spec.md` (FR-005/006/007/013/014/021, NFR-002) describes it. Treat those sections of this plan as history, not as the current design.
```

In `tasks.md`, replace `- [ ]` with `- [~] (superseded 2026-08-20: no shards / no Creusot artifacts)` on the four lines. Example, `tasks.md:24` old:
```
- [ ] Add test for evict_next_for_key targeting correct shard
```
New:
```
- [~] (superseded 2026-08-20: no shards / no Creusot artifacts) Add test for evict_next_for_key targeting correct shard
```
(and the same prefix on `tasks.md:16`, `:37`, `:43`). Also, `tasks.md:13` "all 12 unit tests in lib.rs" → "all 12 unit tests in lib.rs and the 1 regression test in `repair_test_insert_track_failure.rs`".

**Rationale**: the decision the plan says is pending was taken on 2026-08-20. Leaving the plan as is keeps an inter-spec conflict open.
