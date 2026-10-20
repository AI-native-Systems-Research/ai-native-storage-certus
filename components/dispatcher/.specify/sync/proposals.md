# Drift Resolution Proposals — dispatcher

Generated: 2026-10-14 (sync date; analysis run 2026-10-20) · Branch: `fix/dispatcher-remove-clear-mt` (HEAD `cccfb328e`, base `origin/unstable` `af221dd82`) · Mode: **batch**, no questions asked
Based on: `drift-report.md` / `drift-report.json` of the same run. The finding ids D-xx, P-xx and U-xx refer to that report.

**Nothing has been applied.** Every proposal is `approved: false`. No spec, source or other file was edited. Paths are relative to `components/dispatcher/`. `001` means `specs/001-dispatcher-cache-interface/spec.md`. Every BACKFILL "old text" is copied verbatim from the current file, and each one was confirmed to occur exactly once.

**Id scheme.** This run's ids are single-digit (**B-1…B-6**, **A-1…A-4**). The two-digit ids (B-01…, A-01…A-08, N-01…N-04) belong to the 2026-10-14 `fix/bundle-dispatcher-ems` run and live in `align-tasks.md`. They are referred to, not re-proposed.

## Summary

| Resolution Type | Count | Ids |
|-----------------|-------|-----|
| Backfill (Code → Spec) | 6 | B-1 … B-6 |
| Align (Spec → Code) | 4 | A-1 … A-4 (A-3 needs a contract decision first; A-4 is LOW confidence) |
| Human Decision | 0 as a separate type (the decision is inside A-3) |
| New Specs | 0 (the unspecced code U-06, U-07 and U-04 is folded into B-1, B-2, B-3, B-5 and B-6; U-01 stays open as N-01) |
| Supersede | 0 |
| Remove from Spec | 0 |
| **Total** | **10** | |

**Priority:**
- **A-1 is major.** A write-through scheduled for a remote-fetched key consumes a concurrent reader's pin.
- **Apply B-1…B-6 in this branch's PR**, so the next stamp certifies both formal-verification repairs.
- **After B-2/B-3 land, close A-03 and N-04** in `align-tasks.md`. The code now meets A-03's acceptance criteria (`src/repair_test_clear_mt_pinned.rs:599,621,644`), and B-2/B-5/B-6 specify the `remove` result that N-04 asked about.

Still open from the earlier run, re-verified and unchanged (not re-proposed): A-01, A-02, A-04, A-05, A-06, A-07 (decision), A-08, N-01.

---

## BACKFILL (Code → Spec)

### B-1 · 001/FR-018 — `remove` takes over the pin of a pending write-through
- **Type**: BACKFILL · **Finding**: correction to FR-018, U-06 · **Confidence**: HIGH · `approved: false`
- **Requirement**: FR-018 (`remove` does not wait for write-through).
- **Spec says** (`001:297`): "The `remove(key)` method does NOT block waiting for background write-through to complete. It proceeds immediately with removal."
- **Code does**:
  - `copy_gpu_to_memory_completed` records the pin `downgrade_reference` leaves for the job, under the ledger lock, before the job is enqueued (`src/lib.rs:3677`, `:3684-3695`; `WriteThroughPins::pin` `src/background.rs:288-293`).
  - `remove` handles `ActiveReferences` with `WriteThroughPins::remove_entry` (`src/lib.rs:3203-3212`). That releases the recorded pin and removes the entry. If a reader still holds the key, it re-takes the pin and refuses (`src/background.rs:335-356`).
  - A queued job is then dropped (`JobStart::Stale`, `src/background.rs:301-305`; `src/lib.rs:1418`).
  - A running job frees the slot and the extent it wrote (`src/lib.rs:1425-1437`). `remove` skips its own `mt.remove` in that case (`:3214-3218`).
  - Before commit `0236a3c98`, `remove` returned `KeyNotFound` and left the entry cached (base `src/lib.rs:3151-3152`).
  - Tests: `remove_proceeds_while_write_through_pending`, `key_can_be_stored_again_after_remove_with_write_through_pending` (`src/repair_test_remove_no_wait.rs:465,485`).
