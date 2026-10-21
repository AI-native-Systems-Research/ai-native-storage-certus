# Align Tasks — dispatcher (code-side follow-ups)
Generated: 2026-08-31 (sync on branch `sync-tmp`)

This sync produced **no ALIGN items** (all six findings were code-authoritative
BACKFILL/BACKFILL-UNSPECCED, resolved by `specs/**` edits). The item below was a
documentation-only source cleanup that was outside this sync's editable scope
(`.specify/sync/**` + `specs/**`), recorded here for a follow-up source pass.

## T1 — ✅ RESOLVED (2026-09-21) — stale transport-example source comments removed

- **Status**: Resolved. No follow-up remains. The stale transport-specific
  comments in `copy_gpu_to_memory_async`'s null-stream branch were removed in
  commit `6d7ba234` ("revert dispatcher GPU experiments to specified stream
  model"). Verified 2026-09-21: `grep -rin grpc components/dispatcher/src`
  returns nothing, so the dispatcher source no longer references the removed
  control transport anywhere. The canonical spec's FR-040 / FR-042 already
  describe the shm-queue control transport (`2026-08-31` sweep).
- **Original item** (now closed): two comments described the null-stream caller
  with a transport-specific example that named the pre-`97e26738` control
  transport; the guidance was to reword to the shm-queue control handler or drop
  the example. Superseded by the `6d7ba234` deletion above.

## Task DISP-RESERVE-REASON (OPEN, Low) — "already cached" is reported as a failure

`op_reserve` writes a flat `0` for any `Err`, so a reserve refused because the key is
**already resident** is indistinguishable on the wire from one refused for want of space.
Measured: 7 224 of 121 430 reserves refused, **100% of them already-resident**, with
capacity-driven refusals at exactly **0** (`store_backpressure_events` and
`store_drops_on_full` both 0, `eviction_scans_exhausted` 0).

The same `mt.insert` → `AlreadyExists` is treated as a **hit** on the load path
(`serve_concurrently_promoted`, whose contract is that observing `MemoryTier` means the
data is resident) and as a **failure** on the store path. A declined block stays in GPU
HBM, so ~6% of stores currently waste device memory for data that is already safely
cached.

Options: carry a reason code on the wire, or report already-resident as reserved. Either
changes the client contract, hence deferred. Specified in **interfaces FR-018a**;
`store_already_resident` measures it.

---

# Apply 2026-10-14 — branch `fix/bundle-dispatcher-ems` (approved by Cornel, 2026-10-18)

Source: `.specify/sync/proposals.json` / `proposals.md` (analysis run 2026-10-18, HEAD
`4f8b5554b`). Each approved ALIGN proposal below carries the approval note *"approved as
an ALIGN task only: record in align-tasks.md, no code change in this PR"*. **No code was
changed.** Line numbers refer to HEAD `4f8b5554b`.

## Task: Align 002-served-by-tier-attribution/US1 AS3 + FR-009 (A-01, OPEN, major)

**Spec Requirement**: 002 US1 AS3 / FR-009. When the batched GPU sync fails, every key whose copy was in flight is attributed `ERROR`, and none is reported as a hit (002 spec.md:303-305, 430-434).
**Current Code**: a failed warm `stream_synchronize` is only logged. The warm results stay `Ok` with tier `Dram`, so they are counted as hits and sent as wire byte 1 (`src/lib.rs:2750-2755`).
**Required Change**: record the warm-hit indices as their copies are submitted (`src/lib.rs:2420-2431`). On a sync failure, set each of them to `Err(IoError("batch stream_synchronize failed: …"))`, mirroring the remote-delivery block (`src/lib.rs:2907-2917`). Leave `tier[i]` unchanged; the final mapping (`:2982`) then yields `Error`.
**Files to Modify**: `components/dispatcher/src/lib.rs` plus a test module
**Estimated Effort**: small

### Acceptance Criteria
- [ ] A test using `PinProbe::failing_sync` on warm keys asserts `ServedBy::Error` and `!result.is_ok()`.
- [ ] Warm keys whose sync failed are not counted in `promotions_to_gpu` / `lookup_hits_dram`.

## Task: Align 001-dispatcher-cache-interface/FR-051 + FR-036: `batch_lookup` cold pin (A-02, OPEN, major)

**Spec Requirement**: 001 FR-051 ("The caller's load pin keeps the entry from being evicted while waiting"), and FR-036 as amended by B-01 (the pin is kept through promotion).
**Current Code**: `let _ = dm.release_read(key);` runs right after BlockDevice classification (`src/lib.rs:2433-2434`). `evict_and_insert`, the NVMe read and `promote_block_to_memory_tier` then run unpinned (`src/lib.rs:2562-2717`), and the staging and no-drives paths do the same. `promote_to_memory_tier` repeats the pattern (`src/lib.rs:3726-3729`).
**Required Change**: adopt each cold key's classification pin into a `pins::PinnedKeys` guard, as warm hits do (`src/lib.rs:2356`, `:2428`). Drop the guard after the cold block and the staging post-pass, before the recovery pass. *Alternative:* document "the caller holds a pin for every key passed to `batch_lookup`" as a precondition in 001 FR-039 and in the interfaces trait docs.
**Files to Modify**: `components/dispatcher/src/lib.rs`, `src/repair_test_cold_pin.rs` (or a new test module); for the alternative, `specs/001-…/spec.md` and `components/interfaces/src/idispatcher.rs`
**Estimated Effort**: medium

### Acceptance Criteria
- [ ] `repair_test_cold_pin`'s concurrent-remove test is ported to `batch_lookup` and passes. It must fail before the fix.
- [ ] The shm-queue `LOOKUP` path (`lib/shmq-dispatcher/src/translate.rs:756`) is covered.

## Task: Align 001-dispatcher-cache-interface/FR-038 `clear_memory_tier` (A-03, ✅ RESOLVED 2026-10-20, major)

> **Resolved** by the apply on branch `fix/dispatcher-remove-clear-mt`. Code fixed in commit `cccfb328e`. The acceptance criteria are covered by `src/repair_test_clear_mt_pinned.rs:599,621,644`. The FR-038 wording was updated by B-3 of that run. Reserved-but-not-completed slots are a separate case, tracked as A-3 below.

**Spec Requirement**: 001 FR-038. Clearing must not leave a dispatch-map entry that points at a freed slot.
**Current Code**: on *any* `try_evict_to_block` failure, `ActiveReferences` included, the code calls `mt.remove(key)` unconditionally and then `dm.remove(key)`. The map refuses that removal while the entry is pinned, which leaves a dangling `MemoryTier` pointer (`src/lib.rs:3875-3890`). The path is reachable over shm-queue `CLEAR_MEMORY_TIER`.
**Required Change**: match on the error. For `ActiveReferences`, skip the key and leave it resident. For anything else, call `dm.remove` first and `mt.remove` only if it succeeds. Bound the outer loop. In the same change, update the FR-038 wording (`evict_next` → `oldest_keys`; `convert_memory_tier_to_block` → `try_evict_to_block`; "pinned entries are skipped and remain resident").
**Files to Modify**: `components/dispatcher/src/lib.rs`, `specs/001-dispatcher-cache-interface/spec.md` (FR-038)
**Estimated Effort**: small

### Acceptance Criteria
- [ ] A test that pins a key and then calls `clear_memory_tier` sees the key still resident and readable.
- [ ] `clear_memory_tier` terminates when every resident key is pinned.

## Task: Align 001-dispatcher-cache-interface/FR-028 + US-9 AS3: no-drives promote (A-04, OPEN, moderate)

**Spec Requirement**: 001 FR-028 / US-9 AS3. The reader MUST re-insert the entry into the memory tier and re-register it as a MemoryTier entry (001 spec.md:305, :166).
**Current Code**: `dm.create_memory_tier_entry` on an existing BlockDevice entry returns `AlreadyExists`, which is ignored. `dm.release_write` is then called without a write ref. The slot is orphaned and the entry stays BlockDevice (`src/lib.rs:814-820`).
**Required Change**: use `dm.promote_block_to_memory_tier(key, mem_ptr, ipc_handle.size)`, as `batch_lookup`'s no-drives branch does (`src/lib.rs:2495-2500`). Drop the stray `release_write`. Make `MockDispatchMap::create_memory_tier_entry` (`src/lib.rs:4428-4451`) refuse to overwrite.
**Files to Modify**: `components/dispatcher/src/lib.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] After a no-drives single-key cold lookup, the entry is `MemoryTier` and the slot is referenced.
- [ ] The mock refuses an overwrite, and the existing tests still pass.

## Task: Align 001-dispatcher-cache-interface/FR-003 + FR-056: phase-3 release in `copy_gpu_to_memory_completed` (A-05, OPEN, moderate)

**Spec Requirement**: 001 FR-003 as amended by B-03. Every failure before the entry is registered MUST release the reserved slot, and no failure may leave a dangling entry.
**Current Code**: under `integrity-check`, a `stream_synchronize` or `set_checksum` failure after `create_memory_tier_entry` calls `mt.remove(key)` but leaves the new dispatch-map entry write-locked and pointing at the freed slot. A `downgrade_reference` failure leaves it write-locked (`src/lib.rs:3599-3620`).
**Required Change**: on these paths, undo the registration (`release_write` and then `dm.remove`) **before** `mt.remove`.
**Files to Modify**: `components/dispatcher/src/lib.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] A test under `--features integrity-check` with a failing sync leaves neither a dispatch-map entry nor a slot.

