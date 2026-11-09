# Drift Resolution Proposals — dispatcher

Generated: 2026-10-21 · Branch: `fix/dispatcher-remove-clear-mt` (HEAD `efd502331`, base `origin/unstable` `a25e9be29`, plus the approved spec edits that are not yet committed) · Mode: **batch**, no questions asked
Based on: `drift-report.md` / `drift-report.json` of the same run (2026-10-21). The finding ids D-xx, P-xx and U-xx refer to that report.

**Nothing has been applied.** Every proposal is `approved: false`. No spec, source or other file was edited. Paths are relative to `components/dispatcher/`, and `001` means `specs/001-dispatcher-cache-interface/spec.md`. Every BACKFILL "old text" is copied verbatim from the current working-tree file, and each one was confirmed to occur exactly once.

**The approved edits stay.** The working-tree edits to 001 FR-056 (`flush_to_ssd`), interfaces FR-018a and the `IDispatcher::flush_to_ssd` doc comment are approved, and no proposal undoes them. B-7 keeps the approved FR-056 sentence word for word. It only adds a clarifying sentence and corrects the signature.

**Id scheme.** This run continues the single-digit ids of the 2026-10-20 run (B-1…B-6 and A-1…A-4, all applied or recorded). This run's ids are **B-7, B-8, B-9** and **A-5**. Two-digit ids (A-01…A-08, N-01) are tasks in `align-tasks.md`. They are referred to here, not re-proposed.

## Summary

| Resolution Type | Count | Ids |
|-----------------|-------|-----|
| Backfill (Code → Spec) | 3 | B-7, B-8, B-9 |
| Align (Spec → Code) | 1 | A-5 |
| Human Decision | 0 as a separate type (B-7 names an ALIGN alternative) |
| New Specs | 0 (the unspecced code U-08 is folded into B-8; U-01 stays open as N-01) |
| Supersede | 0 |
| Remove from Spec | 0 |
| **Total** | **4** | |

**Priority:**
- **Apply B-7, B-8 and B-9 in this branch's PR**, so the next stamp certifies `9bd231d85`. In the same apply, add a 001 header line: `**Last Synced**: 2026-10-21 — spec-sync on branch fix/dispatcher-remove-clear-mt …`.
- **Close A-07 in `align-tasks.md`.** Commit `efd502331` implemented its ALIGN option. FR-058 and interfaces FR-018a are aligned (tests `src/repair_test_store_already_resident.rs:474` and `src/lib.rs:6696`). No proposal is needed for it.
- A-5 is minor. Do it together with A-1, which is major, still open, and the main way to reach A-5 (b).

Still open from earlier runs, re-verified at HEAD and not re-proposed:
- A-1, A-2, A-3, A-4 (from the 2026-10-20 run);
- A-01, A-02, A-04, A-05, A-06, A-08, and N-01 (decision).

---

### Proposal B-7: 001/FR-056 — `flush_to_ssd`: what the count includes, and the return type

**Direction**: BACKFILL · **Finding**: D-19 · **Confidence**: MEDIUM · **approved: false**

**Requirement**: 001 FR-056, "Durability / introspection", the `flush_to_ssd` bullet. This is the approved text of 2026-10-21.

**Spec says** (`001:346`): "`flush_to_ssd() -> usize` forces a synchronous write-through flush of pending dirty entries to SSD and returns the number of memory-tier entries that have an SSD copy, or `IoError` if a write-through to SSD failed for an entry that is still in the memory tier (design invariant P9)."

**Code does**:
- The signature is `fn flush_to_ssd(&self) -> Result<usize, DispatcherError>` (`src/lib.rs:4006`; trait `components/interfaces/src/idispatcher.rs:838`).
- It drains the writer (`src/lib.rs:4010-4012`) and returns `IoError` if `failed_writes(|k| mt.contains(k)) > 0` (`:4026-4034`).
- Otherwise it returns `mt.oldest_keys(usize::MAX).filter(|&key| dm.is_evictable(key)).count()` (`:4037-4041`).
- `is_evictable` also requires `read_ref == 0 && write_ref == 0` (`components/dispatch-map/src/lib.rs:530-546`). A persisted entry that is pinned while `flush_to_ssd` runs (a reader, a client `pin`, a queued warm H2D) has an SSD copy but is not counted.

**Proposed resolution**: keep the approved sentence. Correct the return type, and say what the count includes.

- Old text (`001:346`, exact):
  > `` `flush_to_ssd() -> usize` forces a synchronous write-through flush of pending dirty entries to SSD and returns the number of memory-tier entries that have an SSD copy, or `IoError` if a write-through to SSD failed for an entry that is still in the memory tier (design invariant P9). ``