- **Rationale**: this repair was found by Kani/Loom and accepted by the independent check. The spec states the goal but not the ownership rules that make it safe: who releases the pin, and who frees the slot. Those are the properties the repair proves.
- **Old** (`001:297`):
  > The `remove(key)` method does NOT block waiting for background write-through to complete. It proceeds immediately with removal.
- **New**:
  > The `remove(key)` method does NOT block waiting for background write-through to complete. It proceeds immediately with removal. *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt` — code authoritative, commit `0236a3c98`. Before it, a `remove` that came before the key's write-through had run returned `KeyNotFound` and left the entry cached, because the dispatch map refused the read pin the write-through holds.)* The dispatcher MUST record, per key, the read pin that `copy_gpu_to_memory_completed` leaves on the entry for its background write-through (`WriteThroughPins`). The record MUST be made before the job is enqueued, so the job cannot start first. When `dm.remove` refuses with `ActiveReferences` and that recorded pin is held, `remove` MUST take the pin over: it releases the pin and removes the dispatch-map entry. If another reader still holds the key, the pin is given back and `remove` is refused. Once the entry has been removed this way, ownership is as follows. A job still in the queue is dropped when it is dequeued. A job already reading the DRAM slot finishes, then frees the slot and the extent it has just written, since nothing references either any more. In that case `remove` MUST NOT free the slot itself. A job whose pin was not taken over settles normally (`convert_to_storage`, or `release_read` if the write failed).

### B-2 · 001 edge case "When remove is called for a key …" — what refuses `remove` now
- **Type**: BACKFILL · **Finding**: D-10′, U-04 (resolves N-04) · **Confidence**: HIGH · `approved: false`
- **Requirement**: Edge case on `remove` ordering and refusal (`001:233`).
- **Spec says** (`001:233`): "…and it is refused while the entry is pinned or write-locked. Then the memory-tier slot is freed, then the SSD extent (if any). A remove that is refused because the entry is pinned returns `KeyNotFound` and frees nothing."
- **Code does**:
  - The write-through's own pin no longer refuses `remove` (`src/lib.rs:3207-3210`). A reader's or client's pin still does (`src/background.rs:343-346` → `None` → `KeyNotFound`).
  - A write lock makes the initial `dm.lookup` wait. Its timeout is reported as `KeyNotFound` (`src/lib.rs:3189-3199`).
  - When the job is mid-copy, it frees the slot (`src/lib.rs:3214-3218`, `:1432`).
- **Rationale**: the sentence is now false for the most common pin, the write-through's own. N-04 was deferred to "the remove-no-wait PR", and this is that PR.
- **Old** (`001:233`):
  > it is refused while the entry is pinned or write-locked. Then the memory-tier slot is freed, then the SSD extent (if any). A remove that is refused because the entry is pinned returns `KeyNotFound` and frees nothing.
- **New**:
  > it is refused while the entry is pinned or write-locked. The one exception is the pin of the key's own pending write-through, which `remove` takes over instead (FR-018). Then the memory-tier slot is freed, then the SSD extent (if any). If the write-through job is reading the slot at that moment, the job frees the slot when it is done instead. A `remove` that is refused because a reader or a client `pin` holds the entry, or because the entry stays write-locked past the dispatch-map wait, returns `KeyNotFound` and frees nothing. *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt` — code authoritative, commit `0236a3c98`.)*