## Task: Align 001-dispatcher-cache-interface/FR-042: SSD evictor sender wiring (A-06, OPEN, moderate)

**Spec Requirement**: 001 FR-042 as amended by B-05. The SSD evictor emits `Removed`, and its dropped events are counted.
**Current Code**: `BackgroundEvictor::start` receives a one-time clone of `eviction_tx` at `initialize()` (`src/lib.rs:2059-2071`). Both servers create the channel after `initialize` (`apps/certus-server-yaml/build.rs:447-471`; `apps/certus-server/src/main.rs:469`, `:529`), so the evictor holds `None` forever.
**Required Change**: pass `Arc::clone(&self.eviction_tx)`, as `MemoryTierEvictor` does (`src/lib.rs:2101`, `src/background.rs:462`), and take the lock on each emit.
**Files to Modify**: `components/dispatcher/src/lib.rs`, `components/dispatcher/src/background.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] A test that creates the channel after `initialize` sees a `Removed` event arrive, or sees it counted in `eviction_dropped_count()`.

## Task: Align 002-served-by-tier-attribution/FR-027 + SC-009: per-value attribution tests (A-08, OPEN, moderate)

**Spec Requirement**: 002 FR-027 / SC-009. Each attribution value MUST have a test that fails if that value is mis-assigned (002 spec.md:554-555).
**Current Code**: value-specific assertions exist only for `Dram` (`src/repair_test_served_by.rs:484`) and `Miss` (`src/lib.rs:5927`).
**Required Change**: add one test for each remaining value: `Ssd` (a no-drives cold key), `Remote` (`setup_initialized_with_remote`), `SizeMismatch` (`MockDispatchMap::set_mismatch_key`, `src/lib.rs:4200`) and `Error` (the A-01 warm-sync failure, plus the not-initialized path, `src/lib.rs:2238-2251`).
**Files to Modify**: dispatcher test modules
**Estimated Effort**: small

### Acceptance Criteria
- [ ] Each new test fails against a deliberately wrong mapping (FR-028).

## Open items (not approved — awaiting a decision)

### ✅ RESOLVED A-07 — 001/FR-058 `store_already_resident` counts load-path race losers (minor)

> **Resolved 2026-10-21** in code by `efd502331` (repair of `7db1a25a1`, branch `fix/dispatcher-remove-clear-mt`): the ALIGN option was taken. `store_already_resident` is now counted only in `reserve_memory`, the store path; a load that loses the insert race is a hit and is not counted. FR-058 ("reserves refused") already said this, and interfaces FR-018a was edited to match (approved). Tests: `src/repair_test_store_already_resident.rs:474`, `src/lib.rs:6696`. No spec edit was needed.
> Approval note: *"waits for Cornel's decision (ALIGN as a task, or BACKFILL); record in align-tasks.md as an open decision"*

- **Option ALIGN**: move `record_store_already_resident()` out of `evict_and_insert` (`src/lib.rs:1213-1222`) into `reserve_memory`'s `Err(AlreadyExists)` handling.
- **Option BACKFILL**: in FR-058, replace "reserves refused because the key was already cached" with "`mt.insert` calls refused because the key was already resident, on the store path (a refused reserve) and on the load path (a lost concurrent promotion, FR-051)". Interfaces FR-018a then needs the matching change.
- Related: DISP-RESERVE-REASON above.

### OPEN N-01 — new 001/FR-062 `integrity-check` verify-before-serve (MEDIUM confidence)
> Approval note: *"MEDIUM confidence new requirement: record as an open item in align-tasks.md (Cornel's rule)"*

- Draft FR-062 text: see `proposals.md` § N-01. CRC-32 is computed after the store D2H (`src/lib.rs:3593-3617`) and checked before every warm, remote and single-key H2D. A mismatch returns `IoError` and is attributed `ERROR`. Cold SSD promotions are not verified.

### ✅ RESOLVED N-04 — 001/FR-009 + `contracts/errors.md`: `remove` on a pinned entry (MEDIUM confidence)

> **Resolved 2026-10-20** by the apply on branch `fix/dispatcher-remove-clear-mt` (the remove-no-wait PR). B-2 (remove edge case), B-5 (`contracts/errors.md` `Timeout` + `KeyNotFound` rows) and B-6 (`contracts/idispatcher.md` `remove`) specify the result the draft below asked about. The BACKFILL option was taken: `remove` stays `KeyNotFound`, with no distinct busy error.
> Approval note: *"MEDIUM confidence: record as an open item; it belongs to the remove-no-wait PR"*

- Draft: a pinned or write-locked entry → `remove` frees nothing and returns `KeyNotFound`, and a dispatch-map timeout is reported as `KeyNotFound` as well. Drop `remove` from the `Timeout` row of `contracts/errors.md:37`. *Alternative (ALIGN)*: return `Timeout` or a distinct error, so callers can tell busy from absent. Deferred to the remove-no-wait PR.

---

# Apply 2026-10-14 — branch `fix/dispatcher-remove-clear-mt` (approved by Cornel, 2026-10-20)

Source: `.specify/sync/proposals.json` / `proposals.md` (analysis run 2026-10-20, HEAD
`cccfb328e`). Approved by Cornel Constantinescu, 2026-10-20 ("let's finish the PR"). This
run's ids are single-digit (A-1…A-4). Each one was approved as an ALIGN task only, so
**no code was changed**. A-1, A-2 and A-4 carry the note *"approved as an ALIGN task only:
record in align-tasks.md, no code change in this PR"*. A-3 carries its own note, quoted in
the task. Line numbers refer to HEAD `cccfb328e`, relative to `components/dispatcher/`.

## Task: Align 001-dispatcher-cache-interface/FR-013 (+ Assumption, interfaces FR-031a): a `schedule_write_through` job must own the pin it settles (A-1, OPEN, major)

**Spec Requirement**: 001 FR-013 ("The dispatcher MUST use appropriate read/write locking on the dispatch map to ensure thread safety during concurrent operations"), the Assumption on the background writer's read reference as amended by B-4, and interfaces FR-031a (non-blocking, best-effort).
**Current Code**: remote-lookup publishes the entry with `read_ref == 0` and then calls `schedule_write_through` (`components/remote-lookup/src/actor.rs:600-629`). The job is enqueued with no pin and no ledger record (`src/lib.rs:4017-4024`). It runs as `JobStart::Untracked` (`src/background.rs:297-300`; `src/lib.rs:1419-1421`), but it still settles with `convert_to_storage`, which decrements any read reference (`components/dispatch-map/src/lib.rs:194-196`), or with `release_read` (`src/lib.rs:1442-1455`). That consumes a concurrent reader's pin: a warm `batch_lookup` hit (`:2477`), a cold `lookup_async` (`:3121-3148`) or a client `pin` (`:3715-3730`). The evictors or `remove` can then free the slot under that reader's DMA. The job also reads the slot unpinned (`mt.peek`, `:1468`).
**Required Change**:
1. In `schedule_write_through`, call `dm.take_read(key)` first. On failure, return `Ok(())` without enqueueing (best-effort, FR-031a). Then record the pin and enqueue through `self.write_through_pins.pin(key, || writer.enqueue(…).is_ok())`, as `copy_gpu_to_memory_completed` does (`src/lib.rs:3684-3695`). If the enqueue fails, release the pin.
2. Make `PinState::pinned` a count, or refuse a second schedule while one is pending.
3. `JobStart::Untracked` must not settle a pin it never took. Either settle only when a pin is recorded, or make it unreachable (`debug_assert`).
**Files to Modify**: `components/dispatcher/src/lib.rs`, `components/dispatcher/src/background.rs`, a new test module
**Estimated Effort**: medium

### Acceptance Criteria
- [ ] A reader's pin survives a scheduled and completed write-through: `dm.remove` and `try_evict_to_block` are still refused, and the reader's `release_read` does not underflow.
- [ ] The test fails on HEAD `cccfb328e`.

## Task: Align 001-dispatcher-cache-interface/SC-003 (vs FR-009): `remove` must free the SSD extent of a persisted resident entry (A-2, OPEN, moderate)

**Spec Requirement**: 001 SC-003 ("Cache removal frees all associated resources (memory-tier slots and SSD extents) so that they can be reused", `spec.md:390`). FR-009 (`:288`) frees the extent only "if in BlockDevice state" and should be widened in the same change.
**Current Code**: the extent offset comes only from `LookupResult::BlockDevice` (`src/lib.rs:3189-3199`). For a MemoryTier entry with `ssd_offset: Some`, `remove_extent` is never called (`:3220-3226`), and nothing frees the extent later. With `format_on_init=false`, the next `initialize` recovers the removed key from the leaked extent (`src/lib.rs:1975-1976`).
**Required Change**: have `IDispatchMap::remove` return the removed entry's location, or add `IDispatchMap::ssd_offset(key) -> Option<u64>` read under the same lock. `remove` then frees `ssd_offset` for BlockDevice entries and for persisted MemoryTier entries. Keep the ledger case unchanged: a running job frees what it wrote. Amend FR-009 to "free the extent on SSD (if the entry has one: BlockDevice, or MemoryTier with a completed write-through)".
**Files to Modify**: `components/interfaces/src/idispatch_map.rs`, `components/dispatch-map/src/lib.rs`, `components/dispatcher/src/lib.rs` (`remove`), the mocks
**Estimated Effort**: medium

### Acceptance Criteria
- [ ] `populate` → `flush_to_ssd` → `remove` frees the extent (the extent manager no longer lists it).
- [ ] A `format_on_init=false` re-initialize does not resurrect the removed key.

## Task: Align 001-dispatcher-cache-interface/FR-056 vs FR-038: `clear_memory_tier` must not free a reserved-but-not-completed slot (A-3, OPEN — decision recorded, moderate)

> Approval note: *"record as an ALIGN task with the open decision; recommendation (a): clear_memory_tier skips a reservation that is not completed, like a pinned entry; no code change in this PR"*

**Spec Requirement**: 001 FR-056 (the reserve → copy → complete lifecycle owns its slot until completion or `release_memory`, `spec.md:338-341`) against FR-038 (empty the tier, as amended by B-3).
**Current Code**: a reserved key is tracked by the memory tier at once (`components/memory-tier/src/lib.rs:345-367`), so `oldest_keys` returns it. It has no dispatch-map entry yet (created at `src/lib.rs:3618`). `clear_memory_tier` gets `KeyNotFound` from both `try_evict_to_block` and `dm.remove`, counts the key as cleared, and frees the slot while the caller's D2H may still be writing into it (`src/lib.rs:3957-3969`). The later `copy_gpu_to_memory_completed` fails `KeyNotFound` (`:3616`).
**Open decision**: is `clear_memory_tier` an administrative call made only on a quiescent node (the workload node agent's use, `apps/workload-generator/crates/workload-node-agent/src/agent.rs:256`), or must it be safe under live stores, as the J22 repair implies?
- **(a) ALIGN — recommended**: in `clear_memory_tier`, treat `KeyNotFound` from `try_evict_to_block` as owned by an in-flight store and skip it like a pinned entry. The 5 s `Timeout` then also covers a reservation that is never completed.
- **(b) BACKFILL a precondition**: add to FR-038 "MUST NOT be called while stores are in flight; reserved-but-not-completed slots are freed and their completion fails `KeyNotFound`."
**Required Change (for a)**: as above, in `clear_memory_tier`.
**Files to Modify**: `components/dispatcher/src/lib.rs`, `components/dispatcher/src/repair_test_clear_mt_pinned.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] Decision (a) or (b) confirmed by Cornel.
- [ ] (a): a test runs reserve → `clear_memory_tier` → `copy_gpu_to_memory_completed` and asserts the slot was never freed while reserved.

