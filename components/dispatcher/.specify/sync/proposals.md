# Drift Resolution Proposals — dispatcher

Generated: 2026-10-23 · Branch: `fix/dispatcher-remove-ssd-extent` (HEAD `765f75ba3`, base `origin/unstable` `a905f89be`, clean working tree) · Mode: **batch**, no questions asked
Based on: `drift-report.md` / `drift-report.json` of the same run (2026-10-23). The finding ids D-xx, P-xx and U-xx refer to that report.

**Nothing has been applied.** Every proposal is `approved: false`, including B-12, which Cornel approved in the earlier run. It is re-proposed here because its provenance note had to change for this branch; the approval is recorded in `prior_approval` in the JSON. No spec, source or other file was edited. Paths are relative to `components/dispatcher/`. `001` means `specs/001-dispatcher-cache-interface/spec.md`. Every BACKFILL "old text" is copied verbatim from the current file, and each occurs exactly once in its file.

**Id scheme.** This run continues the ids of the 2026-10-23 run (A-9, A-10, B-12, B-13). B-12 keeps its id. New ids: **B-14, B-15, A-11, A-12**. Existing tasks (A-1…A-5, A-01…A-08, A-10, N-01) are referred to here, not re-proposed. **A-9 is implemented** by `765f75ba3` (option (c)); its remaining part, option (a), is re-proposed as A-12, because the option (c) form leaves the P-07 race open.

## Summary

| Resolution Type | Count | Ids |
|-----------------|-------|-----|
| Backfill (Code → Spec) | 3 | B-12 (approved earlier, re-checked), B-14, B-15 |
| Align (Spec → Code) | 2 | A-11, A-12 |
| Human Decision | 0 |
| New Specs | 0 (U-10 is folded into B-14; U-01 stays open as N-01) |
| Supersede | 0 |
| Remove from Spec | 0 |
| **Total** | **5** | |

**Order:**
1. Apply **B-12** in this branch's PR, so the next stamp certifies `a7444f4ab`. Add **B-14** and **B-15** in the same apply if approved. Add a 001 header line: `**Last Synced**: 2026-10-23 — spec-sync on branch fix/dispatcher-remove-ssd-extent …`.
2. After the apply, update `align-tasks.md`:
   - close **A-2**, resolved by `a7444f4ab` (its restart criterion has no test);
   - close **A-06**, resolved by #568;
   - record **A-9** as implemented by `765f75ba3`.
3. **A-11** (moderate) and **A-12** (minor) are code tasks, each test first. They are independent of each other. A-12 is best done together with A-4(a), because both need the same dispatch-map operation.

---

### Proposal B-12: 001 FR-009 + US-4 + `contracts/idispatcher.md` — `remove` frees the extent of a persisted resident entry, and releases only its own read reference

**Direction**: BACKFILL · **Finding**: D-23 (resolves the spec side of A-2) · **Severity**: minor · **Confidence**: HIGH · `approved: false` (approved by Cornel on 2026-10-23 for the earlier run; re-checked here)

**Requirement**: 001 FR-009 (and SC-003, which it implements); the US-4 narrative; the `remove` doc comment in `contracts/idispatcher.md`.

**Spec says**:
- `001:294` FR-009: "The `remove(key)` method MUST free the memory-tier slot (if present via `mt.remove()`), free the extent on SSD (if in BlockDevice state), and remove the dispatch map entry."
- `001:92` US-4: "… and frees the SSD extent (if the entry was in BlockDevice state)."
- `contracts/idispatcher.md:46-47`: "If the entry is on a block device, frees the SSD extent via the extent manager."

**Code does**:
- For a `MemoryTier` entry whose write-through has landed, `remove` holds its lookup reference and flips the entry to `BlockDevice` (`src/lib.rs:3305`). That transition records the `ssd_offset`. It then looks the key up again to learn the offset (`:3308-3314`), and frees that extent through `drive_index` (`:3372-3378`).
- BlockDevice entry: `:3289-3292`. An unpersisted resident entry gets `InvalidState`, so there is no extent to free (`:3328-3330`). A pending job's extent is freed by the job (FR-018).
- Its lookups release a read reference only when they took one. Never after `NotExist` (`:3332-3334`, `:3324-3325`).
- Tests: `src/repair_test_remove_frees_ssd_extent.rs:941` (slot and extent freed and reused), `:991` (no foreign release).