### B-3 · 001/FR-038 `clear_memory_tier` — pinned entries keep their slot; exact count; `Timeout`
- **Type**: BACKFILL · **Finding**: D-06, U-07 · **Confidence**: HIGH · `approved: false`
- **Requirement**: FR-038.
- **Spec says** (`001:317`): "This method evicts ALL entries from the memory-tier pool by calling `IMemoryTier::evict_next()` in a loop until empty. For each evicted key, it transitions the dispatch-map entry to BlockDevice state via `convert_memory_tier_to_block`. If the transition fails (write-through not complete), the entry is removed from the dispatch map entirely. Returns the number of entries cleared."
- **Code does** (`src/lib.rs:3923-3987`, commit `cccfb328e`):
  - Scans `mt.oldest_keys(64 + pinned)` (`:3950`).
  - Tries `try_evict_to_block`. On `ActiveReferences` the slot is kept (`:3959`). On any other error it calls `dm.remove`, and the slot is kept if that returns `ActiveReferences` (`:3963-3966`).
  - Calls `mt.remove` and increments the count only for entries really cleared (`:3968-3971`).
  - Waits 50 µs when a pass clears nothing (`:3983`). Once 5 s have passed since the start of the call, a pass that clears nothing returns `Timeout` (`:3943-3944`, `:3976-3982`).
  - Tests: `src/repair_test_clear_mt_pinned.rs:599,621,644`.
- **Rationale**: Spin model J22 found this defect, and a Loom proof with a failing twin backs the repair. The old text names two calls the code does not use, and it allowed the dangling pointer that FR-024/FR-049 forbid.
- **Old** (`001:317`):
  > This method evicts ALL entries from the memory-tier pool by calling `IMemoryTier::evict_next()` in a loop until empty. For each evicted key, it transitions the dispatch-map entry to BlockDevice state via `convert_memory_tier_to_block`. If the transition fails (write-through not complete), the entry is removed from the dispatch map entirely. Returns the number of entries cleared.
- **New**:
  > This method empties the memory-tier pool by repeatedly scanning `IMemoryTier::oldest_keys(64 + p)`, where `p` is the number of entries left pinned on the previous pass, so that pinned entries cannot hide clearable ones behind them. For each key it first tries `IDispatchMap::try_evict_to_block` (write-through complete and unpinned → BlockDevice). If that fails for any reason other than `ActiveReferences`, the entry has no SSD copy and is removed from the dispatch map (`dm.remove`). The memory-tier slot is freed (`IMemoryTier::remove`) only after the dispatch-map entry is BlockDevice or gone. An entry that is pinned, by a lookup, a client `pin` or the write-through's read reference, MUST keep its slot and is retried on a later pass, whether `try_evict_to_block` or the `dm.remove` detects the pin: its dispatch-map entry still points at the slot, and the pin holder may be copying from it. A pass that clears nothing waits 50 µs before the next pass. Once 5 s have passed since the call started, the next pass that clears nothing MUST return `DispatcherError::Timeout` and leave the still-pinned entries resident. `Ok(n)` therefore means the tier was emptied, and `n` counts only the entries actually demoted or removed. *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt` — code authoritative, commit `cccfb328e`. Previously any failed demotion freed the slot unconditionally, so a pinned entry's dispatch-map entry pointed at released memory, the entry stayed in the map, and the count included it.)*
- **Companion edit (code-side, outside `specs/**`, not part of this BACKFILL):** the `IDispatcher::clear_memory_tier` doc comment (`components/interfaces/src/idispatcher.rs:802-806`) still says "Entries still being written are removed entirely". It should say pinned entries are kept and retried, and document the `Timeout` result.
- **Not covered by this text, deliberately:** reserved-but-not-completed slots (P-05 / A-3), whose contract is undecided.

### B-4 · 001 Assumptions — which write-through jobs hold a read reference
- **Type**: BACKFILL · **Finding**: D-18 · **Confidence**: HIGH for the description; the unpinned case is a defect (A-1) · `approved: false`
- **Requirement**: Assumption on the background writer's read reference (`001:416`).
- **Spec says** (`001:416`): "The background writer holds a read reference on each entry while write-through is in progress. The reference is released after write completes or fails."
- **Code does**:
  - A `copy_gpu_to_memory_completed` job holds the `downgrade_reference` pin (`src/lib.rs:3677`), recorded at `:3686`.
  - `remove` can take that pin over (FR-018 / B-1).
  - A `schedule_write_through` job holds none (`src/lib.rs:4017-4024` → `JobStart::Untracked`, `src/background.rs:297-300`). It still settles with `convert_to_storage` / `release_read` (`src/lib.rs:1403-1404`, `:1442-1455`).
