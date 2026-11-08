# Drift Resolution Proposals — dispatcher

Generated: 2026-11-07T12:00:00Z · Branch: `fix/dispatcher-j33-repairs` (HEAD `f4579a41f`, merge-base with `origin/unstable` `7c12e427a`) · Based on: `drift-report.json` of the same run.

**Mode: BATCH. Every proposal is `approved: false`. Nothing was applied; no spec, source or interface file was edited.** Line numbers are at HEAD `f4579a41f`. Spec paths are relative to `components/dispatcher/specs/`. Code paths are relative to `components/dispatcher/` unless a crate is named.

**No proposal edits `components/interfaces`.** Two proposals (B-45, B-49) name an *optional* interfaces doc follow-up, marked **[interfaces — optional, re-stamps every component]**. Neither needs it.

**This run replaces the 2026-11-06 analysis** (git stash `j33 spec-sync analysis 2026-11-06 (before repair 7 out)`, HEAD `01d9ec387`). Since then the branch has three new commits. `209b01de5` takes fix 7 out. `9a79224a1` fixes A-14 in gpu-services. `f4579a41f` merges unstable, which touches no hashed input of this component. Every item of that analysis was checked again against today's code. The ids follow on from it:
- **Kept, re-verified, line numbers updated:** B-35…B-42, B-44…B-47, A-15…A-17.
- **Withdrawn (do not apply):** **B-43** and **A-13**, because fix 7 was taken out. A-13's finding is kept as note **N-13** below.
- **Closed in code:** **A-14**, by `9a79224a1`. It is replaced by the small spec note **B-48**.
- **New:** **B-48** (fix 2) and **B-49** (pre-existing; its only reader was added by fix 7 and removed again by the revert).

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 14 (B-35…B-42, B-44…B-49) |
| Align (Spec → Code) | 3 (A-15, A-16, A-17) |
| Human Decision | 0 (the two judgment calls are stated as options inside B-38 and B-45) |
| New Specs | 0 |
| Remove from Spec | 0 |

Align tasks this run closes:
- **A-04** (no-drives promote) is resolved in code by `60838384e`. B-41 removes the spec's reference to it, and on apply `align-tasks.md` marks it resolved.
- **A-14** (registration without SPDK) is resolved in code by `9a79224a1`. It was never written into `align-tasks.md`, so that file needs no edit.

### By fix

| Fix | Commit | Spec already right? | Proposals |
|---|---|---|---|
| 1 failed `initialize` keeps the two tunables | `11748983d` | no: FR-012 says nothing about it | B-35 |
| 2 pool registered with the GPU in every mode, failure = warning | `7ac879dd4` (+ gpu-services `9a79224a1`) | **yes** (FR-034: "after GPU and memory-tier are ready … logged as a warning"); only the CUDA-only case is unstated | B-48 (A-14 closed) |
| 3 read/write stats never decrease | `d65de1fef` | interface doc yes ("Monotonic"); spec FR-056 says nothing | B-36, A-16 |
| 4 eviction errors by real cause | `03b05d505` | intent yes (002 FR-033); variant mapping and the third cause unstated | B-37, B-38, A-17 |
| 5 exhausted map retry is an error, not `KeyNotFound`/remote | `b0720fe6c` | partly (FR-039 step 1); `errors.md` contradicts | B-39, B-40 |
| 6 no-drives promotion delivers, then flips in place | `60838384e` | no: FR-028 still records the old defect (A-04) | B-41, B-42 |
| 7 `lookup_async` real stream: **taken out** | `9ff85fa51` reverted by `209b01de5` | n/a: code is back to unstable's (sync copy, null stream) | none; B-43/A-13 withdrawn; note N-13 |
| 8 NVMe timeout fails only its own entry | `408a568d0` | **yes** (US-11 AS4 "that entry only"); FR-061 silent on how | B-44, A-15 |
| 9 route counters add up | `ec1b890ca` | partition yes; conflicts with remote-on-fetch inside FR-058 | B-45 |
| pre-existing | — | no | B-46, B-47, B-49 |

| Id | Type | Requirement | Fix | Approved |
|---|---|---|---|---|
| B-35 | BACKFILL | 001 FR-012 | 1 | false |
| B-36 | BACKFILL | 001 FR-056 `read_write_stats` | 3 | false |
| B-37 | BACKFILL | 002 FR-033 | 4 | false |
| B-38 | BACKFILL | 002 FR-035 | 4 (gap found while verifying it; pre-existing) | false |
| B-39 | BACKFILL | 001 FR-039 step (1) | 5 | false |
| B-40 | BACKFILL | 001 `contracts/errors.md` `Timeout`, `IoError` rows | 5 | false |
| B-41 | BACKFILL | 001 FR-028 (closes A-04) | 6 | false |
| B-42 | BACKFILL | 001 US-9 AS3 | 6 | false |
| B-44 | BACKFILL | 001 FR-061 | 8 | false |
| B-45 | BACKFILL | 001 FR-058, SC-017; 002 FR-025 | 9 | false |
| B-46 | BACKFILL | 001 FR-062, US-13 AS1 | pre-existing | false |
| B-47 | BACKFILL | 001 FR-001 | pre-existing | false |
| B-48 | BACKFILL | 001 FR-034 | 2 | false |
| B-49 | BACKFILL | 001 FR-037, FR-052 | 7 (revert; pre-existing on unstable) | false |
| A-15 | ALIGN | 001 FR-028 vs FR-062 | 8 (related, pre-existing) | false |
| A-16 | ALIGN | 001 FR-056 `read_write_stats` | 3 (residual) | false |
| A-17 | ALIGN | 002 FR-033 | 4 (residual) | false |

## Proposals

### Proposal B-35: 001-dispatcher-cache-interface/FR-012

**Direction**: BACKFILL · **Fix**: 1 (`11748983d`, DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE)

**Requirement**: what a failed `initialize` leaves behind.

**Current State**:
- Spec says (`001-dispatcher-cache-interface/spec.md:342`): "The `initialize` method MUST validate that the `dispatch_map` and `memory_tier` receptacles are bound before proceeding. Other receptacles are validated lazily on first use." Neither SC-005 (`:491`) nor US-5 AS2 says what a failed call leaves behind.
- Code does: `max_eviction_attempts` and `store_backpressure_ms` are stored only after every check has passed, just before `initialized` is published (`src/lib.rs:2371-2379`; the reason is in the comment at `:2142-2147`). The failure paths that now change nothing:
  - an unbound receptacle (`:2149-2155`);
  - an empty `data_pci_addrs` (`:2157-2161`);
  - drive creation (`:2167`).

  Tests: `src/repair_test_failed_initialize_changes_config.rs:208`, `:242`.

**Proposed Resolution**: replace `spec.md:342`.

