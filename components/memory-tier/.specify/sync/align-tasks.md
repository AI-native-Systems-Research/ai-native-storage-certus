# Alignment Tasks — memory-tier

**Regenerated**: 2026-10-14 (spec-sync apply, branch `fix/memory-tier-insert-track-panic`, base `78f60ab8`)
**Source**: `.specify/sync/proposals.json` (generated 2026-10-14T16:47:49Z), approved by Cornel Constantinescu, 2026-10-14

This apply step changed no code. The ALIGN proposals below are recorded as tasks only.
The spec text for FR-013 and SC-3 stays as it is: the spec is correct, and the code must change to match it.

## ALIGN tasks (code violates a correct spec): 2

## Task: Align 001-memory-tier/FR-013 (proposal P3)

**Spec Requirement**: FR-013 (`spec.md:162`): "evict_next() delegates victim selection to the eviction policy (`identify_next_to_evict(pool_id)`), then removes that slot and frees its allocation; there is no shard round-robin counter"
**Verdict**: PLAUSIBLE. Found by reading the code; not reproduced.
**Current Code**: `evict_next` (`src/lib.rs:445-477`) pops the victim from the policy (`:452`) while holding only the outer `state` read lock. It takes the `pool` write lock later (`:465`). If a `remove(key)` runs in between, `pool.slots.remove(&key)` returns `None` and nothing is freed. Even so, `evict_next` returns `Some(key)` (`:473`) and bumps `evictions` (`:470-471`). The popped LRU index is recycled (`eviction-policy-lru/src/lru_list.rs:143` → `:38-46`), and `EvictionHandle` has no generation counter. So an `insert(J)` inside the window can receive K's old index. A later `remove(K)` (`lib.rs:511`) then removes J's node, and J can never be evicted.
**Required Change**: Take the `pool` write guard before calling `identify_next_to_evict`, so that victim selection, slot removal and deallocation all happen under one guard. The lock order stays `pool` → policy, as in `insert`/`remove`/`clear`. Return `Some(key)` and count an eviction only when a slot was actually removed. If the policy returns a key that has no slot, either loop to the next victim or return `None`.
**Files to Modify**: `components/memory-tier/src/lib.rs` (`evict_next`, about `:445-477`); a new test in `components/memory-tier/src/`
**Estimated Effort**: medium

### Acceptance Criteria
- [ ] A test that reproduces the race is added first and fails on the current code. It interleaves `evict_next` with `remove(K)` + `insert(J)`, using a test-only hook or a policy wrapper that pauses inside `identify_next_to_evict`. It shows J missing from `oldest_keys(usize::MAX)` while `contains(J)` is true.
- [ ] `evict_next` selects the victim and removes its slot under one `pool` write guard, and the test passes.
- [ ] `evict_next` returns `Some(key)`, and bumps the `evictions` counter, only when it freed a slot.
- [ ] Follow-up spec edit: once this lands, change FR-027's "one per `evict_next` call for which the policy returned a victim" to "one per `evict_next` call that freed a slot" (note on proposal P6).

## Task: Align 001-memory-tier/SC-3 (proposal P4; NFR-002 related)

**Spec Requirement**: SC-3 (`spec.md:231`): "Concurrent access from 16+ threads does not deadlock or corrupt state". Related: NFR-002 (`spec.md:185`).
**Verdict**: PLAUSIBLE. Found by reading the code; not reproduced.
**Current Code**: `get` (`src/lib.rs:417-419`), `touch` (`:525-527`) and `batch_touch` (`:561-565`) copy the eviction handle, drop the `pool` lock, and then call the policy. A concurrent `clear` (`:613-617`) calls `ep.clear_pool`, which empties the LRU node vector (`eviction-policy-lru/src/lru_list.rs:147-153`). The stale handle then reaches `move_to_back(idx)`, which indexes `self.nodes[idx]` without a bounds check (`lru_list.rs:70-71`). The result is an out-of-bounds panic while the policy's per-pool `Mutex` is held (`eviction-policy-lru/src/lib.rs:84-85`). That poisons the mutex, so every later policy call on that pool panics. No multi-threaded test exists (`tasks.md:20` is still open).
**Required Change**: Pick one option.
- (a) In `get`/`touch`/`batch_touch`, call the policy while still holding the `pool` read guard. `clear` holds the write guard across `clear_pool`, so a stale handle can no longer reach the policy. The policy mutex is then taken inside the pool read lock, the same order as in `insert`. NFR-002 must then change from "outside the pool lock" to "under the shared pool lock".
- (b) Make the policy robust to stale handles: bounds-check `idx` in `move_to_back`/`remove` (`lru_list.rs:70,121`), and add a generation counter to `EvictionHandle` that is checked on every use. This also fixes P3's ABA problem. The change is in `eviction-policy-lru` and `interfaces`, outside this component.
**Files to Modify**: (a) `components/memory-tier/src/lib.rs` and `specs/001-memory-tier/spec.md` NFR-002; (b) `components/eviction-policy-lru/src/lru_list.rs`, `components/eviction-policy-lru/src/lib.rs`, `components/interfaces/src/` (`EvictionHandle`). Both options need a new stress test in `components/memory-tier/`.
**Estimated Effort**: medium for (a); large for (b), because it crosses components and an interface change re-stamps every component.