## Task: Align 001-dispatcher-cache-interface/FR-018 (as amended by B-1): close the two residual windows of the remove/write-through handshake (A-4, OPEN, minor, LOW confidence)

**Spec Requirement**: 001 FR-018 as amended by B-1: "A job already reading the DRAM slot finishes, then frees the slot …; in that case `remove` MUST NOT free the slot itself." This implies that nobody else frees the slot while the job reads it.
**Current Code**:
- **(a) Give-back window.** `remove_entry` calls `release_read` before `dm.remove`, and calls `take_read` only after that fails (`src/background.rs:342-346`). Between the two, the running job's pin is not held. A concurrent reader's release plus a second `remove` (`src/lib.rs:3203`, outside the ledger lock) or a `clear_memory_tier` (`:3957-3969`) can then remove the entry and free the slot under the job. `take_read` can also wait out the dispatch-map timeout while holding the ledger mutex.
- **(b) Re-store during hand-off.** After `remove` returns `Ok` with `job_frees_slot`, a re-store of the key fails `AlreadyExists` until the job's `finish` (`src/lib.rs:1214-1223`, `:1432`).
**Required Change**: first extend the Loom/Spin model with two `remove`s plus one reader against one running job, to confirm (a). If (a) is confirmed, replace release-then-remove with one dispatch-map operation that removes the entry only if exactly one read reference is held, and consumes it (for example `IDispatchMap::remove_with_read(key)`). For (b), have `reserve_memory` treat `AlreadyExists` on a key the ledger shows as removed and running as transient: retry it under backpressure, and do not count it in `store_already_resident`.
**Files to Modify**: `components/dispatcher/src/background.rs`, `components/dispatcher/src/lib.rs` (`reserve_memory` / `evict_and_insert`), possibly `components/interfaces/src/idispatch_map.rs` and `components/dispatch-map`
**Estimated Effort**: medium

