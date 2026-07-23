# Sync Apply Report

**Date**: 2026-07-22
**Component**: dispatch-map
**Spec**: 001-dispatch-map
**Mode**: AUTO-BACKFILL
**Based on**: `.specify/sync/drift-report.md` / `drift-report.json` (generated 2026-07-22T21:28:13Z)
**Backups**: `.specify/sync/backups/20260722T232141Z/` (pre-edit copies of `spec.md`, `data-model.md`, `quickstart.md`, `plan.md`, `research.md`, `tasks.md`, `contracts/idispatch_map.md`, `checklists/requirements.md`)

## Actions Taken

### 1. BACKFILL: `promote_block_to_memory_tier` and `try_evict_to_block`

Both methods are real, actively-consumed production `IDispatchMap` API — used by `components/dispatcher` and `components/dispatcher-p2p` for on-demand cold-block promotion (read-miss path) and atomic SSD-evictor demotion — with zero requirements coverage in `spec.md` prior to this pass.

**Files modified**:
- `specs/001-dispatch-map/spec.md`
  - Added **User Story 10** — Promoting a Cold Block-Device Entry Back to Memory Tier (P2), 5 acceptance scenarios.
  - Added **User Story 11** — Atomically Evicting a Memory-Tier Entry to Block Device (P2), 5 acceptance scenarios.
  - Added **FR-025** (`promote_block_to_memory_tier`) and **FR-026** (`try_evict_to_block`).
  - Added 3 new Edge Cases entries covering in-place promotion with active references, `InvalidSize` on promotion, and the atomicity guarantee of `try_evict_to_block`.
  - Added a **2026-07-22 Clarifications** session entry superseding the 2026-04-27 "one-way MemoryTier → BlockDevice" clarification, which `promote_block_to_memory_tier` contradicts (the lifecycle is now bidirectional).
  - Added 2 Assumptions entries documenting caller responsibility for DRAM staging and that dispatch-map itself has no promotion/eviction policy logic.
  - Added a **Last Synced** header line pointing back to this apply pass.
- `specs/001-dispatch-map/contracts/idispatch_map.md` — fully rewritten: removed the stale `Staging`/`DmaBuffer` API (`set_dma_alloc`, `create_staging`) that no longer exists in code; documented the full current method set including `promote_block_to_memory_tier`, `try_evict_to_block`, `is_evictable`, `entry_size`, `recover_extent`, `convert_memory_tier_to_block`; added `RefCountOverflow` to the error enum; added invariants #8-#10 covering atomicity/no-partial-state guarantees for the two backfilled methods.
- `specs/001-dispatch-map/data-model.md` — fully rewritten: removed the stale `Staging`/RDTSC-`tsc` model; documented current `Location`/`DispatchEntry`/`DispatchMapState`/`LookupResult`/`DispatchMapError` shapes matching `entry.rs`/`state.rs`/`idispatch_map.rs`; updated the state-machine diagram to show the now-bidirectional `MemoryTier ⇄ BlockDevice` transitions; added a note on the `reuse_count` dead field and the missing null-pointer error variant (both tracked in `align-tasks.md`).
- `specs/001-dispatch-map/quickstart.md` — fully rewritten: replaced the `set_dma_alloc`/`create_staging` example with a `create_memory_tier_entry`/`convert_to_storage`/`try_evict_to_block`/`promote_block_to_memory_tier` walkthrough matching the current API and the mandatory `IEvictionPolicy` receptacle; added a "Typical Promotion Flow" section.

**Verification**: New spec text cross-checked against `components/interfaces/src/idispatch_map.rs:234-262` (trait signatures/doc comments) and `components/dispatch-map/src/lib.rs:451-545` (implementations) for exact error conditions, atomicity, and reference-preservation semantics.

### 2. NEW_SPEC: Not applicable

No genuinely separate feature was identified — both unspecced methods extend the existing `001-dispatch-map` lifecycle (`Location` transitions) and were backfilled into the existing spec rather than split into a new increment.

### 3. SUPERSEDE: Not applicable

No prior spec increment needed a supersede banner; `001-dispatch-map` is the only spec for this component (per `drift-report.json: conflicts: []`).

### 4. ALIGN / DEFECT / AMBIGUOUS: Deferred to `align-tasks.md`