**Re-check against this branch** (clause by clause; details in the drift report, D-23): every normative clause is true at HEAD `765f75ba3`. The P-07 race does not falsify it: `remove` then returns `KeyNotFound`, not `Ok`, so the success case still holds, and P-07 is an ALIGN task (A-12). The text describes the outcome, not the mechanism, so it stays true when A-12 replaces the flip. **One change from the approved text:** the italic provenance named `fix/dispatcher-promote-remove` and commit `b430e4d47`. That commit is not an ancestor of this branch, where the repair is `a7444f4ab`. The note now names this branch and its commits. The normative wording is unchanged, word for word.

**Rationale (code authoritative)**: SC-003 ("Cache removal frees all associated resources … so that they can be reused") always required this. The old FR-009 wording described the leak, not the intent. The code is a formal-verification repair (Spin J26). Loom tests failed on the old code, and the acceptance check passed. Task A-2 asked for exactly this amendment.

**Exact edits** (3):

1. `specs/001-dispatcher-cache-interface/spec.md:294`

   Old:
   ```
   - **FR-009**: The `remove(key)` method MUST free the memory-tier slot (if present via `mt.remove()`), free the extent on SSD (if in BlockDevice state), and remove the dispatch map entry.
   ```
   New:
   ```
   - **FR-009**: The `remove(key)` method MUST free the memory-tier slot (if present via `mt.remove()`), free the extent on SSD (if the entry has one: a BlockDevice entry, or a MemoryTier entry whose write-through has landed and recorded its `ssd_offset`), and remove the dispatch map entry. Its own dispatch-map lookup MUST release only a read reference that lookup acquired: a `NotExist` answer takes none, so none is released. *(Sync 2026-10-23, branch `fix/dispatcher-remove-ssd-extent` — code authoritative, commits `a7444f4ab` and `765f75ba3`, repair of root causes #9 and #12. Before it, `LookupResult::MemoryTier` hid the `ssd_offset`, so the extent of a persisted resident entry leaked and the key came back from SSD after a `format_on_init=false` restart (SC-003); and a lookup that answered `NotExist` still released a read reference, which could decrement the write-through pin of a concurrently registered entry.)*
   ```

2. `specs/001-dispatcher-cache-interface/spec.md:92`

   Old:
   ```
   A client application wants to evict a cache entry. The client calls the dispatcher's remove method with a cache key. The dispatcher removes the entry from the memory-tier (if present), removes the dispatch map entry, and frees the SSD extent (if the entry was in BlockDevice state).
   ```
   New:
   ```
   A client application wants to evict a cache entry. The client calls the dispatcher's remove method with a cache key. The dispatcher removes the entry from the memory-tier (if present), removes the dispatch map entry, and frees the SSD extent (if the entry has one: it is in BlockDevice state, or it is memory-tier resident and its write-through has landed).
   ```

3. `specs/001-dispatcher-cache-interface/contracts/idispatcher.md:46-47`

   Old:
   ```
           /// If the entry is on a block device, frees the SSD extent via the
           /// extent manager.
   ```
   New:
   ```
           /// If the entry has an SSD copy (it is on a block device, or it is
           /// resident and its write-through has landed), frees the SSD extent
           /// via the extent manager.
   ```

**Companion (outside `specs/**`)**: `components/interfaces/src/idispatcher.rs:629` already says "Frees memory-tier slot and/or SSD extent". No change is needed.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal B-14: 001 "remove" edge case + `contracts/idispatcher.md` postcondition — a refused `remove` leaves the entry exactly as it was

**Direction**: BACKFILL · **Finding**: U-10 · **Severity**: minor (spec gap) · **Confidence**: HIGH for the restore, MEDIUM for the window sentence · `approved: false`

**Requirement**: the 001 edge case "remove", and the `remove()` postcondition in `contracts/idispatcher.md`.

**Spec says**:
- `001:239` (end of the bullet): "A `remove` that is refused because a reader or a client `pin` holds the entry, or because the entry stays write-locked past the dispatch-map wait, returns `KeyNotFound` and frees nothing."
- `contracts/idispatcher.md:171`: "`remove()` guarantees, when it returns `Ok`, that the dispatch map entry is removed, so no new lookup can obtain its memory-tier pointer. When it returns `KeyNotFound`, nothing was freed."