- **Rationale**: the Assumption is the only spec statement about the writer's reference. It is now conditional. It has to state the gap honestly rather than imply a guarantee the code does not give.
- **Old** (`001:416`):
  > The background writer holds a read reference on each entry while write-through is in progress. The reference is released after write completes or fails.
- **New**:
  > A write-through job queued by `copy_gpu_to_memory_completed` holds the read reference that `downgrade_reference` left on the entry while write-through is in progress. The reference is released after the write completes (`convert_to_storage`) or fails (`release_read`), unless `remove` has taken it over first (FR-018). A job queued by `schedule_write_through` (interfaces FR-031a) is enqueued without a read reference of its own. *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt`. This is a known gap, not a guarantee: such a job still settles through `convert_to_storage`, which decrements whatever read reference is held at that moment. A code alignment is tracked in `.specify/sync/align-tasks.md`.)*

### B-5 · `contracts/errors.md` — `Timeout` and `KeyNotFound` rows for `remove` / `clear_memory_tier` / `pin` / `lookup`
- **Type**: BACKFILL · **Finding**: D-06, D-10′, U-04 (resolves N-04), inter-spec conflict 5 · **Confidence**: HIGH · `approved: false`
- **Requirement**: Error-variant contract (`specs/001-dispatcher-cache-interface/contracts/errors.md:33,37`).
- **Spec says** (`contracts/errors.md:37`): "| `Timeout` | `lookup`, `remove` | Dispatch map blocking operation exceeds timeout |"
- **Code does**: `DispatcherError::Timeout` is constructed in exactly two places, `pin` (`src/lib.rs:3725-3727`) and `clear_memory_tier` (`:3978`). `lookup_async` maps any `dm.lookup` error to `KeyNotFound` (`src/lib.rs:3155`). So does `remove` (`:3198`, `:3207-3211`).
- **Rationale**: a caller reading the contract would wait for a `Timeout` from `remove` that never comes, and would not expect one from `clear_memory_tier`.
- **Edit 1 — old** (`contracts/errors.md:37`):
  > | `Timeout` | `lookup`, `remove` | Dispatch map blocking operation exceeds timeout |
- **Edit 1 — new**:
  > | `Timeout` | `pin`, `clear_memory_tier` | `pin`: the dispatch-map wait for a writer exceeded its timeout. `clear_memory_tier`: the tier could not be emptied within 5 s because entries stayed pinned, and those entries remain resident *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt`: `lookup` and `remove` never return `Timeout`; they report a dispatch-map timeout as `KeyNotFound`)* |
- **Edit 2 — old** (`contracts/errors.md:33`, end of the `KeyNotFound` row):
  > which is a miss *(Sync 2026-09-28)* |
- **Edit 2 — new**:
  > which is a miss *(Sync 2026-09-28)*. **For `remove`, also** an entry held by a reader's or client's pin, or one still write-locked when the dispatch-map wait times out; in both cases nothing is freed. The pin of the key's own pending write-through does not refuse `remove` (001 FR-018) *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt`)* |