- New text:
  > `` `flush_to_ssd() -> Result<usize, DispatcherError>` forces a synchronous write-through flush of pending dirty entries to SSD and returns the number of memory-tier entries that have an SSD copy, or `IoError` if a write-through to SSD failed for an entry that is still in the memory tier (design invariant P9). The count is taken after the queue has drained. It counts the resident entries that `IDispatchMap::is_evictable` reports: an SSD copy and no reference held at that moment. A persisted entry that a reader or a client `pin` holds while the count is taken is therefore not included, so the count is a lower bound. Only `Ok` versus `IoError` is the persistence guarantee. ``

  The existing italic note `*(Sync 2026-10-21 — CODE CHANGE, repair DIS-FLUSH-REPORTS-FAILED-WRITE: …)*` that follows stays unchanged.

**Rationale**:
- The approved text describes the intended guarantee, and the code meets it for the error result. Only the meaning of the count is imprecise.
- Its one consumer (shm-queue `FLUSH_TO_SSD`, `lib/shmq-dispatcher/src/translate.rs:903-910`) only reports the number (`apps/remote-lookup-bench/src/main.rs:1051-1070`).
- Counting pinned persisted entries exactly would need an `IDispatchMap` accessor for `ssd_offset` that does not check pins. That is the same interface addition as A-2.

**Alternative (ALIGN)**: once A-2 adds `IDispatchMap::ssd_offset(key)`, count `ssd_offset(key).is_some()` instead, and drop the lower-bound sentence. Companion (outside `specs/**`, not a BACKFILL): the doc line "Returns the number of entries that now have a valid SSD offset" (`components/interfaces/src/idispatcher.rs:830`) would get the same qualifier.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal B-8: 001/FR-017 — a failed write-through is recorded and reported by `flush_to_ssd`

**Direction**: BACKFILL · **Finding**: D-20, U-08 · **Confidence**: HIGH · **approved: false**

**Requirement**: 001 FR-017.

**Spec says** (`001:298`): "When the asynchronous background write-through fails (extent allocation failure or block device I/O error), the background writer silently drops the job. The dispatch map entry remains in MemoryTier state. (Known limitation.)"

**Code does**:
- The job is still dropped (no retry), and the entry stays MemoryTier with its read pin released (`settle_write_through(None)` → `release_read`, `src/lib.rs:1454-1456`). The store's caller is still not told; `populate` returned `Ok` before the job ran (`:3311-3319`).
- Since `9bd231d85` the outcome is no longer silent. `run_write_job` records each settled write-through per key with `WriteThroughPins::record_write(key, landed)`: for untracked jobs at `src/lib.rs:1419-1423`, for owned jobs at `:1429`, with the set at `src/background.rs:262-263` and `:363-372`.
- `flush_to_ssd` returns `IoError` while any such key is still in the memory tier (`src/lib.rs:4026-4034`; `failed_writes` `src/background.rs:374-380`).
- A later successful write-through of the key clears the record. So does the key leaving the memory tier.
- Test: `flush_to_ssd_returns_err_when_a_write_through_failed` (`src/repair_test_flush_failed_write.rs:607`).

**Proposed resolution**:

- Old text (`001:298`, exact):
  > `When the asynchronous background write-through fails (extent allocation failure or block device I/O error), the background writer silently drops the job. The dispatch map entry remains in MemoryTier state. (Known limitation.)`
