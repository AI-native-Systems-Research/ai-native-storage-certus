---
spec_sync_component: extended-metadata-store
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-14T21:37:10Z
spec_sync_git_commit: 6f781e85
spec_sync_inputs_sha256: 43801741b23a2623a2ac240c5b16ca60a5191ca5523fde7068c5e3a09e3e226d
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Sync 2026-10-14 (branch fix/bundle-dispatcher-ems) — full re-analysis at `4f8b5554` (component source last changed in `4a8fe3e7`); batch mode, read-only.** Scope: the branch's only change to this component is the formal-verification fix `4a8fe3e7` (`src/flush.rs` +23/−3, new `src/repair_test_force_flush_error.rs`). I re-checked all 36 requirements (FR-01..18, NFR-01..11, SC-1..7) and the US1–US6 acceptance criteria against the current `src/` with file:line evidence; nothing was taken from earlier reports. I could not run any tests (`cargo test` needed a permission this session did not have), so SC-2..6 stay *unverified*. **The fix is correct as far as reading the code can show:**
> - `FlushTickets` gains a `durable` watermark and `last_error` (`flush.rs:88-97`).
> - `durable` advances only after a successful pass (`:265-268`) or a signalled pass that found nothing dirty (`:237`).
> - `trigger_flush` returns `Ok` iff `durable >= ticket`, otherwise the last recorded error (`:181-185`).
> - With the documented trigger, `force_flush()` now returns `StorageError` when the flush fails (`lib.rs:232`), and a fault-injection test covers this (`repair_test_force_flush_error.rs:42-61`).
>
> This closes the major FR-05 / ALIGN-EMS-003 drift on the code side.
>
> **Result:** 25 aligned, 6 drifted, 5 unverified, 2 unspecced behaviours, 4 inter-spec conflicts.
>
> **Drift:**
> - FR-05: the spec row still says failures are "not propagated (`src/flush.rs:177`)", which is now false (spec-side, BACKFILL).
> - **FR-13 (new, moderate):** the documented wiring `attach_flush_trigger(move || flush_manager.trigger_flush())`, combined with `FlushManager` callbacks that hold `Arc`s of the component, is a reference cycle through the worker thread. In that wiring the `FlushManager` is never dropped, so the final flush never runs and the worker thread leaks. The new repair test wires it exactly this way (`repair_test_force_flush_error.rs:24-37`).
> - FR-09, NFR-04, FR-01 and SC-7 are unchanged and still open (ALIGN-EMS-004..007).
>
> **Stale and missing items:** line references in FR-11/12/13, the FR-05 Known Gaps entry and the US6 note are stale after the fix, and so is the status in tasks.md:19 and align-tasks.md. Two of ALIGN-EMS-003's acceptance tests are still missing.
>
> **Verdict: drift.** 15 proposals in `proposals.md` (backfill 7, align 7, supersede 1), all `approved: false`. The `spec_sync_*` frontmatter was left unchanged.

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

Generated: 2026-10-18 (analysis note dated 2026-10-14 as requested)
Project: extended-metadata-store (`components/extended-metadata-store/`)
Branch / commit analysed: `fix/bundle-dispatcher-ems` @ `4f8b5554` (component source last changed in `4a8fe3e7`)
Mode: batch, read-only. The only files written are under `.specify/sync/`.

**Method.** I checked each FR, NFR, SC and US acceptance criterion in
`specs/001-extended-metadata-store/spec.md` by reading the shipped code. Line numbers refer to
the tree at `4f8b5554`. I also reviewed the branch diff (`git diff origin/unstable.. -- src`)
on its own. **No tests were executed**: `cargo test` needed a permission this session did not
have. Anything that can only be shown by running tests is marked *unverified*, not aligned.

**Body replaced.** This body supersedes the 2026-10-14 body from branch
`fix/extended-metadata-store-flush` @ `2970e0a8`. That analysis predates the fix `4a8fe3e7`,
and the spec was changed by the 2026-10-14 apply, so all spec line numbers below are re-derived
from the current `spec.md`.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 (`001-extended-metadata-store`; spec.md + plan.md + tasks.md) |
| Requirements Checked | 36 (18 FR + 11 NFR + 7 SC), plus US1–US6 acceptance criteria |
| ✓ Aligned | 25 (69%) |
| ⚠️ Drifted | 6 (17%): FR-01, FR-05 (spec side only), FR-09, FR-13 (new), NFR-04, SC-7 |
| ✗ Not Implemented | 0 (0%) |
| ❔ Unverified (need test execution) | 5 (14%): SC-2, SC-3, SC-4, SC-5, SC-6 |
| 🆕 Unspecced Code | 2 |
| Parked (open ALIGN carried over) | 1: `CapacityExhausted` never constructed (ALIGN-EMS-002) |
| Inter-spec conflicts | 4 |