### B-6 · `contracts/idispatcher.md` — `remove` does not take a write reference
- **Type**: BACKFILL · **Finding**: D-10′ · **Confidence**: HIGH · `approved: false`
- **Requirement**: `IDispatcher::remove` contract (`specs/001-dispatcher-cache-interface/contracts/idispatcher.md:45`, `:171`).
- **Spec says** (`contracts/idispatcher.md:45`): "/// Acquires a write reference, then removes the dispatch map entry." And at `:171`: "- `remove()` guarantees the dispatch map entry is removed when it returns."
- **Code does**: no write reference is taken anywhere in `remove`. It calls `dm.lookup` (a read pin, released at once), then `dm.remove` (refused on any reference), then the ledger takeover (`src/lib.rs:3189-3212`). On `KeyNotFound` nothing is removed (`:3198`, `:3210-3211`).
- **Rationale**: the contract has been wrong since before this branch. This branch changes exactly the reference semantics it describes.
- **Edit 1 — old** (`contracts/idispatcher.md:45`):
  > /// Acquires a write reference, then removes the dispatch map entry.
- **Edit 1 — new**:
  > /// Removes the dispatch map entry first, then frees the memory-tier slot. The removal is refused while a reader or client holds a pin; the pin of the key's own pending write-through is taken over instead (001 FR-018).
- **Edit 2 — old** (`contracts/idispatcher.md:171`):
  > - `remove()` guarantees the dispatch map entry is removed when it returns.
- **Edit 2 — new**:
  > - `remove()` guarantees, when it returns `Ok`, that the dispatch map entry is removed, so no new lookup can obtain its memory-tier pointer. When it returns `KeyNotFound`, nothing was freed.

---

## ALIGN (Spec → Code)

### A-1 · 001/FR-013 + Assumption — a `schedule_write_through` job must own the pin it settles
- **Type**: ALIGN · **Finding**: D-16 · **Severity**: major · **Confidence**: MEDIUM (reasoning with file:line evidence; not reproduced) · `approved: false`
- **Requirement**: FR-013 ("appropriate read/write locking on the dispatch map to ensure thread safety"). Also the B-4 Assumption, and interfaces FR-031a (non-blocking, best-effort).
- **Spec says** (`001:292`): "The dispatcher MUST use appropriate read/write locking on the dispatch map to ensure thread safety during concurrent operations."
- **Code does**:
  - Remote-lookup publishes the entry with `read_ref == 0`, then calls `schedule_write_through` (`components/remote-lookup/src/actor.rs:600-629`).
  - The job is enqueued with no pin and no ledger record (`src/lib.rs:4017-4024`). It runs as `JobStart::Untracked` → `process_write_job` (`src/background.rs:297-300`; `src/lib.rs:1419-1421`).
  - It settles with `convert_to_storage`, which decrements any read reference (`components/dispatch-map/src/lib.rs:194-196`), or with `release_read` (`src/lib.rs:1442-1455`). That consumes the pin of a concurrent reader (warm `batch_lookup` hit `:2477`, cold `lookup_async` `:3121-3148`, client `pin` `:3715-3730`).
  - The evictors or `remove` can then free the slot under that reader's DMA. The job also reads the slot while unpinned (`mt.peek` `:1468`), so `remove`/`clear_memory_tier` can free it under the NVMe write and leak the published extent.
- **Proposed code change**:
  1. In `schedule_write_through`, take a read pin first: `dm.take_read(key)`, which bounds its wait by the dispatch-map timeout. The entry was published with `write_ref == 0`, so this does not block in practice. If you want a strict non-blocking guarantee, use a try-variant. On failure, return `Ok(())` without enqueueing (best-effort, FR-031a). Then record the pin and enqueue through `self.write_through_pins.pin(key, || writer.enqueue(…).is_ok())`, exactly as `copy_gpu_to_memory_completed` does (`src/lib.rs:3684-3695`). If the enqueue fails, release the pin.
  2. Make `PinState::pinned` a count, or refuse a second schedule while one is pending, so that two schedules of one key cannot share one flag.
  3. Once every enqueuer pins, `JobStart::Untracked` should not settle a pin it never took. Either make `Untracked` call `write_through` and then `release_read` only if a pin is recorded, or make it unreachable (`debug_assert`).