- New text:
  > `` When the asynchronous background write-through fails (extent allocation failure or block device I/O error), the background writer drops the job without retrying it, and the caller of the store is not told. The dispatch map entry remains in MemoryTier state with no SSD copy. (Known limitation.) The failure MUST be recorded per key, and `flush_to_ssd` MUST report it as `IoError` while the key is still in the memory tier (FR-056). The record is cleared when a later write-through of the key lands, or when the key is no longer in the memory tier. *(Sync 2026-10-21, branch `fix/dispatcher-remove-clear-mt` — code authoritative, commit `9bd231d85`, repair DIS-FLUSH-REPORTS-FAILED-WRITE. Before it the failure left no trace, so a `flush_to_ssd` followed by `clear_memory_tier` dropped the entry's only copy.)* ``

**Rationale**:
- The code is authoritative: it is a formal-verification repair (Spin J23), a Loom test failed on the old code, and Cornel approved it.
- FR-056 already states the reporting side in approved text. FR-017 is the requirement for the failure itself and now contradicts it ("silently").
- The record's known imprecision (P-06) is not written into the spec. It is the ALIGN task A-5.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal B-9: `contracts/errors.md` — `IoError` is raised by `flush_to_ssd`

**Direction**: BACKFILL · **Finding**: D-21 · **Confidence**: HIGH · **approved: false**

**Requirement**: `specs/001-dispatcher-cache-interface/contracts/errors.md`, Variant Usage table, `IoError` row.

**Spec says** (`contracts/errors.md:36`): "| `IoError` | `lookup`, `batch_lookup`, `initialize`, `commit_store` | Block device read/write error, device init failure. …"

**Code does**: `flush_to_ssd` returns `DispatcherError::IoError("flush_to_ssd: {failed} memory-tier entries have no SSD copy: …")` (`src/lib.rs:4029-4034`). The approved interface doc says the same (`components/interfaces/src/idispatcher.rs:832-837`).

**Proposed resolution** (the rest of the row, from "From the remote path" on, stays unchanged):

- Old text (`contracts/errors.md:36`, exact prefix):
  > `` | `IoError` | `lookup`, `batch_lookup`, `initialize`, `commit_store` | Block device read/write error, device init failure. ``
- New text:
  > `` | `IoError` | `lookup`, `batch_lookup`, `initialize`, `commit_store`, `flush_to_ssd` | Block device read/write error, device init failure. **For `flush_to_ssd`**: a write-through to SSD failed for an entry that is still in the memory tier, so that entry has no SSD copy (001 FR-017, FR-056) *(Sync 2026-10-21, branch `fix/dispatcher-remove-clear-mt`)*. ``

**Rationale**: the code is authoritative, and this is the companion to the approved interface doc. It follows the same pattern as the 2026-10-20 B-5 correction of the `Timeout` row.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal A-5: 001/FR-056 + FR-017 — tie the failed-write record to the entry it describes

**Direction**: ALIGN · **Finding**: P-06 · **Confidence**: MEDIUM for (a), LOW for (b) · **approved: false** · Severity: minor

**Requirement**: 001 FR-056 (approved text), and FR-017 as amended by B-8. `flush_to_ssd` returns `IoError` "if a write-through to SSD failed for an entry that is still in the memory tier". The record is cleared "when a later write-through of the key lands".

**Spec says** (`001:346`): "… or `IoError` if a write-through to SSD failed for an entry that is still in the memory tier."

**Code does**:
- (a) **The record outlives its entry.**
  - `failed` is keyed by `CacheKey` only. It is forgotten only when `mt.contains(key)` is false at the moment `flush_to_ssd` runs (`src/background.rs:376-379`). `remove` (`src/lib.rs:3183-3232`) and `clear_memory_tier` (`:3940-4004`) leave it alone.
  - Scenario: K's write fails, then `remove(K)`, then a new store of K is reserved (`reserve_memory`, `:3431`) but not completed yet. The shm-queue reserve / CopyToStore / complete steps are separate requests.
  - A `flush_to_ssd` in that interval returns `IoError`. The resident K is a different entry, and no write-through of it has run.
- (b) **"Landed" does not check that the offset was recorded.**
  - The outcome is recorded as `written.is_some()` (`src/lib.rs:1401-1403`, `:1429`), while `convert_to_storage`'s result is discarded (`:1452`). The `record_write` doc says landed means "the SSD offset was recorded" (`src/background.rs:363-364`).
  - An Untracked job (A-1) whose entry was removed, or freed and re-created, under it is still recorded as landed. If the entry was re-created, its `convert_to_storage` stamps the old job's offset onto the new entry.

**Proposed code change**:
1. In `remove`, after the dispatch-map entry is gone (`src/lib.rs:3206-3215`), and in `clear_memory_tier`, where an entry is removed without an SSD copy (`:3980-3983`), clear the key's failure record (`failed.remove(&key)`). The entry it described no longer exists. One way is a `WriteThroughPins::forget_failure(key)`.
2. Make `settle_write_through` return whether the offset was recorded (`dm.convert_to_storage(..).is_ok()`), and pass that, not `written.is_some()`, to `record_write`. Change `process_write_job` the same way.
3. Leave the success count to B-7 / A-2.

**Files to modify**: `src/lib.rs` (`remove`, `clear_memory_tier`, `settle_write_through`, `process_write_job`, `run_write_job`), `src/background.rs` (`WriteThroughPins`), `src/repair_test_flush_failed_write.rs` (new cases).

**Acceptance criteria**:
- [ ] Write fails, `remove`, re-reserve, `flush_to_ssd`: the result is `Ok`. The test fails on HEAD `efd502331`.
- [ ] A job whose `convert_to_storage` returns `KeyNotFound` is recorded as not landed.
- [ ] `flush_to_ssd_returns_err_when_a_write_through_failed` and `flush_to_ssd_counts_entries_with_an_ssd_offset` still pass.

**Rationale**:
- The approved spec is authoritative here. It ties the error to "an entry that is still in the memory tier", and the code ties it to a key.
- (b) is reachable only through the A-1 and A-4 windows, so do it in the same change as A-1.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

## Not proposed (recorded for the reviewer)

- **A-07 (FR-058 `store_already_resident`)**: resolved in code by `efd502331`. Close it in `align-tasks.md` at apply time. No spec edit is needed: FR-058 already says "reserves refused", and interfaces FR-018a is approved.
- **Cross-component, outside this component's `specs/**`:**
  - `dispatcher-p2p::flush_to_ssd` (`components/dispatcher-p2p/src/lib.rs:2889-2902`) still returns the queue length from before the flush and never returns `IoError`, against the edited interface contract (`components/interfaces/src/idispatcher.rs:824-838`). Handle it in the `dispatcher-p2p` sync.
  - The shm-queue reply field `jobs_flushed` (`lib/shmq-dispatcher/src/wire.rs:46-47`, `src/translate.rs:901`) no longer names what is returned. That crate has no `specs/`.
- **Carried over, code-side companion:** the interfaces `clear_memory_tier` doc comment (`components/interfaces/src/idispatcher.rs:806-810`) still predates FR-038 as amended.
