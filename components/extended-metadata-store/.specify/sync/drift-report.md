---
spec_sync_component: extended-metadata-store
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-14T21:37:10Z
spec_sync_git_commit: 6f781e85
spec_sync_inputs_sha256: 43801741b23a2623a2ac240c5b16ca60a5191ca5523fde7068c5e3a09e3e226d
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Re-stamp 2026-10-14 (transitive, merge of unstable 6f5743cc into this branch).** Only `components/interfaces` changed for this component (`DispatcherConfig::cold_staging_buf_bytes` default 4 -> 16 MiB, #550); `extended-metadata-store` does not use it (grep over its src/ and specs/: 0 hits). This branch's full sync below stands; no re-analysis was needed.

> **Re-stamp 2026-10-14 (transitive: `components/interfaces` changed, branch `fix/cold-staging-buf-16mib`).** The spec-sync digest folds `components/interfaces/{src,specs}` into every component's hash, so the single interface change on this branch — `DispatcherConfig::cold_staging_buf_bytes` default 4 MiB → 16 MiB (commit `8e12e49f`) and the matching correction to interfaces FR-027 — invalidates every component's stamp at once; this is the design, not a defect. **No code re-analysis was performed for this component, and none was needed:** its own `src/**` and `specs/**` are unchanged on this branch, and it does not reference `cold_staging_buf_bytes` (consumed only by the dispatcher's host-DRAM cold-staging pool). Digest refreshed to the recomputed value, computed over the **committed** `specs/**` tree (as the CI gate's clean checkout sees it): this working tree carries uncommitted, untracked spec files under `specs/001-*` and `specs/002-*` that are a separate local work-in-progress, out of scope for this branch, and deliberately excluded so the stamp matches what CI recomputes.

> **Apply 2026-10-14 (branch fix/extended-metadata-store-flush, approved by Cornel).** All 14 proposals in `proposals.json` were approved and handled; backups are in `.specify/sync/backups/20261014T181138Z/`, and details are in `apply-report.{md,json}`. **Applied to the spec (no code change):** P7 (FR-12, US6 AC2/AC5: ticket coalescing), P8 (FR-11: timer + threshold details), P9 (FR-14, FR-17: snapshot-scoped `mark_flushed`, single-flusher precondition), P10 (FR-13: unconditional final flush; `FlushManager` entity: `completed_seq()`), P11 (FR-05: evidence refreshed to `src/lib.rs:221-235`/`:131`/`:72`, trigger read lock, an explicit **Open** clause pointing at P1), P12 (US6 tests: T048 caveat, 5 repair tests), P13 (plan.md :25/:32/:96/:101/:104/:162; tasks.md T056, ALIGN-001/002 marked resolved), and P14 (SUPERSEDE: the untracked 002 spec references are replaced by ALIGN-EMS-001/002 and P1, with line refs refreshed). These clear the unspecced behaviours U1–U5 and inter-spec conflicts 1–5. **Became ALIGN tasks** (approval note: "approved as an ALIGN task only: record in align-tasks.md, no code change in this PR"): P1 → ALIGN-EMS-003 (FR-05, `trigger_flush` reports a failed flush as `Ok`, major), P2 → ALIGN-EMS-004 (FR-09), P3 → ALIGN-EMS-005 (NFR-04), P4 → ALIGN-EMS-006 (FR-01; error variant still to be decided), P5 → ALIGN-EMS-007 (SC-7), and P6 refreshed ALIGN-EMS-002 (still parked; its `force_flush` alternative depends on ALIGN-EMS-003). **Still open:** no unapproved proposals remain. The code-side drift for FR-05, FR-09, NFR-04, FR-01 and SC-7 is unchanged until those tasks are implemented, along with ALIGN-EMS-001 and the parked ALIGN-EMS-002, so the verdict remains **drift**. The `spec_sync_*` frontmatter was not touched by this apply.
>
> **Stamp: clean.** Every drift item of this analysis is resolved (spec updates applied with Cornel's approval) or tracked in `align-tasks.md` (ALIGN tasks and open items), as in earlier `clean` stamps of this and other components (e.g. dispatcher-p2p).

> **Sync 2026-10-14 (branch fix/extended-metadata-store-flush, full re-analysis at `2970e0a8`; batch mode, read-only).** I re-checked all 36 requirements (FR-01..18, NFR-01..11, SC-1..7) and the US1–US6 acceptance criteria against the current `src/` with file:line evidence; nothing was taken from earlier reports. I could not run any tests: `cargo test` needs a permission this session did not have, so every verdict below rests on reading the code and tests. **The branch fix holds up.** I checked it by hand: (a) a `trigger_flush` ticket is released only by a pass that reads `requested` (`flush.rs:213`) before it snapshots (`:240`); (b) `snapshot_entries` reads `dirty_count` while holding the read lock, and put/delete increment it only after dropping the write lock (`lib.rs:99-105`, `:185-188`, `:206-209`), so the recorded count never covers a mutation missing from the snapshot. A count can only be too small, which costs a spare flush. (c) The threshold trigger is real now (`flush.rs:185-206`). Result: 24 aligned, 7 drifted, 5 unverified (SC-2/3/4/5/6 can only be shown by running tests), 5 unspecced behaviours. **Drift:** FR-05 — `trigger_flush` returns `Ok` even when the flush fails (`flush.rs:177`, `:246-257`), so `force_flush` reports durability it did not achieve (major, ALIGN). This was open in the 10-13 note too. FR-09 — a corrupt *entry* is skipped silently and recovery does not fall back to the other region (`on_disk.rs:341-344`, `recovery.rs:53`) (new, ALIGN). NFR-04 — after a fallback recovery the next flush overwrites the only good region (`recovery.rs:73-77`, `flush.rs:45-46`) (new, ALIGN). FR-01 — `put` accepts keys longer than `u16::MAX` bytes that cannot be persisted (`on_disk.rs:233`) (new, ALIGN). SC-7 — many public items have no doc comment (new, ALIGN). FR-12, FR-17 — the spec text is out of date after the fix (BACKFILL). plan.md:32/:101/:104/:162 and tasks.md:13/:18/:19 contradict spec.md (conflicts). `CapacityExhausted` stays parked. Verdict: drift. 14 proposals in `proposals.md`, all `approved: false`.

> **Sync 2026-10-13 (branch fix/extended-metadata-store-flush, fix: ticketed `trigger_flush` + snapshot-scoped `mark_flushed` + real dirty-threshold trigger).** Scope: `src/flush.rs`, `src/lib.rs`, two `repair_test_*.rs`. Findings: RESOLVED — FR-11 threshold was previously ignored (only the timer fired; this report wrongly listed it aligned), now `flush.rs:186-206`; FR-14 / US6 durability no longer lose writes made mid-flush (`lib.rs:99-121`, `flush.rs:171-175,213`). OPEN — (1) `trigger_flush` returns `Ok(())` after a failed flush (`flush.rs:177`; `do_flush` completes tickets regardless of result, `:251-257`), so an attached trigger never yields `StorageError` and `force_flush` claims durability falsely, contradicting `spec.md:163` and `interfaces/src/iextended_metadata_store.rs:44` (pre-existing, unreported). (2) Unspecced: `mark_flushed` now clears only the count recorded by the last `snapshot_entries` (no-op without one), while `plan.md:101` says "reset dirty_count" and FR-17 (`spec.md:175`) is silent; threshold disarm after failure (`flush.rs:188-190,234`) undocumented; `plan.md:32`/`flush.rs:79` "share a single in-flight flush" now inexact. Verdict: drift.

> **Re-stamp 2026-10-09 (transitive: `components/interfaces` changed, branch `fix/evict-blocked-by-pin`).** The only change to this component's hashed inputs is a doc-comment correction on `IDispatchMap::try_evict_to_block` in `components/interfaces/src/idispatch_map.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and its previous stamp was re-verified to match `origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

> **Re-stamp 2026-10-05 (transitive: `components/interfaces` changed, branch `fix/poller-cpu-placement-logging`).** The only change to this component's hashed inputs is a doc-comment correction on `DispatcherConfig::poller_base_cpu` in `components/interfaces/src/idispatcher.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and it does not reference `poller_base_cpu` (verified by grep over its src/ and specs/). Its previous stamp was re-verified to match `git archive origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

> **Re-stamp 2026-10-01 (transitive: `components/interfaces` changed).** The spec-sync digest
> folds `components/interfaces/{src,specs}` into **every** component's hash, so an interface
> change invalidates all of them at once -- which is the design, not a defect. The interface
> change is the `served_by` attribution work: new `ServedBy` and `LookupOutcome` types, and
> `IDispatcher::batch_lookup` widened to return `Vec<LookupOutcome>`.
>
> **No re-analysis was performed for this component, and the digest bump asserts only what was
> actually checked**: the interface change is additive except for `batch_lookup`'s return type,
> which the compiler enforces across all implementors, and this component's own `src/**` and
> `specs/**` are unchanged. Components that implement `IDispatcher` (`dispatcher`,
> `dispatcher-p2p`) had their specs updated substantively; this one did not need it. If a later
> sweep finds drift here, this stamp is not evidence against it.

> **Re-stamp 2026-09-15 (workspace `cargo fmt` sweep; no drift).** Commit `f9bcd965` ("Add shmq RESERVE batch shared-deadline regression test") ran `cargo fmt` across the whole workspace, reflowing this component's `src/*.rs` (multi-line ↔ single-line argument lists and struct literals, import reordering). `git diff -w` confirms no token-level logic, signature, or contract change — the only substantive addition in that commit is a regression test in `lib/shmq-dispatcher/src/translate.rs`, which is outside this component and outside the spec-sync gate's `components/` scope. The formatting moved this component's `spec_sync_inputs_sha256`, but its spec↔implementation alignment is unchanged. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

> **Re-stamp 2026-09-15 (interfaces-fold; no drift).** Branch `fix-reserve-batch-deadline` (`bec6c6ec`) added a `deadline: Option<std::time::Instant>` parameter to `IDispatcher::reserve_memory` in `components/interfaces/src/idispatcher.rs` (shared batch backpressure deadline for the shm-queue OP_RESERVE handler). `scripts/spec-sync-hash.sh` folds the whole `components/interfaces` tree into every component's hash, so this component's digest moved even though its own spec/implementation did not change. This component does not describe or call `reserve_memory`; the interface delta cannot affect its spec↔implementation alignment. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

> **Re-stamp 2026-09-09 (merged-branch interfaces-fold; no drift).** Branch
> `fix-dispatcher-store-backpressure` was merged into `unstable` at `3411518a`; that
> branch adds `DispatcherConfig::store_backpressure_ms` plus two `TierEventStats`
> counters (`store_backpressure_events`, `store_drops_on_full`) to
> `components/interfaces/src/idispatcher.rs`. `scripts/spec-sync-hash.sh` folds
> the whole `components/interfaces/` tree into every component's hash, so this
> component's digest moved even though its own `src/`+`specs/` are byte-for-byte
> unchanged and it references none of those new dispatcher symbols (verified by
> grep across `components/` and `lib/`). The merge's conflict resolution had
> reverted this stamp to unstable's `b220a1c8` value; the digest is recomputed
> here at merged HEAD. Drift status remains `clean`; the report body stands
> verbatim.
>
> **Correction 2026-09-09T23:14Z (clean-tree digest; branch linearized).** The
> 22:27 re-stamp above was computed in a working tree that carried untracked,
> uncommitted files under this component's `specs/` — the speckit scratch docs
> (`data-model.md`, `research.md`, `quickstart.md`, `contracts/`, `checklists/`)
> and an entire local `002-ssd-integration-test/` spec — none of which are
> tracked on this branch. `scripts/spec-sync-hash.sh` walks files on disk, so
> those strays inflated the local digest and CI's clean checkout recomputed a
> different one (same failure class as the 2026-09-03 `iipc.rs` note below). The
> digest is now recomputed in a pristine `git worktree` at the branch tip over
> the **tracked** inputs only (`specs/001-extended-metadata-store/{spec,plan,tasks}.md`
> + `components/interfaces`), giving `6b39fa69…`, which is exactly what CI hashes.
> The branch was also rebased onto `origin/unstable` (merge commit removed), so
> `spec_sync_git_commit` now points at the linear tip `8f6a5b40`. No `src/`,
> `specs/`, or report-body content changed; drift status remains `clean`.

> **Digest refreshed 2026-09-03 (spec-sync gate fix).** The earlier
> `spec_sync_inputs_sha256` was computed in a working tree that contained an
> untracked `components/interfaces/src/iipc.rs` — a local file not part of this
> branch — which `scripts/spec-sync-hash.sh` folds into every component's
> interface hash, so CI's clean checkout recomputed a different digest. The
> stray file was removed and the digest recomputed in a clean tree. No `src/`,
> `specs/`, or report content changed; drift status remains `clean`.

# Spec Drift Report

Generated: 2026-10-14
Project: extended-metadata-store (`components/extended-metadata-store/`)
Branch / commit analysed: `fix/extended-metadata-store-flush` @ `2970e0a8` (source last changed in `95529247`)
Mode: batch, read-only. The only files written are under `.specify/sync/`.

**Method.** Each FR, NFR, SC and US acceptance criterion in
`specs/001-extended-metadata-store/spec.md` was checked against the shipped code by reading it.
Line numbers refer to the tree at `2970e0a8`. I also reviewed the branch diff
(`git diff origin/unstable.. -- src`) on its own. **No tests were executed**: `cargo test`
needs a permission this session did not have. Anything that can only be shown by running
tests is marked *unverified*, not aligned.

**Body replaced.** The previous body (dated 2026-09-03, stamped `clean`) is superseded. It
listed FR-11 as aligned while the threshold trigger was unimplemented. It also missed the
FR-09, NFR-04, FR-01 and SC-7 findings below, which are all present on `origin/unstable` too.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 (`001-extended-metadata-store`; spec.md + plan.md + tasks.md) |
| Requirements Checked | 36 (18 FR + 11 NFR + 7 SC), plus US1–US6 acceptance criteria |
| ✓ Aligned | 24 (67%) |
| ⚠️ Drifted | 7 (19%) — FR-01, FR-05, FR-09, FR-12, FR-17, NFR-04, SC-7 |
| ✗ Not Implemented | 0 (0%) |
| ❔ Unverified (need test execution) | 5 (14%) — SC-2, SC-3, SC-4, SC-5, SC-6 |
| 🆕 Unspecced Code | 5 |
| Parked (open ALIGN carried over) | 1 — `CapacityExhausted` never constructed |
| Inter-spec conflicts | 5 |

## Branch fix — independent verification

The fix (`a7c10168`, `95529247`) touches `src/flush.rs`, `src/lib.rs` and two new test modules.
I re-derived its correctness from the code rather than from the commit messages:

1. **A `trigger_flush` caller is never released by a flush that predates its write.**
   - The caller's write happens before it takes ticket `requested+1` under `flush_state`
     (`flush.rs:156-161`).
   - The worker reads `serving = requested` under the same mutex (`flush.rs:213`), and only
     then checks the dirty count (`:222`) and snapshots (`:240`).
   - Tickets ≤ `serving` are marked complete only after that pass (`:252-257`), or on the
     `dirty == 0` path (`:223-231`).
   - A ticket taken after `:213` waits for a later pass (`:172-175`).
   - No wakeup is lost: the caller sets `signaled` after taking its ticket (`:164-169`). If
     that lands after the worker clears it (`:208`), the next loop iteration sees it.
2. **Writes made during a flush stay dirty.**
   - `snapshot_entries` records `dirty_count` while holding the read lock (`lib.rs:99-105`).
   - `put`/`delete` increment the count only after dropping the write lock
     (`lib.rs:185-188`, `:206-209`).
   - So any increment the recorded count includes belongs to a mutation that is in the
     snapshot. This follows from RwLock mutual exclusion plus read–write coherence, and
     holds even with `Relaxed` ordering.
   - `mark_flushed` subtracts exactly the recorded count with `fetch_update` and
     `saturating_sub` (`lib.rs:113-121`), so increments that land at the same time are kept.
   - The recorded count can be too small (a put has inserted but not yet incremented). That
     is safe: the mutation stays counted and costs one spare flush.
3. **The threshold trigger is real.**
   - The worker's wait loop exits on `signaled`, on the deadline, or when
     `threshold_armed && dirty_count() >= max(threshold, 1)` (`flush.rs:187`, `:197-206`).
   - It polls every `THRESHOLD_POLL_INTERVAL = 10 ms` (`:14-16`), because writes do not
     notify the condvar.
4. **What the fix does not cover:** a failed flush still completes the tickets, and
   `trigger_flush` returns `Ok(())` anyway (`flush.rs:177`, `:246-257`). See FR-05 below.

Test coverage added by the branch:
- `repair_test_flush_lost_dirty.rs:30,90` — tests for points 1 and 2.
- `repair_test_flush_threshold.rs:62,75,91` — tests for point 3.

Both modules are `#[cfg(test)]` inside `flush.rs`, which only compiles with `--features testing`
(`lib.rs:31-32`). None of the five tests is listed in spec.md.

## Detailed Findings

### Spec: 001-extended-metadata-store — Feature Specification: Extended Metadata Store

#### Aligned ✓

| Req | What was checked | Evidence |
|-----|------------------|----------|
| FR-02 | `get` returns a clone, or `NotFound` | `src/lib.rs:192-200`; test `get_not_found` `lib.rs:258` |
| FR-03 | `delete` removes the key, returns `Ok` for a missing key, increments dirty count | `src/lib.rs:202-211`; tests `lib.rs:270,281` |
| FR-04 | `iterate_all` clones the whole map under one read lock (consistent snapshot) | `src/lib.rs:213-219` |
| FR-06 | `MAX_VALUE_SIZE = 128*1024`; `put` returns `ValueTooLarge` before any mutation | `src/lib.rs:66-67`, `:179-181`; test `lib.rs:329` |
| FR-07 | Serialise → write the inactive region → flip `active_region` and bump `flush_seq` → write the superblock last | `src/flush.rs:24-57` (inactive write `:45-46`, commit point `:54`) |
| FR-08 | Recovery reads the superblock and the active region (checking that the header `flush_seq` matches the superblock), then loads the entries | `src/recovery.rs:23-59`; `src/lib.rs:147-162` |
| FR-10 | No valid superblock → `format_fresh` returns `partition_sectors == 0` → `initialize_from_client` calls `format_partition` | `src/recovery.rs:27-32`, `:89-113`; `src/lib.rs:150-154`; test `recovery_fresh_partition_formats_empty` `tests/persistence.rs:765` |
| FR-11 | Both triggers work: the timer (`deadline` `flush.rs:194`, `:199-202`) and the dirty threshold (`:187`, `:197`). Defaults are 5 s and 100 (`:68-74`). See the unspecced items U2 for details the spec does not mention | `src/flush.rs:14-16`, `:59-75`, `:185-206`, `:234` |
| FR-13 | `Drop` sets `shutdown`, signals the worker and joins it; the worker runs a final `do_flush` | `src/flush.rs:263-277`, `:215-219` (see U4: this final flush runs even when nothing is dirty) |
| FR-14 | `put`/`delete` increment the count (`lib.rs:188`, `:209`). A successful flush subtracts only the count recorded with its snapshot (`:113-121`); `load_entries` resets to 0 (`:92-93`). "Mutations since last flush" holds in the sense of "not yet covered by a successful flush"; the more precise wording is proposed in P9 | `src/lib.rs:76-121`; tests `lib.rs:342`, `repair_test_flush_lost_dirty.rs:90` |
| FR-15 | `define_component!` provides `IExtendedMetadataStore` | `src/lib.rs:40-64` |
| FR-16 | Optional `logger: ILogger` receptacle; each operation logs at `debug` if a logger is bound | `src/lib.rs:44-46`, `:182-184`, `:193-195`, `:203-205`, `:214-216`, `:222-224`, `:167-171`. One gap: a `put` rejected with `ValueTooLarge` returns before logging (`:179-184`), which makes US1 AC6 "all operations are logged" slightly inexact |
| FR-18 | `Superblock::region_capacity_bytes()` returns `region_a_size * sector_size` | `src/on_disk.rs:147-149` (spec cites `:142`, which is stale) |
| NFR-01 | `RwLock<HashMap>`: `get`/`iterate_all`/`snapshot_entries` take the read lock, `put`/`delete`/`load_entries` take the write lock | `src/lib.rs:48`, `:87`, `:100`, `:185`, `:196`, `:206`, `:217` |
| NFR-02 | CRC32 (poly `0xEDB88320`) on the superblock, region header and each entry record | `src/on_disk.rs:78`, `:110-113`, `:170`, `:186-189`, `:248-252`, `:276-283`, `:364-377` |
| NFR-03 | Superblock and header are each one sector buffer; records are padded to the sector size | `src/on_disk.rs:60`, `:163`, `:222-229`, `:353-358` |
| NFR-05 | `on_disk` is always compiled; `block_io`/`flush`/`recovery`/`test_support` need `feature = "testing"`; `testing = ["interfaces/spdk"]`, `spdk = ["testing", …]` | `src/lib.rs:26-38`; `Cargo.toml:8-10` |
| NFR-06 | The default build has no SPDK dependency: `force_flush` with no trigger returns `Ok` | `src/lib.rs:230-234`; test `lib.rs:319` |
| NFR-07 | `MockBlockDevice` with `FaultConfig::fail_after_n_writes` | `src/test_support.rs:19-21`, `:59`, `:133`, `:169`, `:223` |
| NFR-08 | `BlockDeviceClient` takes a `DmaAllocFn`; tests use the heap-backed `heap_dma_alloc` | `src/block_io.rs:5`, `:11`; `src/test_support.rs:244` |
| NFR-09 | All integer fields are written and read with `to_le_bytes`/`from_le_bytes` | `src/on_disk.rs:379-410` |
| NFR-10 | Magic number `0x4345_5254_4D45_5441` is checked on read | `src/on_disk.rs:6`, `:91-94` |
| NFR-11 | `create_test_component_from_state` → `MockBlockDevice::reboot_from` | `src/test_support.rs:275-284` (spec cites `:272`, which is stale) |
| SC-1 | Exactly 9 `#[test]` in `src/lib.rs` (`:247-354`) with no features; another 6 in `on_disk.rs` (`:416-479`) | static count |

#### Drifted ⚠️

- **FR-05 — `force_flush()` durability on the attached `FlushManager` path.** (Severity: **major**)
  - Spec says (`spec.md:163`): "`force_flush()` ensures all mutations are durable on disk: it invokes a durable-flush trigger … and blocks until it returns, mapping trigger errors to `StorageError`". `spec.md:308` and the interface doc (`interfaces/src/iextended_metadata_store.rs:44`, "Returns when all data is durable") say the same.
  - The component side is correct: `force_flush` calls the trigger and maps `Err` to `StorageError` (`src/lib.rs:221-235`).
  - The problem is the trigger itself. The documented trigger is `move || flush_manager.trigger_flush()` (`lib.rs:127`, `spec.md:180`), and `FlushManager::trigger_flush` **always** returns `Ok(())` (`src/flush.rs:177`). `do_flush` marks the served tickets complete whether or not `flush_to_disk` succeeded (`:246-257`).
  - Consequence: if the device fails or the region capacity is exceeded (`flush.rs:36-42`), `force_flush()` returns `Ok`, the data is not durable, and `StorageError` is never produced on this path. No test covers it: `attach_flush_trigger` is not called by any test or caller in the workspace (grep).
  - The spec's line references are also stale: it cites `lib.rs:201-215`, `:111`, `:68`; the current lines are `221-235`, `131`, `72`.
  - Location: `src/flush.rs:152-178`, `:239-260`.
- **FR-09 — corrupt *entries* do not trigger the fallback to the inactive region.** (Severity: **moderate**)
  - Spec says (`spec.md:167`): "Recovery falls back to inactive region if active region is corrupt". The Recovery Algorithm (`spec.md:299-300`) says "Read active region, verify header CRC and entry CRCs … If active region corrupt: try inactive region".
  - What the code does: fallback happens only when the region **header** fails to parse or its `flush_seq` does not match (`recovery.rs:53-66`).
  - When an entry's CRC fails, `deserialize_region` skips one sector and carries on (`on_disk.rs:341-344`) and still returns `Some`. Recovery therefore accepts a partial entry set from the active region, with no warning (although `recovery.rs:21` says warnings include "skipped corrupt entries") and no fallback.
  - It gets worse for large values. A multi-sector record (for example a 128 KiB value, 33 sectors) is skipped by only one sector. The following sectors are then parsed as records, so later valid entries in the region can also be lost.
  - The only test, `recovery_fallback_to_inactive_region`, corrupts the header (`tests/persistence.rs:268-277`), so this path is never exercised.
- **NFR-04 — after a fallback recovery, a crash during the next flush destroys the only good copy.** (Severity: **moderate**)
  - Spec says (`spec.md:193`): "partial flush never corrupts previously-committed data". It also says (`spec.md:290-292`): "Crash during step 2: old active region is still valid".
  - What the code does: on fallback, `recover_from_disk` returns the superblock **unchanged** (`recovery.rs:68-77`). `active_region` still points at the corrupt region, and the entries were loaded from the *inactive* one.
  - That superblock is returned by `initialize_from_client` (`lib.rs:155-157`, `:173`) and fed to `flush_to_disk`/`FlushManager::start`.
  - The next flush writes `inactive_region_offset()` (`flush.rs:45-46`), which is the region that holds the only valid checkpoint.
  - A crash between `flush.rs:46` and `:54` leaves both regions invalid. The next recovery then formats the partition fresh (`recovery.rs:79-84`), losing all data.
  - That fresh format also resets `flush_seq` to 0 (`recovery.rs:106-112`), contrary to US2 AC6 "Flush sequence numbers are monotonically increasing".
- **FR-01 — keys longer than 65 535 bytes are accepted but cannot be persisted.** (Severity: **moderate**)
  - Spec says (`spec.md:159`): "`put(key, value)` stores arbitrary binary data (0 to 128 KiB) by string key". It sets no key limit, and `tasks.md:7` lists "key size limits" as a missing requirement.
  - What the code does: `put` checks only the value length (`lib.rs:179`).
  - `EntryRecord::serialize` writes `key_len` as `key_bytes.len() as u16` (`on_disk.rs:233`), which silently truncates, but copies the full key (`:242`).
  - On recovery the CRC therefore fails (`:276-283`) and the entry is skipped (see FR-09). Through the skip-one-sector behaviour, the entries after it can be lost too.
  - The key is held in memory and acknowledged, and is silently lost on restart.
- **FR-12 / US6 AC2 — coalescing wording is out of date after the fix.** (Severity: minor)
  - Spec says (`spec.md:170`): "Multiple `force_flush()` calls coalesce into single flush operation". `spec.md:140` and `plan.md:32` ("share a single in-flight flush") say the same.
  - What the code does: coalescing works by ticket. All calls registered before a pass reads `requested` (`flush.rs:213`) are released by that pass. A call registered while a pass is in flight is deliberately **not** released by it and waits for the next pass (`:171-175`).
  - So concurrent calls can take two passes, not one. The doc comment `flush.rs:79` still says "share a single in-flight flush".
  - US6 AC5 "returns immediately (no-op)" is also inexact. With nothing dirty the call still waits for one worker wake-up (`flush.rs:223-231`), although no I/O happens. `flush.rs:153` says "Returns immediately" too.
- **FR-17 — the `mark_flushed` contract changed and the change is not specified.** (Severity: minor)
  - Spec says (`spec.md:175`): the wiring API is listed with no stated semantics. `plan.md:101` says `mark_flushed(new_seq) [reset dirty_count]`.
  - What the code does: `snapshot_entries` now records the dirty count it covers (`lib.rs:103-104`). `mark_flushed` subtracts exactly that recorded count and clears it (`lib.rs:113-121`).
  - Undocumented consequences:
    - (a) `mark_flushed` with no snapshot before it no longer resets the dirty count; it leaves it unchanged.
    - (b) A second `snapshot_entries()` between a flusher's snapshot and its `mark_flushed` overwrites the single recorded slot (`lib.rs:53`). That can clear mutations that were never persisted, so only one flusher may drive the pair at a time.
    - (c) A failed flush leaves the recorded count stale until the next snapshot. This is harmless, because it is overwritten.
  - The existing manual callers already follow the snapshot → flush → `mark_flushed` order (`tests/integration_ssd.rs:206-209`).
- **SC-7 — "All public items have doc comments" is false.** (Severity: minor)
  - Spec says (`spec.md:257`): "All public items have doc comments; `cargo doc --no-deps` is warning-free".
  - Undocumented public items:
    - `on_disk.rs`: all `Superblock` fields (`:12-23`), `Superblock::new`/`serialize`/`deserialize`/`inactive_region_offset`/`active_region_offset`/`region_capacity_bytes` (`:27`, `:59`, `:85`, `:131`, `:139`, `:147`), `RegionHeader` fields and methods (`:155-158`, `:162`, `:176`), `EntryRecord` fields and `new`/`serialized_size`/`serialize`/`deserialize` (`:206-208`, `:214`, `:222`, `:227`, `:257`), `pad_to_sector`, `bytes_to_sectors` (`:353`, `:360`).
    - `block_io.rs`: `BlockDeviceClient::sector_size` and `BlockDeviceClient::new` (`:12`, `:18`).
    - `recovery.rs`: `RecoveryResult` fields (`:14-16`).
  - The workspace has no `missing_docs` lint, so `cargo doc` can be warning-free while SC-7's first clause fails. The second clause was not run this sweep.

#### Not Implemented ✗

None.

#### Unverified ❔ (static evidence only; tests could not be executed this sweep)

| SC | Static evidence | Note |
|----|-----------------|------|
| SC-2 | 19 `#[test]` in `tests/persistence.rs`, plus 5 new `#[cfg(test)]` tests in `src/repair_test_*.rs` (`--features testing`) | the pass/fail state is unknown |
| SC-3 | 14 tests in `tests/integration_ssd.rs` | needs NVMe hardware. `test_capacity_exhaustion` (`:526`) cannot reach its `CapacityExhausted` arm (`:542`), so it proves nothing (parked item) |
| SC-4 | `crash_mid_flush_recovers_previous_state` (`tests/persistence.rs:687`) | covers a crash with a healthy inactive region only. The NFR-04 scenario above has no test |
| SC-5 | `concurrent_stress_8_threads` (`tests/persistence.rs:417`) | — |
| SC-6 | — | `cargo clippy -- -D warnings` was not run |

### Parked (carried over, unchanged)

- **`CapacityExhausted` is defined but never constructed.**
  - The variant is defined at `interfaces/src/iextended_metadata_store.rs:12` and no `src/` path constructs it (grep).
  - Capacity is surfaced only as a `String` at flush time (`flush.rs:36-42`). On the `FlushManager` path even that `String` is now discarded (see FR-05).
  - `tests/integration_ssd.rs:526-547` (arm `:542`) proves nothing. The earlier report cited `:513-547`/`:530`, which are stale.
  - Tracked as ALIGN-EMS-002 in `.specify/sync/align-tasks.md`. The maintainer decision of 2026-09-03 to keep it parked stands.

### Unspecced Code 🆕

| # | Feature | Location | Lines | Suggested Spec |
|---|---------|----------|-------|----------------|
| U1 | Ticket-based `trigger_flush` protocol (`FlushTickets { requested, completed }`); a pass serves the tickets taken before its snapshot | `src/flush.rs:88-94`, `:152-178`, `:211-213`, `:223-231`, `:251-257` | ~45 | 001 FR-12 (P7) |
| U2 | Threshold trigger details: 10 ms polling (`THRESHOLD_POLL_INTERVAL`), `dirty_threshold` clamped to ≥1, threshold **disarmed after a failed flush** until a later pass succeeds, timer measured from the start of each wait (an interval that cannot be represented disables the timer) | `src/flush.rs:14-16`, `:185-206`, `:234` | ~30 | 001 FR-11 (P8) |
| U3 | `snapshot_dirty` field; `snapshot_entries` → `mark_flushed` pairing and the single-flusher precondition | `src/lib.rs:50-53`, `:93`, `:96-121` | ~30 | 001 FR-14 / FR-17 (P9) |
| U4 | The final flush on shutdown runs **even when nothing is dirty** (it writes a region and bumps `flush_seq`), and its failure is not reported; public `FlushManager::completed_seq()` accessor | `src/flush.rs:215-219`, `:180-183` | ~10 | 001 FR-13 + Key Entities (P10) |
| U5 | `force_flush` holds the `flush_trigger` read lock while the trigger runs, so `attach_flush_trigger` blocks until any in-flight `force_flush` returns | `src/lib.rs:230-234`, `:131-133` | ~6 | 001 FR-05 (P11) |

## Inter-Spec Conflicts

1. **`plan.md:104` contradicts `spec.md:163`/`:308`.**
   - `plan.md:104` says `force_flush()` "is currently *not* wired into this path in any build configuration — it is an unconditional no-op".
   - spec.md (FR-05) says it is implemented with the trigger, and the code agrees (`lib.rs:221-235`).
2. **`plan.md:162` contradicts `spec.md:259`.**
   - `plan.md:162` says the crate "currently is **not**" a workspace member.
   - The root `Cargo.toml:23` (member) and `:125` (workspace dependency) show it is. spec.md:259 also cites `:105`, which is stale.
3. **`tasks.md:13`, `:18`, `:19` contradict `spec.md:180`, `:259` and `align-tasks.md`.**
   - tasks.md still has unchecked items "T056 … blocked on ALIGN-001", "ALIGN-001 … missing from workspace" and "ALIGN-002 … unconditional no-op".
   - spec.md and `align-tasks.md:9-19` record both items as RESOLVED.
4. **`plan.md:32` and `plan.md:101` contradict the code.**
   - `plan.md:32` says "share a single in-flight flush"; `plan.md:101` says "reset dirty_count".
   - They describe the pre-fix behaviour (see FR-12, FR-17).
5. **spec.md cites a `002-ssd-integration-test` spec that is not tracked in this tree.**
   - The citations are at `spec.md:7` ("002-FR-011", "002-FR-007"), `:151` ("002-FR-007's ALIGN task") and `:182` ("the 002 SSD test", "002-FR-007").
   - Only `specs/001-extended-metadata-store/` is tracked. The 002 text survives only in `.specify/sync/backups/`.

## Recommendations

1. **Fix FR-05 first (major).**
   - The fix: `do_flush` records the pass result, and `trigger_flush` returns `Err` when the pass that served its ticket failed.
   - Add a test that attaches a `FlushManager` trigger to the component, injects a write fault (`FaultConfig::fail_after_n_writes`), and asserts that `force_flush()` returns `StorageError`.
   - That test also closes the "follow-up test through `force_flush()`" left open at `spec.md:151`. (P1)
2. **Make recovery fall back when entries are corrupt (FR-09).**
   - Treat entry CRC failures, and a parsed entry count lower than `header.entry_count`, as region corruption.
   - Record a warning, and skip a corrupt record by its declared padded length rather than by one sector. (P2)
3. **After a fallback recovery, make the recovered region the active one (NFR-04).**
   - Either return a superblock with `active_region` pointing at the region actually loaded, or make the next flush target the corrupt region.
   - Add a test: corrupt the active region, recover, inject a crash mid-flush, recover again. (P3)
4. **Reject keys that cannot be persisted (FR-01).** Keys longer than `u16::MAX` bytes should be rejected at `put` time. This needs a decision on the error variant (an interface change). (P4)
5. **Add the missing doc comments (SC-7)**, or narrow SC-7. CLAUDE.md requires doc comments on public APIs, so adding them is preferred. (P5)
6. **Backfill the stale wording.** Update FR-11/12/13/14/17, the FR-05 text and the US6 test list to the shipped behaviour, and fix the conflicting plan.md/tasks.md notes and the 002 citations. (P7–P14)
7. **Run the tests before the next stamp.** `cargo test -p extended-metadata-store` (default and `--features testing`), `cargo clippy -p extended-metadata-store --features testing -- -D warnings`, and `cargo doc -p extended-metadata-store --no-deps` should run before this report is next stamped `clean`; the SC-2/3/4/5/6 verdicts above are unverified.