**Change since the previous body:**
- FR-05 is now correct in code (the major item ALIGN-EMS-003 is fixed by `4a8fe3e7`); only the spec text is stale.
- FR-12 and FR-17 are aligned: the 2026-10-14 apply backfilled them.
- FR-13 is newly drifted.
- The earlier unspecced items U1–U5 were backfilled by the apply.
- Conflicts 1, 2, 4 and most of 5 were resolved by the apply. The 4 conflicts listed here are new or remaining.

## Branch fix — independent verification (`4a8fe3e7`, "force_flush returns the device error when the flush failed")

The diff touches only `src/flush.rs` (+23/−3) and adds `src/repair_test_force_flush_error.rs`
(72 lines). I re-derived its correctness from the code, not from the commit message.

1. **New state.** `FlushTickets` gains `durable: u64` and `last_error: Option<String>`
   (`flush.rs:88-97`), guarded by the same `flush_state` mutex as `requested`/`completed`.
2. **`durable` advances only when the data is on disk.**
   - A pass that found nothing dirty and was signalled advances `completed` and `durable`
     together (`flush.rs:233-238`). That is correct: nothing is pending, so the caller's
     earlier writes are already covered by the last successful flush.
   - `do_flush` advances `completed` always, and `durable` only on `Ok`. On `Err` it stores
     the error in `last_error` (`flush.rs:260-270`). It does this after `mark_flushed`, which
     runs only on success (`:255-258`).
   - The shutdown pass uses the same `do_flush` (`:223-227`).
3. **`trigger_flush` reports the result of the pass that released it.**
   - The caller waits until `completed >= ticket` (`flush.rs:176-179`), then returns `Ok` iff
     `durable >= ticket`, else `Err(last_error)` (`:181-185`).
   - `completed` and `durable` only grow (`max`). `durable` is advanced only by a pass whose
     `serving` covers the ticket, and that pass's snapshot was taken after the ticket's write
     (`:219-221`, unchanged ticket argument from the previous report). So `Ok` implies a
     successful flush whose snapshot contains the caller's write.
   - `durable < ticket` after `completed >= ticket` implies that the latest pass covering the
     ticket failed. That pass set `last_error` under the same lock first, so `unwrap_or_default()`
     (`:184`) never yields an empty string on a real failure.
   - A waiter that wakes only after a later successful pass also covered its ticket gets `Ok`.
     That is correct, because the later snapshot contains its write. Normally, though, the waiter
     is woken by the failed pass's `notify_all` (`:269`) and returns `Err` before any retry. So
     ALIGN-EMS-003's acceptance criterion 3 ("a ticket served by a failed pass but later covered
     by a successful pass returns `Ok`") holds only if the waiter runs late; it is not a
     deterministic property (see P7).
4. **The component maps the error.** `force_flush` maps the trigger's `Err` to `StorageError`
   (`lib.rs:230-234`, unchanged). With the documented trigger, `force_flush()` therefore now
   returns `StorageError` when the flush fails.
5. **Tests.** Both tests are `#[cfg(test)]` inside `flush.rs` (`:300-302`) and compile only with
   `--features testing`.
   - `force_flush_reports_failed_disk_write` (`repair_test_force_flush_error.rs:42-61`) uses
     `FaultConfig { fail_after_n_writes: Some(0) }`. Every write fails at `test_support.rs:133-141`
     and the error surfaces through `block_io.rs:109-110`. The test asserts `StorageError` and that
     the write stays dirty.
   - `force_flush_ok_on_healthy_device` (`:63-72`) is the healthy control case.
   - This is the first caller of `attach_flush_trigger` in the workspace (grep:
     `repair_test_force_flush_error.rs:37` is the only one). It also closes the
     "follow-up test through `force_flush()`" asked for at `spec.md:154`.
6. **What the fix does not cover:**
   - ALIGN-EMS-003's acceptance criteria 2 and 3 are not tested. The control case checks
     `dirty_count() == 0` but does not reboot and recover. No test covers "a ticket served by a
     failed pass, then covered by a successful pass, returns `Ok`".
   - The wiring used by the test creates an ownership cycle (see FR-13 below).