### Acceptance Criteria
- [ ] The model passes with two `remove`s plus a reader against a running job.
- [ ] Re-storing a key while its removed write-through is still running succeeds.

## Open items from this run

None. Every proposal of this run was approved, and none is unapproved with an `approval_note`. Still open from the `fix/bundle-dispatcher-ems` run: A-01, A-02, A-04, A-05, A-06, A-08 (tasks), and A-07 and N-01 (decisions).

---

# Apply 2026-10-21 — branch `fix/dispatcher-remove-clear-mt` (approved by Cornel, 2026-10-21)

Source: `.specify/sync/proposals.json` / `proposals.md` (analysis run 2026-10-21, HEAD
`efd502331`). B-7, B-8 and B-9 were approved and applied to `specs/**` (see
`apply-report.md`). This run's only ALIGN proposal, A-5, is **not approved**: it carries a
TRACK note and is recorded below as an open item. **No code was changed.** A-07 above is
closed (resolved in code by `efd502331`). Line numbers refer to HEAD `efd502331`, relative
to `components/dispatcher/`.

## Open items from this run (not approved — tracked)

### OPEN A-5 — Align 001-dispatcher-cache-interface/FR-056 + FR-017 (as amended by B-8): tie the failed-write record to the entry it describes (minor; MEDIUM confidence for (a), LOW for (b))