Seven tasks generated — see `.specify/sync/align-tasks.md` for full detail (severity, spec requirement, current code, required change, files, acceptance criteria):

| # | Task | Severity | Disposition |
|---|------|----------|-------------|
| 1 | FR-014 missing `logger.error(...)` calls | Moderate | DEFECT — spec correct, code incomplete |
| 2 | `create_memory_tier_entry` missing null-pointer rejection (US1/AS3) | Moderate | DEFECT — spec correct, code incomplete |
| 3 | Stale Creusot `verif/` comment in `idispatch_map.rs` | Minor | DEFECT — misleading comment, non-existent directory |
| 4 | `reuse_count` dead metric | Minor | AMBIGUOUS — human decision (expose vs. remove) |
| 5 | `lookup` size-mismatch path unreachable (US2/AS4) | Minor | AMBIGUOUS — spec-acknowledged pre-existing gap, no action taken |
| 6 | SC-004 wording imprecise about Rust enum layout | Minor | AMBIGUOUS — spec wording, not code; left unchanged pending human sign-off (outside this pass's explicit BACKFILL authorization) |
| 7 | `README.md`/`CLAUDE.md` stale relative to code and updated specs | Minor | Documentation drift outside this pass's edit scope (not under `specs/**` or `.specify/sync/**`) |

None of these were applied to code or to README.md/CLAUDE.md by this pass — per the AUTO-BACKFILL hard rule, only Markdown under `specs/**` and `.specify/sync/**` was edited.

## Not Applied (Pending Human Decision)

- `reuse_count` disposition (task #4 above).
- SC-004 rewording (task #6 above) — flagged but not edited, since the explicit authorization for this pass covered FR-014 and the null-pointer gap as defects, not SC-004.
- `lookup` size-mismatch path (task #5 above) — no product decision to close this gap yet.

## Post-Apply State

| Item | Status |
|------|--------|
| FR-025 (`promote_block_to_memory_tier`) | Added — spec now covers implemented, consumed code |
| FR-026 (`try_evict_to_block`) | Added — spec now covers implemented, consumed code |
| User Story 10 / 11 | Added, 5 acceptance scenarios each |
| Companion docs (`contracts/`, `data-model.md`, `quickstart.md`) | Refreshed to match current code and updated spec |
| FR-014 (error logging) | Unchanged in spec (correct); code defect tracked in `align-tasks.md` |
| US1/AS3 (null pointer) | Unchanged in spec (correct); code defect tracked in `align-tasks.md` |
| SC-004 (struct size wording) | Unchanged; flagged as ambiguous, deferred |
| `reuse_count`, stale `verif/` comment, doc staleness (README/CLAUDE.md), test coverage gap | Deferred — see `align-tasks.md` |

## Next Steps

1. Review the backfilled User Story 10/11, FR-025/FR-026 in `specs/001-dispatch-map/spec.md`.
2. Work through `.specify/sync/align-tasks.md`: prioritize the two DEFECT tasks (FR-014 logging, null-pointer rejection) since they represent spec-documented behavior the code does not yet provide.
3. Get a human decision on `reuse_count` and the SC-004 wording tweak.
4. Run `components/dispatch-map:component-update-docs` to refresh `README.md`/`CLAUDE.md`.
5. Commit: `git add components/dispatch-map/specs/ components/dispatch-map/.specify/sync/ && git commit -m "spec-sync: backfill promote/evict FRs, refresh companion docs for dispatch-map"`

---

## Prior Apply Pass (2026-05-21) — historical record

**Actions Taken**:

1. BACKFILL: Component naming (`DispatchMapComponentV0` → `DispatchMapComponent`) — updated `data-model.md`, `quickstart.md`, `CLAUDE.md`.
2. BACKFILL: FR-018 `MemoryTier` field types (`pointer: u64, size: usize` → `pointer: *mut u8, size: u32`) — updated `spec.md` FR-018 and Key Entities.
3. NO-OP: SC-004 — no change made at the time ("varies by variant" judged permissive enough).
4. BACKFILL: FR-004 `LookupResult::BlockDevice` returns only `offset` (no `size`) — updated `spec.md` FR-004 and Key Entities.

All four items were marked aligned as of that pass. This 2026-07-22 pass's drift-report re-analysis found SC-004 drifted again (the "varies by variant" wording is inaccurate for a Rust enum) — see task #6 above.