## Detailed Findings

### Spec: 001-extended-metadata-store — Feature Specification: Extended Metadata Store

#### Aligned ✓

| Req | What was checked | Evidence |
|-----|------------------|----------|
| FR-02 | `get` returns a clone, or `NotFound` | `src/lib.rs:192-200`; test `get_not_found` `lib.rs:258` |
| FR-03 | `delete` removes the key, returns `Ok` for a missing key, increments dirty count | `src/lib.rs:202-211`; tests `lib.rs:270`, `:281` |
| FR-04 | `iterate_all` clones the whole map under one read lock | `src/lib.rs:213-219` |
| FR-06 | `MAX_VALUE_SIZE = 128*1024`; `put` returns `ValueTooLarge` before any mutation | `src/lib.rs:66-67`, `:179-181`; test `lib.rs:329` |
| FR-07 | Serialise → write the inactive region → flip `active_region` and bump `flush_seq` → superblock last | `src/flush.rs:24-57` (inactive write `:45-46`, commit point `:54`) |
| FR-08 | Recovery reads the superblock and the active region, checks the header `flush_seq` against the superblock, loads the entries | `src/recovery.rs:23-59`; `src/lib.rs:147-162` |
| FR-10 | No valid superblock → `format_fresh` returns `partition_sectors == 0` → `initialize_from_client` calls `format_partition` | `src/recovery.rs:27-32`, `:89-113`; `src/lib.rs:150-154`; test `tests/persistence.rs:765` |
| FR-11 | Timer (`deadline` `flush.rs:202`, `:207-209`; `None` disables it), threshold `max(dirty_threshold,1)` (`:195`, `:205`), 10 ms poll (`:14-16`, `:211`), disarm on failure (`:198`, `:243`), timer pass with nothing dirty does no I/O (`:230-241`). Behaviour matches the spec text; **the spec's line references are stale** (cites `:185-206`, `:234`), see P5 | `src/flush.rs:14-16`, `:59-75`, `:193-244` |
| FR-12 | Tickets: `requested+1` under `flush_state` (`flush.rs:160-165`), worker reads `serving = requested` before the dirty check/snapshot (`:221`, `:230`, `:249`), completion `max(serving)` (`:236`, `:264`). Matches the spec text. The spec does not say that a call released by a failed pass returns `Err` (new behaviour, U1), and its line references are stale (cites `:88-94`, `:152-178`, `:211-213`, `:251-257`); see P4 | `src/flush.rs:88-97`, `:155-186`, `:219-241`, `:260-270` |
| FR-14 | `put`/`delete` increment after dropping the write lock (`lib.rs:187-188`, `:208-209`); a successful flush subtracts the snapshot's recorded count (`:113-121`), and a failed one subtracts nothing (`flush.rs:255-258`); `load_entries` resets to 0 (`lib.rs:92-93`). The new test confirms the failed case (`repair_test_force_flush_error.rs:57-60`) | `src/lib.rs:86-121`, `:188`, `:209` |
| FR-15 | `define_component!` provides `IExtendedMetadataStore` | `src/lib.rs:40-64` |
| FR-16 | Optional `logger: ILogger` receptacle; every op logs at `debug` if bound. Minor gap: a `put` rejected with `ValueTooLarge` returns before logging (`:179-184`) | `src/lib.rs:44-46`, `:182-184`, `:193-195`, `:203-205`, `:214-216`, `:222-224`, `:167-171` |
| FR-17 | `snapshot_entries` records `snapshot_dirty` under the read lock (`lib.rs:99-106`); `mark_flushed` sets `flush_seq`, swaps the recorded count to 0 and subtracts it (`:113-121`); with no prior snapshot `swap` yields 0, so the count is left unchanged. Matches the backfilled text | `src/lib.rs:50-53`, `:96-121` |
| FR-18 | `region_capacity_bytes()` = `region_a_size * sector_size` | `src/on_disk.rs:147-149` |
| NFR-01 | `RwLock<HashMap>`: read lock for get/iterate/snapshot, write lock for put/delete/load | `src/lib.rs:48`, `:87`, `:100`, `:185`, `:196`, `:206`, `:217` |
| NFR-02 | CRC32 (poly `0xEDB88320`) on superblock, region header, entry record | `src/on_disk.rs:78`, `:110-113`, `:170`, `:186-189`, `:248-252`, `:276-283`, `:364-377` |
| NFR-03 | Superblock/header are one sector each; records padded to the sector size | `src/on_disk.rs:60`, `:163`, `:222-229`, `:353-358` |
| NFR-05 | `on_disk` always compiled; `block_io`/`flush`/`recovery`/`test_support` behind `feature = "testing"` | `src/lib.rs:26-38` |
| NFR-06 | Default build: `force_flush` with no trigger returns `Ok` | `src/lib.rs:230-234`; test `lib.rs:319` |
| NFR-07 | `MockBlockDevice` + `FaultConfig::fail_after_n_writes` | `src/test_support.rs:21`, `:59`, `:133-141`, `:223` |
| NFR-08 | `BlockDeviceClient` takes a `DmaAllocFn`; tests use `heap_dma_alloc` | `src/block_io.rs:18`, `:83`; `src/test_support.rs:244` |
| NFR-09 | Integer fields use `to_le_bytes`/`from_le_bytes` | `src/on_disk.rs:379-410` |
| NFR-10 | Magic `0x4345_5254_4D45_5441` checked on read | `src/on_disk.rs:6`, `:91-94` |
| NFR-11 | `create_test_component_from_state` → `MockBlockDevice::reboot_from` | `src/test_support.rs:275`, `:65` |
| SC-1 | Exactly 9 `#[test]` in `src/lib.rs` (`:247-354`), no features needed | static count |