- **Task**: `components/dispatcher/src/lib.rs` (`schedule_write_through`, `run_write_job`), `src/background.rs` (`PinState`). Add a test module: a reader pins the key, a write-through is scheduled and run, and the test asserts the reader's pin is still counted, so `dm.remove` and `try_evict_to_block` are still refused, and that the reader's `release_read` does not underflow. It must fail on HEAD.
- **Rationale**: the spec forbids freeing a slot that a pinned in-flight load still uses (FR-024/FR-049). Changing the spec to allow this would not be a valid resolution.

### A-2 · 001/SC-003 (vs FR-009) — `remove` must free the SSD extent of a persisted resident entry
- **Type**: ALIGN · **Finding**: D-17 · **Severity**: moderate · **Confidence**: HIGH (direct reading; `LookupResult::MemoryTier` has no offset field) · `approved: false`
- **Requirement**: SC-003 ("Cache removal frees all associated resources (memory-tier slots and SSD extents)"). FR-009 should be widened in the same change.
- **Spec says** (`001:390`): "Cache removal frees all associated resources (memory-tier slots and SSD extents) so that they can be reused." FR-009 (`001:288`) frees the extent only "if in BlockDevice state".
- **Code does**: the extent offset comes only from `LookupResult::BlockDevice` (`src/lib.rs:3189-3199`). For a MemoryTier entry with `ssd_offset: Some`, `remove_extent` is never called (`:3220-3226`), and nothing frees the extent later (the SSD evictor walks dispatch-map keys, `src/background.rs:458-499`). With `format_on_init=false`, the next `initialize` recovers the removed key from the leaked extent (`src/lib.rs:1975-1976`).
- **Proposed code change**:
  - Have `IDispatchMap::remove` return the removed entry's location, or add `IDispatchMap::ssd_offset(key) -> Option<u64>` read under the same lock. `remove` then frees `ssd_offset` whether the entry was BlockDevice or a persisted MemoryTier entry.
  - Keep the ledger case as it is: a running job frees what it wrote.
  - In the same change, amend FR-009 to "free the extent on SSD (if the entry has one: BlockDevice, or MemoryTier with a completed write-through)".
- **Task**: `components/interfaces/src/idispatch_map.rs`, `components/dispatch-map/src/lib.rs`, `components/dispatcher/src/lib.rs` (`remove`), and the mocks. Test: `populate` → `flush_to_ssd` → `remove` → assert that the extent manager no longer lists the extent, and that a `format_on_init=false` re-initialize does not resurrect the key.
- **Rationale**: SC-003 is the measurable outcome, and FR-009's narrower wording predates `ssd_offset` on resident entries. A key the client removed must not come back.

### A-3 · 001/FR-056 vs FR-038 — `clear_memory_tier` must not free a reserved-but-not-completed slot (decision first)
- **Type**: ALIGN (contains a human decision) · **Finding**: P-05 · **Severity**: moderate · **Confidence**: MEDIUM · `approved: false`
- **Requirement**: FR-056 (the reserve → copy → complete lifecycle owns its slot until completion or `release_memory`) against FR-038 (empty the tier).
- **Spec says** (`001:338-341`): `reserve_memory` "reserves a memory-tier slot … and returns the host pointer for staging". `copy_gpu_to_memory_completed` "finalizes the entry after the async copies complete". `release_memory` "releases a reserved-but-not-completed slot".
- **Code does**: the reserved key is tracked by the memory tier at once (`components/memory-tier/src/lib.rs:345-367`), so `oldest_keys` returns it. It has no dispatch-map entry yet (that comes at `src/lib.rs:3618`). `clear_memory_tier` sees `KeyNotFound` from both `try_evict_to_block` and `dm.remove`, treats the key as cleared, and frees the slot while the caller's D2H may still be writing into it (`src/lib.rs:3957-3969`). The later `copy_gpu_to_memory_completed` fails `KeyNotFound` (`:3616`).
- **Options**:
  - **(a) ALIGN.** In `clear_memory_tier`, treat `KeyNotFound` from `try_evict_to_block` as "owned by an in-flight store" and skip it like a pinned entry. The 5 s `Timeout` then also covers a reservation that is never completed.
  - **(b) BACKFILL a precondition.** Add to FR-038: "MUST NOT be called while stores are in flight; reserved-but-not-completed slots are freed and their completion fails `KeyNotFound`."