Old:
> - **FR-012**: The `initialize` method MUST validate that the `dispatch_map` and `memory_tier` receptacles are bound before proceeding. Other receptacles are validated lazily on first use.

New:
> - **FR-012**: The `initialize` method MUST validate that the `dispatch_map` and `memory_tier` receptacles are bound before proceeding. Other receptacles are validated lazily on first use. An `initialize` that returns `Err` — an unbound required receptacle, an empty `data_pci_addrs`, or a drive-creation, partition or format error — MUST leave the dispatcher's tunables unchanged: `max_eviction_attempts` and `store_backpressure_ms` (FR-033) are committed only after every validation has passed, immediately before the dispatcher is marked initialized, so a rejected config never retunes a running dispatcher. *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `11748983d`, repair DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE, `src/lib.rs:2371-2379`. Before it both values were stored at the top of `initialize`.)*

**Rationale**: the code is the tested repair, and the spec says nothing either way. The new text covers the two tunables only. It does not claim that a failed `initialize` changes nothing at all: a drive list built before a later failure is not rolled back (`:2167-2171`).

**Confidence**: HIGH · **approved: false**

---

### Proposal B-36: 001-dispatcher-cache-interface/FR-056 (`read_write_stats`)

**Direction**: BACKFILL · **Fix**: 3 (`d65de1fef`, DIS-READ-WRITE-STATS-MONOTONIC)

**Requirement**: `read_write_stats` must never decrease.

**Current State**:
- Spec says (`spec.md:396`): "`read_write_stats() -> ReadWriteStats` returns aggregate read/write I/O counters (the `ReadWriteStats` type is shared from `iblock_device`)." The interface doc already says "Monotonic; take deltas across two calls" (`components/interfaces/src/idispatcher.rs:936-942`).
- Code does:
  - The field `rw_stats_baseline` (`src/lib.rs:446`) keeps the counters of drives that are gone.
  - `absorb_drive_rw_stats` (`:1831-1837`) adds the current drives' counters to it at two points: before `shutdown` drops the drives (`:2438-2440`), and before a re-`initialize` replaces them (`:2168-2171`).
  - `read_write_stats` starts from the baseline and adds the live drives (`:4595-4608`).

  Test: `src/repair_test_rw_stats_after_shutdown.rs:620`.

**Proposed Resolution**: replace `spec.md:396`.

Old (a four-space sub-sub-bullet):
>     - `read_write_stats() -> ReadWriteStats` returns aggregate read/write I/O counters (the `ReadWriteStats` type is shared from `iblock_device`).

New:
>     - `read_write_stats() -> ReadWriteStats` returns aggregate read/write I/O counters (the `ReadWriteStats` type is shared from `iblock_device`), summed over every data drive. They are zero unless the dispatcher and its block devices are built with the `rw-telemetry` / `telemetry` features. The counters are cumulative over the dispatcher component's lifetime and MUST NOT decrease between two calls: before `shutdown` drops the data drives, and before a re-`initialize` replaces them, the outgoing drives' counters are folded into a retained baseline, and each call returns that baseline merged with the live drives. *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `d65de1fef`, repair DIS-READ-WRITE-STATS-MONOTONIC, `src/lib.rs:1831-1837`, `:4595-4608`. Before it the counters read zero after `shutdown`. Known gap, tracked as A-16: a call made while the drive list is being handed over can count the outgoing drives twice, so the next call can read lower.)*

**Rationale**: the code now does what the interface already promises. Only the spec lacked the rule.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-37: 002-served-by-tier-attribution/FR-033

**Direction**: BACKFILL · **Fix**: 4 (`03b05d505`, DIS-TIER-STATS-BLOCKED-CAUSES)

**Requirement**: each refused clean eviction is counted by its real cause.

**Current State**:
- Spec says (`002-served-by-tier-attribution/spec.md:573-578`): "`evictions_blocked_by_pin` when `IDispatchMap::try_evict_to_block` returns `ActiveReferences`, and `evictions_blocked_unpersisted` when it refuses for want of an `ssd_offset`. The call site MUST match the error variant…". It also says (`:587-590`): "MUST be counted, not inferred … counting both makes their sum against the scanned total a check that the two explanations are exhaustive."
- Code does:
  - `ActiveReferences` → counted as blocked by pin (`src/lib.rs:1222-1224`).
  - `InvalidState` → counted as unpersisted (`:1225-1234`). This is the dispatch map's answer for a memory-tier entry with no `ssd_offset` (`components/dispatch-map/src/lib.rs:571-575`).
  - Any other refusal → counted as **neither** (`:1235-1243`).

  Before the fix, every refusal that was not a pin counted as unpersisted, including `KeyNotFound` for a slot whose entry is not registered yet. Test: `src/repair_test_eviction_errors_counted_unpersisted.rs:273`.

**Proposed Resolution**: replace `002-…/spec.md:574-578`.

Old:
> causes applied, as separate counters on `TierEventStats`: `evictions_blocked_by_pin` when
> `IDispatchMap::try_evict_to_block` returns `ActiveReferences`, and
> `evictions_blocked_unpersisted` when it refuses for want of an `ssd_offset`. The call site MUST
> match the error variant; testing `is_ok()` and incrementing a single "eviction failed" counter
> does NOT satisfy this.

New:
> causes applied, as separate counters on `TierEventStats`: `evictions_blocked_by_pin` when
> `IDispatchMap::try_evict_to_block` returns `ActiveReferences`, and
> `evictions_blocked_unpersisted` when it refuses for want of an `ssd_offset` (the dispatch map
> answers `InvalidState` for a memory-tier entry without one). The call site MUST
> match the error variant; testing `is_ok()` and incrementing a single "eviction failed" counter
> does NOT satisfy this. A refusal that is neither — e.g. `KeyNotFound` for a memory-tier slot
> whose dispatch-map entry is not registered yet — MUST increment **neither** counter, so that a
> third cause shows up as the two counters falling short of the scanned total rather than being
> absorbed into one of them. In `dispatcher` both counters are recorded by the foreground
> clean-eviction scan (`evict_one_clean`); for the background evictor see FR-035.
> *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit
> `03b05d505`, `components/dispatcher/src/lib.rs:1215-1244`. Known gap, tracked as A-17:
> `InvalidState` is also the answer for an entry that is not in memory-tier state, which is
> still counted as unpersisted.)*