> Approval note: *"TRACK (Cornel 2026-10-21): code change for a later repair with a test first; not in this PR."*

**Spec Requirement**: 001 FR-056 (`flush_to_ssd` returns `IoError` "if a write-through to SSD failed for an entry that is still in the memory tier", `spec.md:348`) and FR-017 as amended by B-8 (the record "is cleared when a later write-through of the key lands, or when the key is no longer in the memory tier", `spec.md:300`).
**Current Code**:
- (a) **The record outlives its entry.** The `failed` set of `WriteThroughPins` is keyed by `CacheKey` only and is forgotten only when `mt.contains(key)` is false at the moment `flush_to_ssd` runs (`src/background.rs:376-379`). `remove` (`src/lib.rs:3183-3232`) and `clear_memory_tier` (`:3940-4004`) leave it alone. So: K's write fails → `remove(K)` → a new store of K is reserved (`reserve_memory`, `:3431`) but not yet completed → `flush_to_ssd` returns `IoError` for an entry that no write-through has touched.
- (b) **"Landed" does not check that the offset was recorded.** The outcome is recorded as `written.is_some()` (`src/lib.rs:1401-1403`, `:1429`), while the result of `convert_to_storage` is discarded (`:1452`), although the `record_write` doc says landed means "the SSD offset was recorded" (`src/background.rs:363-364`). Reachable only through the A-1 / A-4 windows.
**Required Change**:
1. Clear the key's failure record in `remove`, after the dispatch-map entry is gone (`src/lib.rs:3206-3215`), and in `clear_memory_tier` where an entry is removed without an SSD copy (`:3980-3983`), e.g. with a new `WriteThroughPins::forget_failure(key)`.
2. Make `settle_write_through` return `dm.convert_to_storage(..).is_ok()` and pass that to `record_write` instead of `written.is_some()`; the same in `process_write_job`.
3. Leave the success count to B-7 / A-2.
**Files to Modify**: `src/lib.rs` (`remove`, `clear_memory_tier`, `settle_write_through`, `process_write_job`, `run_write_job`), `src/background.rs` (`WriteThroughPins`), `src/repair_test_flush_failed_write.rs` (new cases)
**Estimated Effort**: small (do it together with A-1)

### Acceptance Criteria
- [ ] Write fails, `remove`, re-reserve, `flush_to_ssd`: the result is `Ok`. The test is written first and fails on HEAD `efd502331`.
- [ ] A job whose `convert_to_storage` returns `KeyNotFound` is recorded as not landed.
- [ ] `flush_to_ssd_returns_err_when_a_write_through_failed` and `flush_to_ssd_counts_entries_with_an_ssd_offset` still pass.

## Still open (carried over, re-verified at HEAD `efd502331` by the 2026-10-21 analysis)

A-1, A-2, A-3 (decision recorded), A-4 (2026-10-14 apply); A-01, A-02, A-04, A-05, A-06, A-08 (tasks) and N-01 (decision) from earlier runs.

## Cross-component (recorded elsewhere)

- `dispatcher-p2p` still has the pre-repair `remove` / `clear_memory_tier` / `flush_to_ssd` behaviour. That task is interfaces proposal 7 (TRACK) and is recorded in `components/interfaces/.specify/sync/align-tasks.md`, for the component `dispatcher-p2p`.