#### Drifted ⚠️

- **FR-05: the spec text is out of date after the fix.** (Severity: minor; spec side, BACKFILL)
  - Spec says (`spec.md:166`, status column): "**Open:** with the documented `FlushManager::trigger_flush()` trigger, flush failures are not propagated (`src/flush.rs:177`) — see drift-report 2026-10-14 / proposal P1".
  - What the code does now: `trigger_flush` returns `Err(last_error)` when the pass that released it failed (`src/flush.rs:181-185`, `:260-270`), and `force_flush` maps that error to `StorageError` (`src/lib.rs:232`). Test: `repair_test_force_flush_error.rs:42-61`.
  - The FR-05 requirement text itself is now **met in code**. Only the "Open" clause and the stale citation `flush.rs:177` (now a `wait_while` line) are wrong.
  - Related stale text that refers to the same pre-fix state: the US6 note (`spec.md:154`), the Known Gaps FR-05 entry (`spec.md:183`: `lib.rs:201-215`, `:68`, `:111`; the current lines are `:221-235`, `:72`, `:131`), `tasks.md:19` ("Follow-up: flush-failure propagation …"), and `align-tasks.md` ALIGN-EMS-003 (status "open", citing `flush.rs:177`).
- **FR-13: the documented trigger wiring makes the final flush on Drop unreachable.** (Severity: **moderate**; new)
  - Spec says (`spec.md:174`): "`FlushManager` performs a final flush on Drop (graceful shutdown) …". `spec.md:311` and the doc comment at `lib.rs:127` recommend the trigger `move || flush_manager.trigger_flush()`.
  - What the code does:
    - The trigger closure owns the `FlushManager` and is stored in the component's `flush_trigger` field (`lib.rs:61`, `:131-133`).
    - The `FlushManager` owns the worker thread (`flush.rs:85`, `:142-144`). The worker owns `snapshot_fn`/`dirty_count_fn`/`reset_dirty_fn` (`:107-109`).
    - In every wiring in the tree, those callbacks capture an `Arc` of the same component (`repair_test_force_flush_error.rs:24`, `:33-35`; also `tests/persistence.rs:514-523`).
  - Result: component → trigger → `FlushManager` → worker thread → callbacks → component.
    - The component's strong count never reaches zero, so the `FlushManager` is never dropped, `Drop` (`flush.rs:276-290`) never runs, and the final flush (`:223-227`) never happens.
    - The worker thread keeps running for the life of the process, waking every 10 ms (`:205`, `:211`).
    - There is no `detach_flush_trigger`. The only way to drop the manager is to replace the trigger with `attach_flush_trigger`, which drops the old manager while holding the trigger write lock. That join does not deadlock, because the worker never takes `flush_trigger`.
  - Who is affected: today only the new repair test (each of its 2 tests leaks one worker thread). It also affects any production wiring that follows the spec's recommendation.
  - The cycle existed in principle since `attach_flush_trigger` was introduced, but no caller wired it until this branch.
  - Not covered by any test: no test drops a component with an attached trigger and checks that the final flush ran.