**Code does**:
- `765f75ba3` (proposal A-9, option (c)): when `remove` has flipped a persisted resident entry to BlockDevice and the removal is then refused, it puts the entry back with `dm.promote_block_to_memory_tier(key, pointer, size)`, using the pointer and size of its first lookup (`src/lib.rs:3287`, `:3313`, `:3355-3360`).
- The in-place promote keeps every reference and the SSD offset (`components/dispatch-map/src/lib.rs:498-509`). So after a refused `remove`:
  - the entry is still MemoryTier, with the same slot and its `ssd_offset`;
  - no reference was taken or dropped;
  - a later lookup is served from DRAM.
- Test: `refused_remove_leaves_the_entry_unchanged` (`src/repair_test_remove_refused_leaves_entry_unchanged.rs:875`). It fails on `a7444f4ab`.
- The spec only says "frees nothing". It does not say the location is unchanged, which is what callers rely on: the pinning client's own load must still hit DRAM.
- While `remove` runs, other callers can see the entry as BlockDevice for a moment (`:3305` to `:3340`/`:3357`). The spec does not say this either. Its known consequences are P-07, which is not backfilled (ALIGN A-12), and a concurrent cold lookup that takes the `AlreadyExists` recovery (FR-051), which is benign.

**Proposed resolution** (2 edits):

1. `specs/001-dispatcher-cache-interface/spec.md:239`. Only the sentence quoted below changes; the rest of the bullet stays as it is.

   Old:
   ```
   A `remove` that is refused because a reader or a client `pin` holds the entry, or because the entry stays write-locked past the dispatch-map wait, returns `KeyNotFound` and frees nothing.
   ```
   New:
   ```
   A `remove` that is refused because a reader or a client `pin` holds the entry, or because the entry stays write-locked past the dispatch-map wait, returns `KeyNotFound`, frees nothing and leaves the entry as it found it: a memory-tier resident entry is still MemoryTier, with the same slot and its `ssd_offset`, and keeps every reference, so a later lookup is served from DRAM. *(Sync 2026-10-23, branch `fix/dispatcher-remove-ssd-extent` — code authoritative, commit `765f75ba3`. To learn a resident entry's SSD offset, `remove` briefly flips it to BlockDevice before it tries the removal (FR-009), and restores it with `promote_block_to_memory_tier` if the removal is refused. While `remove` runs, a concurrent caller can therefore see the entry as BlockDevice; a cold lookup in that window takes the concurrent-promotion recovery (FR-051). Known gap, tracked as align task A-12: a concurrent `clear_memory_tier` in that window removes the entry, and the SSD extent is then not freed.)*
   ```

2. `specs/001-dispatcher-cache-interface/contracts/idispatcher.md:171`

   Old:
   ```
   - `remove()` guarantees, when it returns `Ok`, that the dispatch map entry is removed, so no new lookup can obtain its memory-tier pointer. When it returns `KeyNotFound`, nothing was freed.
   ```
   New:
   ```
   - `remove()` guarantees, when it returns `Ok`, that the dispatch map entry is removed, so no new lookup can obtain its memory-tier pointer. When it returns `KeyNotFound` because the entry is held, nothing was freed and the entry is unchanged: same location, same slot, same references (001 edge case "remove").
   ```

**Rationale (code authoritative)**: `765f75ba3` is a formal-verification repair. Its Loom test failed on the old code, the acceptance check passed, and it implements the earlier ALIGN proposal A-9, whose spec intent ("frees nothing and changes nothing") is stated here. The window sentence is written as a known gap, not as a guarantee, so the spec does not bless P-07.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal B-15: 001 FR-056 + 002 out-of-scope note — two stale `src/lib.rs` line citations

**Direction**: BACKFILL · **Finding**: stale citations (drift report, note and recommendation 8) · **Severity**: trivial · **Confidence**: HIGH · `approved: false`

**Requirement**: 001 FR-056, the `CERTUS_LOG_RESERVE` diagnostics note; 002 "Out of scope", the `GetIoStats` bullet.

**Spec says**:
- `001:344`: "*(Sync 2026-10-14 — backfilled note, code authoritative, `src/lib.rs:3380-3395`.)*"
- `specs/002-served-by-tier-attribution/spec.md:722`: "`GetIoStats` is device-level and drive-aggregated (`components/dispatcher/src/lib.rs:3257-3272`)"

**Code does**:
- `CERTUS_LOG_RESERVE` is read in `reserve_memory` at `src/lib.rs:3589-3604`. On the base it was at `:3525-3540`, so the citation was already stale; this branch moved it by a further +64 lines.
- `read_write_stats` (`GetIoStats`) is at `src/lib.rs:4226-4236`. The cited `:3257-3272` is now the body of `check`/`remove`.