**Rationale**: the code applies the reason FR-033 already gives ("subtraction silently absorbs any third cause"). The spec only did not say what to do with a third cause.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-38: 002-served-by-tier-attribution/FR-035 (declare `dispatcher`'s background-evictor gap)

**Direction**: BACKFILL · **Fix**: 4 (found while checking it; the gap was there before the fix)

**Requirement**: FR-035 says a gap in these counters must be declared for as long as it exists.

**Current State**:
- Spec says (`002-…/spec.md:599-602`), of `dispatcher-p2p` only: "Its **background** `MemoryTierEvictor` is still not counted … The remaining gap MUST stay declared in both that component's code and its spec, and here, for as long as it exists."
- Code does: `dispatcher`'s own background `MemoryTierEvictor` counts neither cause either: `if dm.try_evict_to_block(key).is_err() { continue; }` (`src/background.rs:736-738`). The only places the counters are recorded are in `evict_one_clean` (`src/lib.rs:1222-1234`).

**Proposed Resolution**: insert a new sub-bullet in FR-035 after `spec.md:604`. The lines on either side do not change.

Old (`spec.md:603-605`):
>   - *Previously (2026-10-02 to 2026-10-09):* p2p reported none of FR-033/FR-034, because those
>     eviction paths were free functions holding no counter handle.
>   - This is distinguished deliberately from the route-counter defect recorded in SC-008: reporting

New:
>   - *Previously (2026-10-02 to 2026-10-09):* p2p reported none of FR-033/FR-034, because those
>     eviction paths were free functions holding no counter handle.
>   - **`dispatcher` has the same gap.** Its background `MemoryTierEvictor` skips a refused
>     candidate without recording either cause (`components/dispatcher/src/background.rs:736-738`),
>     so in `dispatcher` too `evictions_blocked_by_pin` and `evictions_blocked_unpersisted` count
>     the foreground scan only, and are a lower bound whenever `memory_tier_eviction_threshold > 0`
>     starts that evictor. Declared here under this requirement's own rule. *(Sync 2026-11-07,
>     branch `fix/dispatcher-j33-repairs` — pre-existing, found while verifying `03b05d505`.)*
>   - This is distinguished deliberately from the route-counter defect recorded in SC-008: reporting

**Rationale**: FR-035 requires such a gap to be declared. The other option, not taken here, is a code change: count the two causes in the background evictor the way the foreground scan does. That would remove both the gap and this bullet. Either is valid. The backfill is the smaller change, and it is what FR-035 requires while the gap exists.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-39: 001-dispatcher-cache-interface/FR-039 step (1)

**Direction**: BACKFILL · **Fix**: 5 (`b0720fe6c`, DIS-BATCH-LOOKUP-TRANSIENT-RETRY)

**Requirement**: in `batch_lookup`, a map error that is still there after the retries is an error. It is not `KeyNotFound`, and it is not sent to remote lookup.

**Current State**:
- Spec says (`spec.md:369`, step (1)): "…so it is retried locally a few times (up to 5) rather than collapsed into `KeyNotFound`; collapsing it would forward a live key to remote-lookup…". It does not say what the entry returns when **every** retry fails.
- Code does: after the retries (`src/lib.rs:2669-2678`):
  - `DispatchMapError::Timeout` becomes `DispatcherError::Timeout` (`:2748-2752`);
  - any other map error becomes `IoError` (`:2753-2755`).

  Neither is `KeyNotFound`, so neither is sent on: the remote pass takes only `KeyNotFound` (`:3130-3137`). The attribution is `ERROR` (`:3349`). Test: `src/repair_test_transient_map_error.rs:683`, which asserts `ServedBy::Error` (`:780`).

**Proposed Resolution**: in `spec.md:369`, change one clause of step (1).

Old:
> so it is retried locally a few times (up to 5) rather than collapsed into `KeyNotFound`; collapsing it would forward a live key to remote-lookup, whose miss becomes a fatal load failure in the vLLM connector (`EngineDeadError`) *(Sync 2026-09-02 — code authoritative, commit `1d55b9c2`; this retry is defensive and does not fire under the current workload, where `write_ref` windows are sub-millisecond)*,

New:
> so it is retried locally a few times (up to 5) rather than collapsed into `KeyNotFound`; collapsing it would forward a live key to remote-lookup, whose miss becomes a fatal load failure in the vLLM connector (`EngineDeadError`) *(Sync 2026-09-02 — code authoritative, commit `1d55b9c2`; this retry is defensive and does not fire under the current workload, where `write_ref` windows are sub-millisecond)*. If every retry still fails, the entry's result is the dispatch map's own error and never `KeyNotFound`: `Timeout` for a dispatch-map timeout, `IoError` for any other map error. It is attributed `ERROR` (spec 002 FR-007) and is not forwarded to remote lookup, which takes only `KeyNotFound` results (FR-045) — the rule `check` follows (FR-008) *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `b0720fe6c`, `src/lib.rs:2743-2756`)*,

**Rationale**: the code finishes the argument step (1) already makes. The old text left the exhausted case open to the very collapse it forbids.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-40: 001-dispatcher-cache-interface/`contracts/errors.md` (`Timeout`, `IoError` rows)

**Direction**: BACKFILL · **Fix**: 5 (`b0720fe6c`). The `check` half has been there since `898bba7eb` (FR-008).

**Requirement**: the error contract must list who returns `Timeout` and `IoError`.

**Current State**:
- Spec says (`contracts/errors.md:37`): "| `Timeout` | `pin`, `clear_memory_tier` | … *(… `lookup` and `remove` never return `Timeout`; they report a dispatch-map timeout as `KeyNotFound`)* |". Row `:36` lists `IoError` for "`lookup`, `batch_lookup`, `initialize`, `commit_store`, `flush_to_ssd`".
- Code does:
  - `batch_lookup` returns `Timeout` for an entry whose retries are used up (`src/lib.rs:2748-2752`).
  - `check` returns `Timeout` (`:3538-3540`) and `IoError` (`:3541`).
  - `lookup_async` and `remove` still turn a map error into `KeyNotFound` (`:3495`, `:3612`, `:3640`), so the existing parenthesis stays true.

**Proposed Resolution**: two cell edits.

Old (`errors.md:37`, first two cells and the start of the third):
> | `Timeout` | `pin`, `clear_memory_tier` | `pin`: the dispatch-map wait for a writer exceeded its timeout.