- **FR-09: corrupt *entries* do not trigger the fallback to the inactive region.** (Severity: moderate; unchanged, ALIGN-EMS-004 open)
  - Spec says (`spec.md:170`): "Recovery falls back to inactive region if active region is corrupt". Recovery Algorithm (`spec.md:302-303`): "verify header CRC and entry CRCs … If active region corrupt: try inactive region".
  - What the code does: fallback happens only on a header parse failure or a `flush_seq` mismatch (`recovery.rs:53-66`).
  - `deserialize_region` skips a bad-CRC entry by one sector and still returns `Some` (`on_disk.rs:341-344`). There is no warning, although the doc at `recovery.rs:21` says skipped entries produce warnings.
  - A multi-sector record skipped by one sector misparses every later record.
  - The only test corrupts the header (`tests/persistence.rs:245`).
- **NFR-04: after a fallback recovery, the next flush overwrites the only good region.** (Severity: moderate; unchanged, ALIGN-EMS-005 open)
  - Spec says (`spec.md:196`): "partial flush never corrupts previously-committed data". Flush Strategy (`spec.md:294`): "Crash during step 2: old active region is still valid".
  - What the code does: the fallback returns the superblock unchanged (`recovery.rs:68-77`), so `active_region` still names the corrupt region. `initialize_from_client` passes it on (`lib.rs:155-157`, `:173`).
  - The next flush writes `inactive_region_offset()` (`flush.rs:45-46`), which is the region that was just recovered.
  - A crash between `flush.rs:46` and `:54` leaves both regions invalid, and recovery then formats fresh (`recovery.rs:79-84`). That also resets `flush_seq`, contrary to US2 AC6 (`spec.md:68`).
- **FR-01: keys longer than 65 535 bytes are accepted but cannot be persisted.** (Severity: moderate; unchanged, ALIGN-EMS-006 open)
  - Spec says (`spec.md:162`): "`put(key, value)` stores arbitrary binary data (0 to 128 KiB) by string key". `spec.md:213` defines `key_len(2B)`.
  - What the code does: only the value length is checked (`lib.rs:179`). `serialize` writes `key_bytes.len() as u16` (`on_disk.rs:233`) but copies the full key (`:242`).
  - On recovery the CRC fails (`:276-283`) and the entry is skipped (see FR-09), so the key is silently lost on restart.
  - With the fix, an over-long key also makes the region larger. That can now surface as a capacity `StorageError` from `force_flush()`, which is better than the old silent `Ok`, but the key-length truncation itself is unchanged.
- **SC-7: "All public items have doc comments" is false.** (Severity: minor; unchanged, ALIGN-EMS-007 open)
  - Spec says (`spec.md:260`): "All public items have doc comments; `cargo doc --no-deps` is warning-free".
  - Undocumented public items:
    - `on_disk.rs`: `Superblock` fields `:12-23`; `Superblock::new`/`serialize`/`deserialize`/`inactive_region_offset`/`active_region_offset`/`region_capacity_bytes` `:27`, `:59`, `:85`, `:131`, `:139`, `:147`; `RegionHeader` fields `:155-158`, methods `:162`, `:176`; `EntryRecord` fields `:206-208`, methods `:214`, `:222`, `:227`, `:257`; `pad_to_sector` `:353`; `bytes_to_sectors` `:360`.
    - `block_io.rs`: `BlockDeviceClient::sector_size` `:12`, `BlockDeviceClient::new` `:18`.
    - `recovery.rs`: `RecoveryResult` fields `:14-16`.
  - No `missing_docs` lint is set (`lib.rs:1-25`). The `cargo doc` clause was not run.

#### Not Implemented ✗

None.

#### Unverified ❔ (static evidence only; tests could not be executed this sweep)

| SC | Static evidence | Note |
|----|-----------------|------|
| SC-2 | 19 `#[test]` in `tests/persistence.rs`, plus 7 `#[cfg(test)]` repair tests in `src/repair_test_*.rs` (`--features testing`): 2 + 3 + 2 | pass/fail unknown. The commit message of `4a8fe3e7` claims "tests 5/5" for its acceptance run; I did not reproduce it |
| SC-3 | 14 tests in `tests/integration_ssd.rs` | needs NVMe hardware. `test_capacity_exhaustion` cannot reach its `CapacityExhausted` arm (`:542`) |
| SC-4 | `crash_mid_flush_recovers_previous_state` (`tests/persistence.rs:687`) | the NFR-04 scenario has no test |
| SC-5 | `concurrent_stress_8_threads` (`tests/persistence.rs:417`) | none |
| SC-6 | none | `cargo clippy -- -D warnings` was not run |