### Acceptance Criteria
- [ ] The 16-thread stress test from `tasks.md:20` is added: mixed `insert`/`get`/`touch`/`evict_next`/`remove`/`clear`.
- [ ] The test runs without a panic. At quiescence, `used()` equals the sum of the 4 KiB-aligned sizes of the keys for which `contains` is true.
- [ ] Option (a) or (b) is chosen and implemented. If (a), NFR-002 is reworded in the same change.

---

## Open items (not approved; need a decision)

### Open: P2 — `IMemoryTier::insert` and `MemoryTierError::NotInitialized` doc comments

**Status**: not approved. **Approval note**: "deferred: changes components/interfaces, which re-stamps every component; separate PR"
**What**: `components/interfaces/src/imemory_tier.rs:66` says only "Returns `PoolFull` if insufficient contiguous space is available.", and `:33` says "Component not initialized." The code returns `InvalidSize`, `NotInitialized` (pool not initialized, or the policy refused `track`), `AlreadyExists` and `PoolFull` (`src/lib.rs:329-372`). The exact replacement text is in `proposals.md`, Proposal P2.
**Next step**: a separate PR against `components/interfaces`. Keep `NotInitialized`: P1 was approved with "keep NotInitialized (no new variant)", so P2's second edit applies as written.

### Open: P9 — behaviour when the `eviction_policy` receptacle is disconnected (proposed FR-031)

**Status**: not approved. **Approval note**: "open decision (A: errors instead of panics / B: document must stay connected); record in align-tasks.md"
**What**: `insert` `src/lib.rs:340`, `get` `:398`, `oldest_keys` `:441`, `evict_next` `:451`, `remove` `:491`, `touch` `:522` and `clear` `:612` call `self.eviction_policy.get().unwrap()`, which panics when the receptacle is not connected. `batch_touch` (`:539-542`) returns silently instead. `Receptacle::disconnect` makes disconnection possible.
**Options**:
- A (recommended in the proposal): add FR-031 (draft text in `proposals.md`, Proposal P9). Then ALIGN the code by replacing the seven `unwrap()` calls, and add a test.
- B: document that the receptacle must stay connected for the component's lifetime, and that disconnecting it after `initialize` is a programming error that panics.
**Question to decide**: is re-binding the eviction policy at runtime a supported operation?

---

## History (carried over from the 2026-08-20 regeneration)

- The Phase B pass (2026-08-20) generated 0 ALIGN tasks. It superseded the five tasks deferred on 2026-07-22 (`sharding-not-implemented`, `evict-lru-for-key-ignores-key`, `creusot-proofs-absent`, `version-mismatch`, `readme-source-layout-drift`). The plan.md/tasks.md part of `sharding-not-implemented` is now closed by P10 in this apply.
- The NFR-008 version HUMAN_DECISION was resolved on 2026-09-03 (0.3.0; see `spec.md` "Spec-Sync Notes").
- The README source-layout follow-up (`lru.rs`/`LruList`) is outside spec-sync scope. `tasks.md:36` tracks it.
- The full previous file is backed up at `.specify/sync/backups/20261014T172213Z/align-tasks.md`.