New:
> | `Timeout` | `pin`, `clear_memory_tier`, `check`, `batch_lookup` | `pin`: the dispatch-map wait for a writer exceeded its timeout. `check` (FR-008) and `batch_lookup` (per entry, FR-039 step 1): every retry of the dispatch-map lookup timed out behind a writer; the key is live and its presence unknown, so it is not reported absent *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs`, commit `b0720fe6c`; `check` since `898bba7eb`)*.

The rest of the row, from "`clear_memory_tier`: the tier could not be emptied …" to the end, does not change.

Old (`errors.md:36`, second cell):
> `lookup`, `batch_lookup`, `initialize`, `commit_store`, `flush_to_ssd`

New:
> `lookup`, `batch_lookup`, `check`, `initialize`, `commit_store`, `flush_to_ssd`

Also add to the end of that row's condition cell: " **For `check` and `batch_lookup`, also** a dispatch-map error other than a timeout that persists through every retry (FR-008, FR-039 step 1)."

**Rationale**: the contract contradicts the shipped code, so a caller who trusts it would not handle `Timeout` from `batch_lookup`. Not proposed, recorded only: the `Timeout` doc text "exceeded the 100ms timeout" (`errors.md:21`, copied from `components/interfaces/src/idispatcher.rs:178`) is out of date for every method that raises it. Fixing it in `interfaces` would re-stamp every component.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-41: 001-dispatcher-cache-interface/FR-028 (closes align task A-04)

**Direction**: BACKFILL · **Fix**: 6 (`60838384e`, DIS-LOOKUP-NO-HW-PROMOTE, DIS-BATCH-LOOKUP-COLD-PROMOTES)

**Requirement**: with no drives, a promotion must not mark an SSD entry as in memory without first delivering the slot.

**Current State**:
- Spec says (`spec.md:358`, last sentence): "*(Exception, tracked as align task A-04: the no-drives `promote_and_serve` does not re-register the entry and keeps its slot.)*"
- Code does, with no data drives, in `promote_and_serve`:
  1. copies the fresh slot to the GPU (`src/lib.rs:890-900`);
  2. if the copy fails, releases the slot and fails (`:901-911`);
  3. flips the entry in place with `promote_block_to_memory_tier` (`:915`);
  4. if the flip fails, releases the slot and fails (`:915-922`);
  5. counts the promotion (`:923`).

  `batch_lookup`'s no-drives loop now delivers the regions (`serve_memory_tier_to_gpu`, `:2815-2828`) before the same in-place flip (`:2830-2842`). Before the fix it flipped first and delivered nothing. Tests: `src/repair_test_no_drives_promote_without_read.rs:582`, `:613`, `:645`, `:680`, `:724`. Both of A-04's criteria are met: the entry is MemoryTier and the slot is referenced, and the mock refuses an overwrite (`src/lib.rs:5268-5277`).

**Proposed Resolution**: replace in `spec.md:358`.

Old:
> *(Exception, tracked as align task A-04: the no-drives `promote_and_serve` does not re-register the entry and keeps its slot.)*

New:
> With no data drives (memory-tier-only mode) there is no SSD read: the freshly reserved slot stands in for it, is delivered to the entry's GPU destination(s), and the entry is then re-registered in place with `promote_block_to_memory_tier` — in `promote_and_serve` and in `batch_lookup` alike. A failed delivery or transition releases the slot and fails that key, as above. *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `60838384e`, `src/lib.rs:880-925`, `:2789-2847`; this closes align task A-04. Before it `promote_and_serve` called `create_memory_tier_entry`, which refuses the existing BlockDevice entry, and `batch_lookup` flipped the entry without delivering the block.)*

**Also**, outside `specs/`: on apply, mark A-04 in `.specify/sync/align-tasks.md` as "✅ RESOLVED by `60838384e`".

**Rationale**: the spec records a defect this branch fixed. Leaving it would mislead the next verification run.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-42: 001-dispatcher-cache-interface/User Story 9, acceptance scenario 3

**Direction**: BACKFILL · **Fix**: 6 (`60838384e`)

**Requirement**: the same as B-41, as an acceptance scenario.

**Current State**:
- Spec says (`spec.md:182`): "3. **Given** no hardware is available (memory-tier-only mode), **When** promote_and_serve runs, **Then** it creates a memory-tier slot with zero-copied data, performs a direct GPU DMA from the slot, and re-registers the entry."
- Code does:
  - The slot is not zero-filled. There is no SSD read, so the slot holds whatever the tier hands out.
  - The slot is delivered through `dma_copy_to_device` or `serve_memory_tier_to_gpu`.
  - The entry is re-registered with `promote_block_to_memory_tier`.
  - On failure the slot is released and an error returned (`src/lib.rs:880-925`, `:2789-2847`).

  `batch_lookup`, which the scenario does not mention, does the same.

**Proposed Resolution**: replace `spec.md:182`.

Old:
> 3. **Given** no hardware is available (memory-tier-only mode), **When** promote_and_serve runs, **Then** it creates a memory-tier slot with zero-copied data, performs a direct GPU DMA from the slot, and re-registers the entry.

New:
> 3. **Given** no data drives (memory-tier-only mode), **When** a cold lookup promotes a BlockDevice entry (`lookup`/`lookup_async` via `promote_and_serve`, or `batch_lookup`), **Then** it reserves a memory-tier slot (there is no SSD read; the slot's contents stand in for the block), copies the slot to the GPU destination, and re-registers the entry in place as MemoryTier (`promote_block_to_memory_tier`); if the copy or the transition fails, the slot is released, the entry stays BlockDevice and the lookup fails (FR-028). *(Sync 2026-11-07, commit `60838384e`.)*

**Rationale**: "zero-copied data" is wrong, because nothing zeroes the slot. The scenario also named only one of the two paths the fix changed.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-44: 001-dispatcher-cache-interface/FR-061 (drain-all-on-error bullet)

**Direction**: BACKFILL · **Fix**: 8 (`408a568d0`, DIS-BATCH-LOOKUP-ERROR-ISOLATION)

**Requirement**: one NVMe read timeout fails only the entries of that read.

**Current State**:
- Spec says (`spec.md:466`): "Preserve the **drain-all-on-error** invariant of FR-054 per drive: never break early while reads are in flight; retain and report the first error per job while continuing to drain outstanding completions." User Story 11 AS4 (`:217`) already requires "the error is reported for that entry only", so the fix matches the scenario. FR-061 does not say how a timeout, which names no job, is attributed.
- Code does, in the scatter-gather loop:
  - `Completion::Timeout { handle }` is counted, submission carries on, and only the message is kept (`src/pipeline.rs:1402-1420`).
  - After the loop, each object that still has segments outstanding is failed on its own with that message (`:1481-1495`).
  - An invalid tag, an unexpected completion kind, or a disconnected channel still fails every job of the drive and stops its submissions (`:1314-1320`, `:1421-1434`, `:1448-1463`).

  The other two cold readers still fail every object of the call on a timeout (`pipelined_multi_object_zero_copy` `:809-820`; `pipelined_ssd_to_dram_only` `:1645-1650`). Their callers pass one entry per call (`src/lib.rs:997-1012`, `:4364`), so that failure is per entry too. Tests: `src/repair_test_nvme_timeout_fails_whole_drive.rs:794`, `:817`, `:840`, `:862`.

**Proposed Resolution**: replace `spec.md:466`.

Old:
> - Preserve the **drain-all-on-error** invariant of FR-054 per drive: never break early while reads are in flight; retain and report the first error per job while continuing to drain outstanding completions.

New:
> - Preserve the **drain-all-on-error** invariant of FR-054 per drive: never break early while reads are in flight; retain and report the first error per job while continuing to drain outstanding completions. A read timeout (`Completion::Timeout`) carries only the actor's operation handle, not the job tag, and the actor forgets the timed-out operation; it MUST NOT fail the drive's other jobs. Submission and draining continue, and after the loop each job whose segments did not all complete is reported `Err` with the timeout message — that job only (User Story 11 scenario 4). Completions that indicate a broken channel rather than a failed read — an invalid tag, an unexpected completion kind, a disconnected completion channel — still fail every job of that drive and stop its submissions. *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `408a568d0`, `src/pipeline.rs:1402-1420`, `:1481-1495`. Before it one timeout failed every job of its drive. Known gap, tracked as A-15: the timed-out job's memory-tier slot is then released at once, although the drive may still write into it.)*

**Rationale**: this makes explicit the attribution rule a verifier needs. It also records the channel-fault cases, which the fix report said need their own spec decision, instead of leaving them unstated.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-45: 001-dispatcher-cache-interface/FR-058 + SC-017; 002-served-by-tier-attribution/FR-025

**Direction**: BACKFILL · **Fix**: 9 (`ec1b890ca`, DIS-TIER-STATS-ROUTE-PARTITION)

**Requirement**: the route counters add up: single lookups are counted now, and remote hits are still counted when the peer answers.

**Current State**:
- Spec says:
  - FR-058 (`spec.md:405`): "`remote_lookup_hits` (… counted requester-side when the remote fetch succeeds — **not** when the block is subsequently delivered to the GPU …)". In the same requirement it also says "`lookup_hits_dram` and `lookup_hits_ssd`, which with `remote_lookup_hits` **partition** every served key by route: the three MUST sum to the served count for any interval."
  - SC-017 (`:510`): "`lookup_hits_dram + lookup_hits_ssd + remote_lookup_hits` MUST equal the served-key count over the same interval".
  - 002 FR-025 (`002-…/spec.md:554-555`): "The aggregate counters MUST agree with the per-entry attribution in the same responses."
- Code does:
  - The fix adds the local half of the partition to the single-key path. A memory-tier hit counts as DRAM (`src/lib.rs:3434-3443`). A cold promotion counts as SSD, and a concurrent-promotion recovery counts as DRAM (`:3465`, `:3476-3490`). `batch_lookup` already did this (`:3360-3366`). The fix 7 revert kept these counters (`209b01de5`). Tests: `src/repair_test_route_partition_does_not_add_up.rs:544`, `:591`, `:633`, `:684`, `:717`.
  - **Remote hits are still counted at fetch**: `remote_hits += 1` at `:3212-3215`, before `dm.lookup` and the GPU copy (`:3217-3285`). If delivery fails afterwards, the result is `Err` (`:3248`, `:3253-3267`, `:3276-3284`) and the attribution is `ERROR` (`:3349`).

  For such a key the two FR-058 sentences and FR-025 cannot all hold. The fix report left this on purpose (`verif/fixes/fix_dispatcher_route-partition-does-not-add-up.md:277-280`, `:313-316`).

**Proposed Resolution**: three edits.

(1) `spec.md:405`. Old:
> `lookup_hits_dram` and `lookup_hits_ssd`, which with `remote_lookup_hits` **partition** every served key by route: the three MUST sum to the served count for any interval.

New:
> `lookup_hits_dram` and `lookup_hits_ssd`, which with `remote_lookup_hits` **partition** every served key by route: the three MUST sum to the served count for any interval, on both lookup paths — `batch_lookup` and the single-key `lookup`/`lookup_async` (memory-tier hit → DRAM, cold promotion → SSD, concurrent-promotion recovery → DRAM). One exception follows from counting remote hits at fetch (above): a key a peer returned whose local delivery then fails is counted in `remote_lookup_hits` but is not served (its result is `Err`, attributed `ERROR`), so over an interval with such failures the sum exceeds the served count by exactly their number. *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `ec1b890ca`, repair DIS-TIER-STATS-ROUTE-PARTITION, `src/lib.rs:3434-3443`, `:3476-3490`: before it the single-key path counted `promotions_to_gpu` but no route counter. The remote-on-fetch exception is pre-existing and kept deliberately.)*

(2) `spec.md:510`. Old:
> And `lookup_hits_dram + lookup_hits_ssd + remote_lookup_hits` MUST equal the served-key count over the same interval — the partition property, which is checkable without any knowledge of the workload.

New:
> And `lookup_hits_dram + lookup_hits_ssd + remote_lookup_hits` MUST equal the served-key count over the same interval, single-key lookups included, plus the number of keys a peer returned whose local delivery failed in that interval (FR-058); with no such failure it equals the served-key count exactly — the partition property, which is checkable without any knowledge of the workload.

(3) `002-served-by-tier-attribution/spec.md:554-555`. Old:
> - **FR-025**: The aggregate counters MUST agree with the per-entry attribution in the same
>   responses.

New:
> - **FR-025**: The aggregate counters MUST agree with the per-entry attribution in the same
>   responses. One deliberate exception: `remote_lookup_hits` counts a key when the peer returns it,
>   so a key whose local delivery then fails is attributed `ERROR` but still counted as a remote hit
>   (spec 001 FR-058). *(Sync 2026-11-07.)*

**Option not taken (it would be an ALIGN, with a code and interface change):** count remote hits only once delivery succeeds. That makes the partition exact. But it changes what `remote_lookup_hits` is documented to mean in FR-058 and in the interface (`components/interfaces/src/idispatcher.rs:375-377`), and it would re-stamp every component. Not proposed.

**[interfaces — optional, re-stamps every component]** The `TierEventStats::lookup_hits_dram` doc (`components/interfaces/src/idispatcher.rs:346-349`, "so the three sum to the served count") could name the same exception. This proposal does not need it.

**Rationale**: two sentences of the spec conflict. The backfill states exactly what the code does and keeps the partition checkable.

**Confidence**: HIGH that this is the shipped behaviour; MEDIUM that the exception, not the code change, is the wanted resolution · **approved: false**

---

### Proposal B-46: 001-dispatcher-cache-interface/FR-062 + User Story 13 scenario 1

**Direction**: BACKFILL · **Fix**: none (pre-existing; found by the full sweep)

**Requirement**: which drive `read_to_bounce` reads.

**Current State**:
- Spec says (`spec.md:419-421`): "The read targets the drive chosen by the write path's placement hash (`drive_index`), at LBA `offset / block_size`…". US-13 AS1 (`:259-262`) says: "the extent is read from drive `drive_index(key, N)` at LBA `offset / block_size`".
- Code does: it reads the drive recorded in the SSD location (`select_drive`), at the decoded raw offset. A location that names a missing drive returns `BounceError::Io` without reading (`src/lib.rs:4647-4670`). `drive_index` no longer exists anywhere in `src/`. Tests: `src/lib.rs:8497`, `:8541`.

**Proposed Resolution**: two edits.

Old (`spec.md:419-421`, a two-space sub-bullet with four-space continuation lines):
>   - The read targets the drive chosen by the write path's placement hash (`drive_index`), at
>     LBA `offset / block_size`, `len` rounded up to the block size, segmented by the drive's
>     maximum transfer, over a channel checked out of that drive's `ChannelPool`.

New:
>   - The read targets the drive recorded in the entry's SSD location (`select_drive`, bits 56-63,
>     FR-039 step (4)), at LBA `raw_offset / block_size`, `len` rounded up to the block size,
>     segmented by the drive's maximum transfer, over a channel checked out of that drive's
>     `ChannelPool`. A location that names a drive that does not exist returns `Io` without
>     reading. *(Sync 2026-11-07 — code authoritative since the 2026-10-27 rebase onto
>     `4bb69289`, `src/lib.rs:4647-4670`; `drive_index` no longer exists.)*

Old (`spec.md:260`, a continuation line with three leading spaces):
>    extent is read from drive `drive_index(key, N)` at LBA `offset / block_size`, segmented by

New:
>    extent is read from the drive its SSD location records, at LBA `raw_offset / block_size`, segmented by

**Rationale**: the spec names a function that was removed. Reading by key hash would read another entry's data, which is why the code changed.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-47: 001-dispatcher-cache-interface/FR-001 (method inventory)

**Direction**: BACKFILL · **Fix**: none (pre-existing)

**Requirement**: FR-001 calls itself "the full shipped method inventory".

**Current State**:
- Spec says (`spec.md:326`): "**Write / cache management**: `populate`, `batch_populate`, `remove`, `touch`, `promote_to_memory_tier`." It also says (`:328`): "**Durability / introspection** (see FR-056): `flush_to_ssd`, `schedule_write_through`, `read_write_stats`."
- Code does: the trait also declares `clear_memory_tier` (`components/interfaces/src/idispatcher.rs:862`, FR-038), `clear_all` (`:889`, FR-063) and `tier_event_stats` (`:950`, FR-058). They are implemented at `src/lib.rs:4404`, `:4470` and `:4610`.

**Proposed Resolution**:

Old (`spec.md:326`):
>   - **Write / cache management**: `populate`, `batch_populate`, `remove`, `touch`, `promote_to_memory_tier`.

New:
>   - **Write / cache management**: `populate`, `batch_populate`, `remove`, `touch`, `promote_to_memory_tier`, `clear_memory_tier` (FR-038), `clear_all` (FR-063).

Old (`spec.md:328`, start of the line):
>   - **Durability / introspection** (see FR-056): `flush_to_ssd`, `schedule_write_through`, `read_write_stats`.

New (the rest of the line, from " *(Sync 2026-10-05 …", does not change):
>   - **Durability / introspection** (see FR-056): `flush_to_ssd`, `schedule_write_through`, `read_write_stats`, `tier_event_stats` (FR-058).

**Rationale**: a list that says it is complete should be. Each of these methods is already specified by its own FR.

**Confidence**: HIGH · **approved: false**

---

### Proposal B-48: 001-dispatcher-cache-interface/FR-034 (registration in every mode)

**Direction**: BACKFILL (FR-034 was already right; this only states two cases precisely) · **Fix**: 2 (`7ac879dd4`, DIS-INITIALIZE-REGISTER-HOST-MEMORY). This proposal replaces the closed A-14.

**Requirement**: host memory is registered with the GPU in every mode, and a failed registration is a warning, not a failure.

**Current State**:
- Spec says (`spec.md:364`): "During `initialize()`, after GPU and memory-tier are ready, the dispatcher MUST call `IGpuServices::register_host_memory` on the memory-tier pool to CUDA-pin and SPDK-register it for zero-copy NVMe and GPU DMA. Registration failure MUST be logged as a warning (zero-copy pipelines require this registration)." This is already what fix 2 does. **Aligned.**
- Code does:
  - In the dispatcher, registration runs whenever `gpu_services` and `memory_tier` are bound. It is no longer limited to when an SPDK environment is connected (`src/lib.rs:2246-2270`).
  - A failure goes to `log_warn`, and `initialize` goes on to return `Ok` (`:2263-2266`). Tests: `src/repair_test_register_host_memory.rs:408`, `:432`.
  - In gpu-services, since `9a79224a1` (the A-14 fix), `register_host_memory` calls `spdk_mem_register` only when `interfaces::is_spdk_env_active()`. Otherwise it returns `Ok` after the CUDA pin alone (`components/gpu-services/src/lib.rs:1176-1181`). `unregister_host_memory` skips the SPDK half the same way (`:1218-1225`). Before this, memory-tier-only mode crashed `initialize` in `spdk_mem_register` (SIGSEGV). Per the commit message, this was observed on node7 and is covered by `components/gpu-services/tests/host_memory_without_spdk.rs`. That test needs a real GPU and was not run here.
  - The comment "Only when SPDK is available — spdk_mem_register segfaults otherwise" at `src/lib.rs:2200` now sits above the pipeline-ring block only. It is still true there, but it no longer applies to registration.

**Proposed Resolution**: in `spec.md:364`, change the second sentence.

Old:
> Registration failure MUST be logged as a warning (zero-copy pipelines require this registration).

New:
> Registration failure MUST be logged as a warning and MUST NOT fail `initialize` (zero-copy pipelines require this registration; the DRAM path does not). Registration runs whenever the GPU services and the memory tier are bound — with or without an SPDK environment or data drives, so factory-mode and memory-tier-only deployments are registered too; without an active SPDK environment `register_host_memory` only CUDA-pins the pool (there is no SPDK memory map to register in), and `unregister_host_memory` (FR-035) skips the SPDK half likewise. *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `7ac879dd4` (repair DIS-INITIALIZE-REGISTER-HOST-MEMORY, `src/lib.rs:2246-2270`; before it the pool was registered only when `spdk_env` was connected) and gpu-services commit `9a79224a1` (`components/gpu-services/src/lib.rs:1176-1181`, `:1218-1225`; before it memory-tier-only mode crashed `initialize` in `spdk_mem_register`, align task A-14 of the 2026-11-06 analysis, now closed).)*

**Rationale**: the code now registers in modes the spec never mentioned, and in one of them only half the registration ("CUDA-pin and SPDK-register") happens. The spec should say so, so that "SPDK-register" is not read as required in memory-tier-only mode. No interfaces change is needed. The `IGpuServices::register_host_memory` doc belongs to gpu-services and interfaces, and is left to that component's own sync.

**Confidence**: HIGH (the code paths were read; the hardware test was not run here) · **approved: false**

---

### Proposal B-49: 001-dispatcher-cache-interface/FR-037 + FR-052 (the shared `warm_stream` is not a fallback)

**Direction**: BACKFILL · **Fix**: related to 7. The gap is pre-existing on unstable. Fix 7 added the only reader of `warm_stream`, and the revert `209b01de5` removed it again.

**Requirement**: what happens when the destination GPU is unknown.

**Current State**:
- Spec says:
  - FR-037 (`spec.md:367`): "A single `AtomicU64` warm stream (`warm_stream`) is retained as a fallback;".
  - FR-052 (`:382`): "When the device is unknown (`-1`) the paths fall back to the shared pipeline-ring / `warm_stream` streams."
- Code does:
  - `warm_stream` is written at `initialize` (`src/lib.rs:2205`) and swapped out at `shutdown` (`:2425`). Nothing ever reads it. `grep warm_stream src/lib.rs` finds only the declaration `:448`, those two sites, and a test name.
  - When the device is unknown, `device_streams_for` returns `None` (`:505-508`), so `warm_raw = 0` (`:2606`, `:3399`, `:1164`). The warm paths (`batch_lookup` warm hit, single-key `lookup_async` hit, remote delivery, concurrent-promotion recovery, staged multi-region scatter) then take the synchronous `DmaBuffer` + `dma_copy_to_device` branch of `serve_memory_tier_to_gpu` (`:1483-1519`).
  - The cold paths do fall back to the pipeline-ring streams (`promote_and_serve` `:974-985`, `serve_cold_staged` `:1119-1125`). In `batch_lookup`'s scatter-gather they fall back to two newly created streams (`:2958-2964`).
  - On `origin/unstable` (`7c12e427a`) `warm_stream` is likewise only stored and swapped. The 2026-10-31 sync marked FR-037 aligned without checking for a reader.

**Proposed Resolution**: two edits.

Old (`spec.md:367`, inside FR-037):
> A single `AtomicU64` warm stream (`warm_stream`) is retained as a fallback;

New:
> A single `AtomicU64` warm stream (`warm_stream`) is still created at `initialize()` and destroyed at `shutdown()`, but no copy path uses it: a warm copy whose destination device is unknown takes the synchronous `dma_copy_to_device` fallback (FR-006, FR-052) *(Sync 2026-11-07 — pre-existing, `src/lib.rs:2205`, `:2425`; no reader)*;

Old (`spec.md:382`, last sentence of FR-052):
> When the device is unknown (`-1`) the paths fall back to the shared pipeline-ring / `warm_stream` streams.

New:
> When the device is unknown (`-1`), the single-entry cold paths (`promote_and_serve`, `serve_cold_staged`) fall back to the shared pipeline-ring streams, `batch_lookup`'s scatter-gather creates two streams for the call, and the warm (memory-tier→GPU) paths use no stream at all — they copy synchronously through `dma_copy_to_device` (FR-006). *(Sync 2026-11-07 — code authoritative, pre-existing, `src/lib.rs:974-985`, `:1119-1125`, `:2953-2964`, `:1483-1519`.)*

**Option not taken (ALIGN):** either delete the unused `warm_stream` field, or route warm copies for an unknown device to it. The second was part of fix 7 and went out with it. That choice belongs to the separate `lookup_async` issue (N-13).

**[interfaces — optional, re-stamps every component]** Not needed. The interface docs do not mention `warm_stream`.

**Rationale**: the spec describes a fallback that does not exist. A verifier would look for a warm-stream copy on unknown-device lookups and find a synchronous one.

**Confidence**: HIGH · **approved: false**

---

### Proposal A-15: 001-dispatcher-cache-interface/FR-028 vs FR-062 — the slot of a timed-out read is freed while the drive may still write to it

**Direction**: ALIGN · **Fix**: 8 (`408a568d0`). Related to the fix; the hazard was there before it and the fix does not close it.

**Requirement**: FR-062's rule that a buffer a drive may still write to is leaked, never freed.

**Current State**:
- Spec says:
  - FR-028 (`spec.md:358`): on a failed SSD read "the dispatcher MUST release that slot (`IMemoryTier::remove`) before reporting the error".
  - FR-062 (`:422-430`): "the actor's timeout forgets an operation the drive may still be writing, and freeing a buffer then would let a late read land in another entry's bytes … leaves the buffer **leaked**".
- Code does: a timed-out object is reported `Err` (`src/pipeline.rs:1481-1495`), and `batch_lookup` then releases its slot (`src/lib.rs:3068-3071`). The pipeline's own comment covers only the call's lifetime ("the chunk_bufs Arc keeps that memory alive for the whole call", `src/pipeline.rs:1415-1418`). After the release, the slot can be given to another key, and a late NVMe completion for the forgotten operation can then overwrite it. The single-entry readers do the same (`promote_and_serve` `src/lib.rs:1016-1023`). In this case FR-028 and FR-062's reasoning conflict.

**Proposed Resolution**: a code change, plus one sentence in FR-028 on apply.
- Mark a timed-out object distinctly in the result, for example with its own error variant or a flag.
- On that result, `batch_lookup` and `promote_and_serve` do **not** call `mt.remove`. They leave the slot reserved and log it, as FR-062 does for a bounce buffer.
- Add to FR-028: "except a slot whose read timed out, which is leaked (logged), never freed, for the reason given in FR-062".

Test first: a mock drive that reports `Timeout` and then writes the buffer late. The slot must not be reused by a later insert. The test must fail at HEAD `f4579a41f`.

**Rationale**: FR-062 already states the hazard as a design rule. The cold path should follow the same rule.

**Confidence**: MEDIUM · **Severity**: moderate · **approved: false**

---

### Proposal A-16: 001-dispatcher-cache-interface/FR-056 `read_write_stats` — a race while the drive list is handed over

**Direction**: ALIGN · **Fix**: 3 (`d65de1fef`); what the fix leaves open.

**Requirement**: the counters must never decrease between two calls (B-36; the interface says "Monotonic", `components/interfaces/src/idispatcher.rs:940`).

**Current State**:
- Code does: `absorb_drive_rw_stats` takes the `data_drives` read lock and the baseline write lock, then releases both (`src/lib.rs:1831-1837`). The drive list is swapped later, under a separate write lock (`:2171`, `:2441-2444`). A `read_write_stats` call in that window (`:4602-4606`) adds the baseline, which already includes the outgoing drives, to those same drives, which are still listed. It returns double their counters, and the next call reads the correct, lower value.

**Proposed Resolution**: take the `data_drives` write lock first and hold it through both the absorb and the swap: add its drives to the baseline, then replace or clear the list. Readers then see either the state before or the state after. Test: a reader thread that loops on `read_write_stats` while `shutdown` runs must see a sequence that never decreases.

**Confidence**: HIGH · **Severity**: minor (telemetry only, and only during `shutdown` / re-`initialize`) · **approved: false**

---

### Proposal A-17: 002-served-by-tier-attribution/FR-033 — `InvalidState` also means "not in the memory tier"

**Direction**: ALIGN · **Fix**: 4 (`03b05d505`); what the fix leaves open.

**Requirement**: `evictions_blocked_unpersisted` only "when it refuses for want of an `ssd_offset`" (`002-…/spec.md:576`, and B-37).

**Current State**:
- Code does: every `InvalidState` is counted as unpersisted (`src/lib.rs:1225-1234`). But the dispatch map also returns `InvalidState` for an entry that is not in the MemoryTier state (`components/dispatch-map/src/lib.rs:576-578`). That happens for a `batch_lookup` cold-prep slot. The slot is in the memory tier, so `oldest_keys` lists it. Its entry is still BlockDevice and unpinned, because the cold pin is released at classification (`src/lib.rs:2727`, align task A-02). A scan at that moment counts write-through lag that does not exist.

**Proposed Resolution**: two options.
- Give `try_evict_to_block` separate answers for "no `ssd_offset`" and "not in the memory tier". This changes dispatch-map and the **[interfaces]** doc (`components/interfaces/src/idispatch_map.rs:181-187`).
- Without an interface change: have `evict_one_clean` count unpersisted only after confirming the entry is MemoryTier. That costs one extra lookup, on a path that has already failed.

Fixing A-02 (keeping the cold pin) also removes the common case. Test: a cold-prep slot over a BlockDevice entry, scanned by `evict_one_clean`, must increment neither counter.

**Confidence**: MEDIUM · **Severity**: low · **approved: false**

---

## Withdrawn / closed (not proposals)

### B-43 and A-13: withdrawn, because fix 7 was taken out (`209b01de5`)

- **Code today:** `lookup_async`'s memory-tier arm copies synchronously (`serve_memory_tier_to_gpu(…, warm_raw, true)`, `src/lib.rs:3424-3431`). It then releases the read pin (`:3432`), records `promotions_to_gpu` and `lookup_hits_dram` only on `Ok` (`:3434-3443`), and returns the null stream (`:3444`). This is the same as `origin/unstable` (`git show origin/unstable:components/dispatcher/src/lib.rs`, `r.map(|()| null_stream)`), except for fix 9's route counter. The old B-43 (null-stream list) and A-13 (pin released while the copy is still running) therefore describe code that no longer exists. Their regression test `repair_test_lookup_async_stream.rs` was removed with the revert.
- **N-13 (note, kept from A-13):** an asynchronous `lookup_async` (one that returns the stream its copy was queued on) **must hold the entry's read pin until that copy has completed**, not only until it is queued. Otherwise the memory-tier evictor (`src/background.rs:736`, `evict_one_clean` `src/lib.rs:1215`) or `remove` can free and reuse the slot under the DMA. FR-039 step (2) states the same rule for `batch_lookup` ("Only after that sync are the warm read pins released"). It applies to the synchronous `lookup` too, which delegates to `lookup_async` and syncs only after it returns (`src/lib.rs:2477-2488`). Counters for a hit should also be recorded only once the copy is known to have landed. Holding a pin past the return needs an interface decision (a pin guard returned with the stream, or a documented caller-held `pin`), so this belongs to the separate `lookup_async` issue named in `209b01de5`.
- **FR-036 meanwhile:** the text says `lookup_async` "returns a `GpuStream` handle instead of blocking on the H2D DMA transfer". Today it always blocks and always returns null. This is not new: unstable has the same code, and the 2026-10-31 sync counted FR-036 as aligned on the grounds that a null stream plus a completed copy satisfies the caller contract. This run records it as **drifted (carried, deferred to the separate issue)** and proposes no text, as the brief says. Recording the synchronous behaviour now would contradict the intended fix.

### A-14: closed in code (`9a79224a1`)

`register_host_memory` / `unregister_host_memory` no longer call SPDK without an active SPDK environment (`components/gpu-services/src/lib.rs:1176-1181`, `:1218-1225`). The question A-14 asked is answered: it crashed, as the removed comment warned. That was observed on node7 according to the commit, and the case is now guarded. The remaining spec precision is B-48. A-14 was never written into `align-tasks.md`.

## Not proposed (recorded only)

- **Five repair-test modules are compiled into the shipped library.** `#[path = "repair_test_*"] mod …;` has no `#[cfg(test)]` at `src/lib.rs:87` (route_partition), `:8796` (register_host_memory), `:8798` (rw_stats_after_shutdown), `:8800` (transient_map_error) and `:8802` (no_drives_promote_without_read), and none of the five files has an inner `#![cfg(test)]`. Their mocks are therefore compiled into the non-test library. On 2026-11-06 there were six; the sixth, `repair_test_lookup_async_stream`, was removed by the revert. This is source hygiene, not spec drift. It is for the source owner and for the repair acceptance check.
- `DispatcherError::Timeout` doc "exceeded the 100ms timeout" (`components/interfaces/src/idispatcher.rs:178`, mirrored in `contracts/errors.md:21`) is out of date. Fixing it is an interfaces edit, so it is not proposed (see B-40).
- The `IDispatcher::lookup_async` doc (`components/interfaces/src/idispatcher.rs:569-574`, "Returns the CUDA stream the copy was issued on") has the same gap as FR-036. It is deferred with N-13.
- Open align tasks are carried over unchanged. Re-checked at HEAD: A-1 (`schedule_write_through` still enqueues with no pin, `src/lib.rs:4571-4593`), A-02 (`batch_lookup` cold pin released at classification, `:2727`), A-10, A-08 (now has value tests for `Dram`, `Miss`, `Ssd` `repair_test_nvme_timeout_fails_whole_drive.rs:769`, and `Error` `:749` / `repair_test_transient_map_error.rs:780` / `repair_test_sync_failure_error.rs:532`; still none for `Remote` or `SizeMismatch`), A-12. Carried without re-checking: A-2…A-5, A-05, A-06, N-01, RH-03, DISP-RESERVE-REASON. A-04 is closed by B-41.

Previous proposals: `git show HEAD:components/dispatcher/.specify/sync/proposals.md` (the 2026-10-31 run). The 2026-11-06 analysis is in the git stash `j33 spec-sync analysis 2026-11-06 (before repair 7 out)`. The shell copy into `backups/` needed approval in this headless run, so git holds both previous versions.