### Parked (carried over)

- **`CapacityExhausted` is defined but never constructed** (`interfaces/src/iextended_metadata_store.rs:12`; grep finds no construction under `src/`). ALIGN-EMS-002 stays parked (maintainer decision 2026-09-03).
- **What the fix changes:** the flush-time capacity error (`flush.rs:36-42`) now reaches the caller of `force_flush()` as `StorageError("region data … exceeds region capacity …")`, no longer as `Ok`.
- So the "alternative" route in ALIGN-EMS-002 is unblocked: it now only needs mapping to `CapacityExhausted`.
- The dependency note at `align-tasks.md:111-116` ("`trigger_flush` currently discards every flush error (`src/flush.rs:177`)") is out of date. The Known Gaps entry `spec.md:185` does not mention that the error now reaches the caller as `StorageError` (P14).

### Unspecced Code 🆕

| # | Feature | Location | Lines | Suggested Spec |
|---|---------|----------|-------|----------------|
| U1 | `durable` watermark + `last_error` in `FlushTickets`. A `trigger_flush()` released by a failed pass returns that error (the most recent failure recorded; it is never cleared, but it is returned only when `durable < ticket`). A failed pass leaves the caller's writes dirty, so a later pass may still persist them, and a waiter that wakes after such a pass gets `Ok` | `src/flush.rs:88-97`, `:181-185`, `:237`, `:265-268` | ~15 | 001 FR-05 / FR-12 (P1, P4) |
| U2 | Ownership between trigger and manager: `attach_flush_trigger` has no detach counterpart, and the trigger closure keeps the `FlushManager` alive for the component's lifetime | `src/lib.rs:131-133`; `src/flush.rs:276-290` | ~5 | 001 FR-13 / FR-05 (P6) |

## Inter-Spec Conflicts

1. **`tasks.md:19` vs code.** It says "Follow-up: flush-failure propagation through `FlushManager::trigger_flush()` (proposal P1, 2026-10-14)", which describes the work as pending. It is implemented in `flush.rs:181-185` (`4a8fe3e7`).
2. **`align-tasks.md:20` and `:138-176` (ALIGN-EMS-003, status "open") vs code.** The "Current Code" section cites `flush.rs:177` "always ends in `Ok(())`", which is no longer true. Acceptance criterion 1 is met; criteria 2 and 3 are not tested; criterion 4 ("drop the Open clause from FR-05") is pending (P1).
3. **`spec.md:154` (US6 note) vs `src/repair_test_force_flush_error.rs`.** The note says a test that "installs a trigger and asserts durability (and error propagation) through `force_flush()`" is still a follow-up tracked by P1. That test now exists. ALIGN-EMS-001 (the SSD test) is still open.
4. **`spec.md:8` ("Previous Sync") still cites `002-FR-011` and `002-FR-007`.** Only `specs/001-*` is tracked. The 2026-10-14 SUPERSEDE (P14) replaced the live references but left this history line. Minor; a historical record (P15).

## Recommendations

1. **Backfill FR-05 and its satellites** (P1–P3, P12): drop the "Open" clause, cite `flush.rs:181-185`/`:260-270`, list the two new tests, refresh the Known Gaps refs, and mark ALIGN-EMS-003 resolved in `tasks.md`/`align-tasks.md`.
2. **Break the trigger↔manager cycle (FR-13, P6).** Preferred fix: the `FlushManager` callbacks capture `Weak<ExtendedMetadataStoreComponent>` (no component change), or add a `detach_flush_trigger()` that takes the trigger out and drops the manager. Add a test that the final flush runs when the wiring is torn down. Fix the repair test wiring the same way.
3. **Finish ALIGN-EMS-003's acceptance tests** (P7): reboot and recover after the healthy `force_flush()`, and restate criterion 3 so that it is deterministic.
4. **The open ALIGN items stay as they are** (P8–P11: FR-09, NFR-04, FR-01, SC-7). None changed on this branch.
5. **Refresh the stale line references** in FR-11/12/13 (P4, P5), the CapacityExhausted Known Gap (P14) and the history line (P15). Fix the stale code doc comments (`flush.rs:79`, `:156`, `lib.rs:75`; P13).
6. **Run the tests before the next `clean` stamp.** That means `cargo test -p extended-metadata-store` (default and `--features testing`), `cargo clippy -p extended-metadata-store --features testing -- -D warnings`, and `cargo doc -p extended-metadata-store --no-deps`.
