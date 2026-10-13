# memory-tier: design constraints

This file gives the rules of `memory-tier` that other components depend on.
If a change breaks one of these rules, data can become incorrect without an error, or vLLM can stop.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

Spec: `specs/001-memory-tier/spec.md` (cited as "spec").

## MT-1. Keep one pool for all keys

- **Rule:** Keep one `RwLock<Pool>` with one free list. A slot that one key frees must be available to each other key. Do not divide the pool into shards.
- **Reason:** When the pool is full, the dispatcher evicts any clean entry to get space for the new key. With shards, the evicted space can be in a different shard, so the insert fails.
- **Applies to:** `src/lib.rs`; `evict_next_for_key`.
- **Status:** Established by spec (FR-005, FR-014) and by code.
- **Evidence:** spec lines 154 and 163; `src/lib.rs:76-85`; `src/lib.rs:468-470` (the code ignores the key).
- **Rejected designs:**
  - **A sharded pool (with a sharded `dispatch-map`).** A retry after `PoolFull` evicted from the wrong shard, which caused `AllocationFailed` and a vLLM crash. The gain was +2.2%, inside the noise. Commit `8f494f8d` (2026-09-01) removed it. Evidence: `components/dispatch-map/specs/001-dispatch-map/spec.md` line 8. An earlier optimizer test of a sharded pool measured +0.3% (`targets/evolve-latency/experiments/s20261003-092109/prior_session_digest.md:59` in the workbench).

## MT-2. The memory tier has no pins. Ask the dispatch map first

- **Rule:** Call `remove`, `evict_next` or `clear` on a slot only when the dispatch map permits it. The dispatch map permits it when `try_evict_to_block` or `remove` returns `Ok`. If no other user can have the pointer, you can also free the slot.
- **Reason:** These methods free the slot immediately and do not examine any reference. Only the dispatch-map references protect a slot that a reader uses (see DMAP-3).
- **Applies to:** each caller of `IMemoryTier`.
- **Status:** Established by code. The interface documentation states that the pointer stays usable until a call evicts or removes the slot.
- **Evidence:** `components/interfaces/src/imemory_tier.rs:63-66`; `src/lib.rs:494-501` (`remove`); `src/lib.rs:600-606` (`clear`).

## MT-3. Lock order: the pool lock first, then the eviction policy

- **Rule:** You can call `track` on the eviction policy when you hold the pool lock. Call `touch` only after you release the pool lock. Do not call the memory tier from the eviction policy.
- **Reason:** The eviction policy has its own lock. A call in the opposite direction can cause a deadlock.
- **Applies to:** `src/lib.rs`, each `IEvictionPolicy` implementation.
- **Status:** Established by spec (NFR-002).
- **Evidence:** spec line 185; `src/lib.rs:360-367` (`track` under the write lock); `src/lib.rs:405-408` (`touch` after `drop(pool)`).

## MT-4. The memory tier must live longer than all pointers into it

- **Rule:** Do not drop the memory-tier component when the dispatch map can still have `MemoryTier` entries that point into the pool. Do not call `clear` at that time.
- **Reason:** `Drop` frees the pool memory, and `clear` resets the allocator. A pointer that stays in the dispatch map then points at free or reused memory.
- **Applies to:** server shutdown, `clear_memory_tier` in the dispatcher.
- **Status:** Established by code. The spec states only the free on `Drop` (NFR-006).
- **Evidence:** `src/lib.rs:114-136`; `src/lib.rs:600-606`; spec line 189.