**Proposed resolution**:

1. `specs/001-dispatcher-cache-interface/spec.md:344`

   Old:
   ```
   *(Sync 2026-10-14 — backfilled note, code authoritative, `src/lib.rs:3380-3395`.)*
   ```
   New:
   ```
   *(Sync 2026-10-14 — backfilled note, code authoritative, `src/lib.rs:3589-3604` as of 2026-10-23.)*
   ```

2. `specs/002-served-by-tier-attribution/spec.md:722`

   Old:
   ```
     is device-level and drive-aggregated (`components/dispatcher/src/lib.rs:3257-3272`) and
   ```
   New:
   ```
     is device-level and drive-aggregated (`components/dispatcher/src/lib.rs:4226-4236`, `read_write_stats`) and
   ```

**Rationale**: these citations point at unrelated code. The other spec-002 citations (`:2108`, `:2576`, …) are also off, but they were already off at `772885d93`, and they are measured historical references in the research and plan narrative. They are left for a separate refresh and are not part of this proposal.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal A-11: 001 FR-013 (and FR-018) — the SSD evictor must not release a read reference it never took

**Direction**: ALIGN · **Finding**: D-25 · **Severity**: moderate · **Confidence**: MEDIUM (from reading the code; the window is narrow and no schedule has been executed) · pre-existing, not caused by this branch · `approved: false`

**Requirement**:
- 001 FR-013 (`spec.md:298`): "The dispatcher MUST use appropriate read/write locking on the dispatch map to ensure thread safety during concurrent operations."
- FR-018 (`spec.md:303`): "A job already reading the DRAM slot finishes … In that case `remove` MUST NOT free the slot itself."
- FR-009 as amended by B-12: a lookup "MUST release only a read reference that lookup acquired".

**Code does**:
- `BackgroundEvictor::get_evictable_offset` (`src/background.rs:581-601`) runs for every SSD-evictor candidate (`:516`). Its catch-all arm `Ok(_) => { let _ = dm.release_read(key); None }` (`:595-598`) also runs on `LookupResult::NotExist`. `lookup` takes no reference there (`components/dispatch-map/src/lib.rs:142-143`).
- The candidates come from an `oldest_keys` snapshot (`src/background.rs:508`). A candidate removed after the snapshot answers `NotExist`, and if a store of the key then registers the entry (`create_memory_tier_entry`, `downgrade_reference` → write-through pin, `src/lib.rs:3784`, `:3843`, `:3852`) before the evictor's `release_read`, the release drops that pin.
- The ledger still believes the pin is held. A later `remove` then succeeds at `dm.remove` (`src/lib.rs:3340-3341`), bypassing the takeover, and frees the slot (`:3368-3370`) while the job reads it. An evictor or `clear_memory_tier` can do the same.
- This is the defect class of root cause #12, which `a7444f4ab` fixed in `remove`. It is the last such site in `src/`. The others are correct: `src/lib.rs:3256-3261`, `:3792-3797`, `:3953-3965`.

**Required code change**: in `get_evictable_offset`, release only for a found entry:
```rust
Ok(LookupResult::BlockDevice { offset }) => { let _ = dm.release_read(key); Some(offset) }
Ok(LookupResult::MemoryTier { .. }) => { let _ = dm.release_read(key); None }
// NotExist / MismatchSize: this lookup took no read reference.
Ok(_) => None,
Err(_) => None,
```

**Task**:
- Test first: reuse the registration-race fixture of `src/repair_test_remove_frees_ssd_extent.rs:991` (`arm_registration_race`). Call `BackgroundEvictor::get_evictable_offset` on a key whose populate lands between the lookup and the release. Assert `foreign_releases() == 0` and that the write-through pin survives.
- The test must fail on HEAD `765f75ba3`.
- Files: `src/background.rs`, plus a new test module or the existing one.
- Effort: small.

