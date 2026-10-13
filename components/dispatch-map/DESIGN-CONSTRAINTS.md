# dispatch-map: design constraints

This file gives the rules of `dispatch-map` that other components depend on.
If a change breaks one of these rules, data can become incorrect without an error, or vLLM can stop.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

Spec: `specs/001-dispatch-map/spec.md` (cited as "spec").

## DMAP-1. Keep one global lock for the map

- **Rule:** Keep all entries behind the one `Mutex<Inner>` and its `Condvar`. Do not divide the map into shards.
- **Reason:** Eviction and the Check→Pin sequence of the connector must have one view of all entries.
- **Applies to:** `src/state.rs`, `src/lib.rs`.
- **Status:** Established by spec (FR-002, FR-013) and by code.
- **Evidence:** spec line 8 and FR-013 (line 252); `src/state.rs:17-21`; commit `8f494f8d` (2026-09-01).
- **Rejected designs:**
  - **A map with 64 shards (with a sharded `memory-tier`).** The gain was +2.2%, inside the noise. It added two defects that can stop vLLM. First, a retry after `PoolFull` evicted from the wrong shard, which caused `AllocationFailed` and a vLLM crash. Second, per-shard locks exposed a race between Check and Pin. Commit `8f494f8d` removed the shards. Evidence: spec line 8.

## DMAP-2. Wait and take the reference in one lock hold

- **Rule:** In `take_read`, `take_write` and `lookup`, keep the lock from the time that the wait condition is true. Release it only after the reference count changes.
- **Reason:** `take_write` sets `write_ref = 1`. If the code releases the lock between the check and the change, two writers can get the same key. Nothing shows the error.
- **Applies to:** `src/state.rs:54-78` and each new method that waits for a reference.
- **Status:** Established by code. The spec states the waits (FR-006, FR-007) but not the single lock hold.
- **Evidence:** `src/state.rs:54-78` (`wait_for` returns the guard); spec lines 245-246.

## DMAP-3. Use a memory-tier pointer only when you hold a read reference

- **Rule:** Hold a read reference on the key for all the time that you use a pointer from `lookup`. Free a memory-tier slot only after `try_evict_to_block` or `remove` returns `Ok`.
- **Reason:** `memory-tier` has no pins. `try_evict_to_block` refuses an entry with a read or write reference, so the reference is the only protection of the slot.
- **Applies to:** `dispatcher`, `shmq-dispatcher` and each user of `lookup`.
- **Status:** Established by spec (FR-011, FR-026).
- **Evidence:** spec lines 250 and 265; `src/lib.rs:540-555`.

## DMAP-4. Only the write-through job calls `convert_to_storage`

- **Rule:** Call `convert_to_storage` only from the job that holds the write-through read reference. Call it only after the data and the extent are on the SSD.
- **Reason:** `convert_to_storage` sets `ssd_offset`, which makes the entry evictable. It also decreases `read_ref` by one. A different caller releases the reference of a different user.
- **Applies to:** `dispatcher` write-through (`process_write_job`).
- **Status:** Established by spec (FR-005).
- **Evidence:** spec line 244; `src/lib.rs:180-186`; caller `components/dispatcher/src/lib.rs:1463-1464`.

## DMAP-5. Promote an entry in place

- **Rule:** To move an entry from `BlockDevice` to `MemoryTier`, use `promote_block_to_memory_tier`. Do not remove the entry and create it again.
- **Reason:** In-place promotion keeps the read references and the eviction handle. A load that is in progress keeps its pin.
- **Applies to:** the dispatcher promotion paths.
- **Status:** Established by spec (FR-025). Note: `convert_memory_tier_to_block` does not examine the references, so its caller must hold the write reference (spec FR-018 does not say this).
- **Evidence:** spec line 264; `src/lib.rs:478-494`; `src/lib.rs:426-440`.

## DMAP-6. The map is not stored on the drive. The dispatcher builds it again at restart

- **Rule:** At restart, put each extent into the map with `recover_extent(key, offset, size_blocks)`. Give the size in 4 KiB blocks. Do not add a key two times.
- **Reason:** The extent managers are the only stored index. Recovered entries are `BlockDevice` entries without references. A restart loses the DRAM contents.
- **Applies to:** `dispatcher::initialize`, `recover_extent`, `initialize()`.
- **Status:** Established by spec (FR-020, FR-024). **Code and spec disagree:** the dispatcher gives the size in bytes (see KD-5 in the main document). Also, the `initialize()` walk replaces an entry that already exists, but `recover_extent` refuses it.
- **Evidence:** spec lines 259 and 263; `src/lib.rs:567-590` (`recover_extent`, refuses duplicates); `src/lib.rs:94-106` (`initialize()` uses `insert`); `src/lib.rs:369` (`size_blocks * 4096`); `components/dispatcher/src/lib.rs:1922`.

## DMAP-7. Lock order: the map lock first, then the eviction policy

- **Rule:** You can call the eviction policy when you hold the map lock. Do not take the map lock from the eviction policy.
- **Reason:** The map calls `track` and `remove` on the eviction policy when it holds the map lock. A call in the opposite direction can cause a deadlock.
- **Applies to:** `dispatch-map`, `eviction-policy-lru` and each new `IEvictionPolicy` implementation.
- **Status:** Established by code.
- **Evidence:** `src/lib.rs:86-92` (`track` with the lock held); `src/lib.rs:150-156` (`touch` after `drop(inner)`).