- **Questions for the decision**: Is `clear_memory_tier` an administrative call made only on a quiescent node (the workload node agent's use, `apps/workload-generator/crates/workload-node-agent/src/agent.rs:256`), or must it be safe under live traffic, as the J22 repair implies?
- **Recommended**: (a). It matches the safety bar the `cccfb328e` repair set for pinned entries.
- **Task (for a)**: `components/dispatcher/src/lib.rs` (`clear_memory_tier`) and a test in `src/repair_test_clear_mt_pinned.rs`. The test reserves a key, runs a clear, then completes the key, and asserts the slot was never freed while reserved.

### A-4 · 001/FR-018 — close the two residual windows of the remove/write-through handshake
- **Type**: ALIGN · **Finding**: P-04 · **Severity**: minor · **Confidence**: LOW (interleaving reasoning only; outside what the Loom proof models) · `approved: false`
- **Requirement**: FR-018 as amended by B-1.
- **Spec says** (B-1 text): "A job already reading the DRAM slot finishes, then frees the slot …; in that case `remove` MUST NOT free the slot itself." This implies nobody else frees the slot while the job reads it.
- **Code does**:
  - **(a) Give-back window.** `remove_entry` calls `release_read` before `dm.remove`, and calls `take_read` only after that fails (`src/background.rs:342-346`). Between the two, the running job's pin is not held. A concurrent reader release plus a second `remove` (`src/lib.rs:3203`, outside the ledger lock) or a `clear_memory_tier` (`:3957-3969`) can then remove the entry and free the slot under the job. The job then publishes an extent it cannot settle (`:1424-1428`, `convert_to_storage` → `KeyNotFound`). Also, `take_read` can wait out the dispatch-map timeout while holding the ledger mutex.
  - **(b) Re-store during hand-off.** After `remove` returns `Ok` with `job_frees_slot`, a re-store of the key fails `AlreadyExists` until the job's `finish` (`src/lib.rs:1214-1223`, `:1432`).
- **Proposed code change**:
  - Step 1: extend the existing Loom/Spin model with two `remove`s plus one reader against one running job, to confirm (a).
  - If (a) is confirmed, replace release-then-remove with a single dispatch-map operation that removes the entry only if exactly one read reference is held, and consumes it (for example `IDispatchMap::remove_with_read(key)`), so the pin is never dropped without the removal succeeding.
  - For (b), let `reserve_memory` treat `AlreadyExists` on a key the ledger shows as `removed` and `running` as transient: backpressure-retry it instead of reporting `AlreadyExists`, and do not count it in `store_already_resident`.
- **Task**: `components/dispatcher/src/background.rs`, `src/lib.rs` (`reserve_memory`/`evict_and_insert`), possibly `components/interfaces/src/idispatch_map.rs` + `components/dispatch-map`. Add a test re-storing a key while its removed write-through is still running.
- **Rationale**: B-1 states the ownership rule. These are the two places where the code does not quite enforce it.

---

## Not proposed (verified aligned, carried over, or out of scope)

- **Carried over, still open in `align-tasks.md`, re-verified against HEAD:** A-01 (`src/lib.rs:2799-2804`), A-02 (`:2482-2483`; the `clear_memory_tier` fix does not cover an unpinned cold promotion, see the P-03 note), A-04 (`:815-821`), A-05 (`:3651-3678`), A-06 (`:2108`), A-07 (`:1222`; decision pending, and P-04(b) adds a source), A-08, N-01.
- **To be closed after apply:** A-03 (code fixed by `cccfb328e`; spec by B-3) and N-04 (by B-2/B-5/B-6).
- **Spec 002:** untouched by this branch; no new proposal.