**Rationale**: the spec states the locking rule (FR-013) and, once B-12 is applied, the release rule for lookups. The code breaks both. The fix is local and mirrors the one already accepted for `remove`.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal A-12: 001 SC-003 + FR-038 — close the flip window of `remove` (A-9's option (a))

**Direction**: ALIGN · **Finding**: P-07 · **Severity**: minor · **Confidence**: MEDIUM (from reading the code; needs a concurrent `clear_memory_tier`) · introduced by `a7444f4ab`, not closed by `765f75ba3` · `approved: false`

**Requirement**:
- 001 SC-003 (`spec.md:396`): "Cache removal frees all associated resources (memory-tier slots and SSD extents) so that they can be reused."
- FR-038 (`spec.md:323`): `clear_memory_tier` demotes an entry that has an SSD copy and removes only an entry that has none.
- FR-009 as amended by B-12.

**Code does**:
- To learn a resident entry's offset, `remove` flips it to BlockDevice (`src/lib.rs:3305`) and releases its references (`:3306`, `:3310`) before `dm.remove` (`:3340`). In that window the entry is unreferenced and BlockDevice while its slot is still resident.
- A concurrent `clear_memory_tier` gets `InvalidState` from `try_evict_to_block` ("entry is not in memory-tier state", `components/dispatch-map/src/lib.rs:576-578`). It takes the force-remove arm: `dm.remove` and `mt.remove` (`src/lib.rs:4132-4146`).
- `remove` then gets `KeyNotFound` from `dm.remove`, returns `KeyNotFound` (`:3363`), and never reaches `remove_extent` (`:3372-3378`).
- Result:
  - the extent leaks, and with `format_on_init=false` the removed key comes back at the next start (`:2037`);
  - an entry with an SSD copy is dropped, not demoted;
  - the eviction telemetry counts the entry as `blocked_unpersisted` (`:1128-1135`).

**Required code change** (A-9's preferred option (a), the interface addition A-2 originally proposed):
- Add one dispatch-map operation that, under one lock hold, removes the entry only if it is unreferenced and returns its SSD offset (BlockDevice `offset`, or MemoryTier `ssd_offset`). For example, `IDispatchMap::remove_returning_offset(key) -> Result<Option<u64>, DispatchMapError>`, or `remove` returning the removed `Location`.
- In `remove`, use it in place of the flip, the second lookup and the restore (`src/lib.rs:3283-3336`, `:3355-3358`).
- Use the same result in `WriteThroughPins::remove_entry` (`src/background.rs:354-375`). A combined "consume one read reference and remove" variant also closes A-4(a).

**Task**:
- Test first: a persisted resident entry. `remove` and `clear_memory_tier` race, with a hook in the mock dispatch map between the flip and `dm.remove`. Afterwards, exactly one of them reports the entry, and the extent manager lists no extent for the key.
- The test must fail on HEAD `765f75ba3`.
- Keep `src/repair_test_remove_frees_ssd_extent.rs:941,991` and `src/repair_test_remove_refused_leaves_entry_unchanged.rs:875` passing.
- Files: `components/interfaces/src/idispatch_map.rs`, `components/dispatch-map/src/lib.rs`, `src/lib.rs` (`remove`), `src/background.rs` (`remove_entry`), the test mocks.
- Effort: medium.

**Rationale**: SC-003 and FR-038 are agreed requirements, and the window is an artefact of the mechanism `a7444f4ab` chose (no interface change). B-12 and B-14 describe outcomes, so they stay true after this change. B-14's "known gap" sentence is then removed.

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

## Not proposed (recorded for the reviewer)

- **A-9** (the earlier run): implemented by `765f75ba3` using option (c). Mark it done in `align-tasks.md`. Option (a) is carried as A-12.
- **A-2** (`align-tasks.md`): resolved in code by `a7444f4ab`. Close it when B-12 is applied. Its second criterion (a `format_on_init=false` re-initialize does not resurrect the key) has no dedicated test.
- **A-06** (`align-tasks.md`, FR-042, the SSD evictor's sender): resolved in code by #568 (`src/lib.rs:2180`, `src/background.rs:532`). Close it.
- **J26 P2** (an extent released twice after key reuse), named as open in `a7444f4ab`: not verified here. The restore adds one LOW-confidence key-reuse path. If the key is removed and re-stored inside a refused `remove`'s window, the restore's `promote_block_to_memory_tier` hits a MemoryTier entry and fails `InvalidState`, which is harmless. The other branch, a re-store that reaches BlockDevice first, was not shown to be reachable.
- **Still open, unchanged:** A-1, A-3 (decision), A-4, A-5, A-01, A-02, A-04, A-05, A-08, A-10, N-01.
- **Cross-component:** `dispatcher-p2p::remove` (`components/dispatcher-p2p/src/lib.rs:2367-2377`) still releases a read reference after `NotExist`, and never frees a resident entry's extent. Port A-12's form there, not the flip. Run the `dispatcher-p2p` sync.
- **Housekeeping:** this run left two empty scratch files in this folder, `drift-report.note.part` and `drift-report.body.part`. They can be deleted.
