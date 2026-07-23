# Sync Apply Report: extent-manager

**Date**: 2026-05-21
**Spec**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`
**Drift report**: `components/extent-manager/.specify/sync/drift-report.json`

## Summary

| Metric | Count |
|--------|-------|
| Proposals applied | 4 |
| Proposals deferred | 0 |
| Files modified | 1 |

## Applied Changes

### 1. BACKFILL-FR-001: ExtentManagerV2 -> ExtentManager

**File**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`

Replaced all 3 occurrences of `ExtentManagerV2` with `ExtentManager`:
- FR-001 requirement text (line 259)
- User Story 1 independent test description (line 25)
- Assumptions section DMA allocator paragraph (line 506)

**Result**: FR-001 now reads "The component MUST be named ExtentManager" matching the actual `define_component!` invocation in code.

### 2. BACKFILL-FORMAT-VERSION: Version 4 -> 5

**File**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`

Updated format version references:
- Key Entities / Superblock: magic `0x4345_5254_5553_5634` ("CERTUSV4") -> `0x4345_5254_5553_5635` ("CERTUSV5"), version 4 -> 5
- On-Disk Format / Superblock table row 0: magic value and label updated
- On-Disk Format / Superblock table row 1: version (4) -> version (5)

**Result**: Spec on-disk format documentation now matches `FORMAT_VERSION = 5` as defined in `src/superblock.rs`.

### 3. SOFTEN-SC-005: Scalability claim language

**File**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`

Softened SC-005 wording from "The component supports approximately 100 million extents" to "The component is designed to support approximately 100 million extents ... (architectural target scale, not yet verified by benchmark)". This accurately reflects that the claim is an architectural goal based on data structure analysis, not a tested guarantee.

**Result**: SC-005 no longer implies verified performance at 100M extent scale.

### 4. BACKFILL-UNSPECCED-FEATURES: Add FR-031 through FR-034

**File**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`

Added four new functional requirements for features present in the implementation but previously unspecified:

- **FR-031** (`used_bytes()`): IExtentManager method returning total bytes currently allocated.
- **FR-032** (`capacity_bytes()`): IExtentManager method returning total data device capacity in bytes.
- **FR-033** (WriteHandle location): WriteHandle type defined in the interfaces crate for decoupled dependency.
- **FR-034** (Fault injection): Test-only feature gate for simulating I/O failures in crash-safety tests.

**Result**: All previously unspecced features now have formal requirements in the spec.

## Verification

All deferred items from the previous apply report have been resolved:
- SC-005 scalability claim: softened to "designed to support" (applied as item 3)
- Unspecced features: added as FR-031 through FR-034 (applied as item 4)

The spec is now internally consistent with the codebase for all known items.

---

# Sync Apply Report: extent-manager — Cycle 2

**Date**: 2026-07-22
**Mode**: AUTO-BACKFILL
**Spec**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`
**Drift report**: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T22:37:58Z)

## Summary

| Metric | Count |
|--------|-------|
| Requirements checked | 41 (37 aligned, 4 drifted, 0 not-implemented) |
| Unspecced features backfilled (new FRs) | 3 |
| Internal spec conflicts fixed | 1 |
| Drift items fixed via spec wording | 1 (FR-016) |
| Drift items deferred (code defect, not spec-editable) | 1 (FR-030, Severity: Major) |
| Drift items deferred (README.md doc drift, out of edit scope) | 2 (Severity: Low each) |
| New specs created | 0 |
| Specs superseded | 0 |
| Files modified | 2 (`spec.md`, `.specify/sync/align-tasks.md`) |

## Decisions

- **No new spec directory.** All three unspecced features (`metadata_region_size`
  shared-device mode, partition-relative base-LBA addressing, post-checkpoint hook) are
  small extensions/configuration knobs on the existing `IExtentManager`/`ExtentManager`
  surface already covered by `001-extent-manager-v2` — none has an independent user story,
  acceptance scenario, or success criterion distinct from the existing spec's Init/Format,
  Instance & Configuration, and Persistence & Recovery sections. Backfilled as new FRs into
  the existing spec instead.
- **`volatile_write_cache` cfg-polarity inversion (FR-030) is NOT resolved by editing the
  spec.** The spec's FR-030 describes the *intended* design (default = durable/flushing,
  opt-in feature = risky/no-flush for performance); the code implements the opposite
  polarity, and by default never flushes the metadata device after checkpoint writes. Per
  instruction, this is a correctness defect, not a documentation nit, so FR-030 was left
  unchanged and the discrepancy was recorded in `align-tasks.md` with **Severity: Major**
  for a maintainer to resolve via a code fix (or an explicit, sign-off-backed spec rewrite).
- **SC-006 vs FR-015 internal conflict resolved by wording fix**, not by picking one side as
  "code-authoritative" and deleting the other — both requirements are compatible once SC-006
  is reworded to describe "at most two sequential checkpoint executions to drain a burst of
  callers" rather than "two active/concurrent operations."
- **README.md drift not touched.** Two drifted items (stale "five minutes" checkpoint
  interval claim, stale "format version (5)" claim) live in `components/extent-manager/README.md`,
  which is outside this apply pass's edit scope (`specs/**` and `.specify/sync/**` only).
  Both recorded in `align-tasks.md` as Severity: Low ALIGN items; FR-016 in `spec.md` was
  additionally updated to disclose the second (README) stale-doc location, since that edit
  is within scope and doesn't touch README.md itself.

## Actions

1. Backed up `spec.md`, `plan.md`, `tasks.md` → `.specify/sync/backups/20260722-162021/`.
2. **BACKFILL** — Added **FR-035** to `spec.md` (Initialization & Format, after FR-004):
   `FormatParams.metadata_region_size` and the shared metadata/data-device co-location mode.
3. **BACKFILL** — Added **FR-036** to `spec.md` (Instance & Configuration, after FR-030):
   `set_metadata_base_lba` / `set_data_base_lba` / `data_base_lba` partition-relative
   addressing, used by `dispatcher` and `dispatcher-p2p`.
4. **BACKFILL** — Added **FR-037** to `spec.md` (Persistence & Recovery, after FR-018):
   `set_post_checkpoint_hook` callback mechanism.
5. **BACKFILL** — Updated the `FormatParams` Key Entity bullet to list the new
   `metadata_region_size` field.
6. **ALIGN (fix wording)** — Reworded **SC-006** to resolve its internal conflict with
   FR-015: now describes "at most two sequential checkpoint executions," explicitly
   consistent with FR-015's single-in-flight-writer semantics, instead of "two active
   [concurrent] operations."
7. **ALIGN (fix wording)** — Extended **FR-016**'s disclosure note to cover the second,
   previously-undisclosed stale "five minutes" doc string in `README.md`, in addition to the
   `IExtentManager` trait doc comment already disclosed by the prior apply cycle.
8. **DEFER (DEFECT, Severity: Major)** — Appended an align-task for FR-030's cfg-polarity
   inversion. Left FR-030's spec text unchanged per instruction (do not flip spec text to
   match a buggy default); flagged for a maintainer decision between a code fix (preferred)
   or an explicit spec rewrite with consumer sign-off.
9. **DEFER (ALIGN, Severity: Low)** — Appended an align-task for `README.md`'s stale "five
   minutes" checkpoint-interval claim (and the analogous stale doc comment in
   `components/interfaces/src/iextent_manager.rs`), out of this pass's edit scope.
10. **DEFER (ALIGN, Severity: Low)** — Appended an align-task for `README.md`'s stale
    "format version (5)" claim (spec and code already correctly say version 6), out of this
    pass's edit scope.

## Result

| Item | Direction | Status |
|------|-----------|--------|
| `metadata_region_size` / shared-device mode | BACKFILL → FR-035 + Key Entity | Applied |
| Base-LBA partition addressing (`set_metadata_base_lba`/`set_data_base_lba`/`data_base_lba`) | BACKFILL → FR-036 | Applied |
| `set_post_checkpoint_hook` | BACKFILL → FR-037 | Applied |
| SC-006 vs FR-015 internal conflict | ALIGN (wording fix) | Applied |
| FR-016 second stale-doc site (README.md) | ALIGN (wording fix, spec-side disclosure only) | Applied |
| FR-030 `volatile_write_cache` cfg-polarity inversion | DEFECT, Severity: Major | Deferred → `align-tasks.md` |
| README.md "five minutes" checkpoint-interval claim | ALIGN, Severity: Low | Deferred → `align-tasks.md` |
| README.md "format version (5)" claim | ALIGN, Severity: Low | Deferred → `align-tasks.md` |

- `spec.md` functional requirements: FR-001..FR-034 → FR-001..FR-037 (FR-035, FR-036, FR-037
  added; no existing FR renumbered).
- No source code touched; only `spec.md` and `.specify/sync/align-tasks.md` /
  `.specify/sync/apply-report.md` were edited.
- Backups available at `.specify/sync/backups/20260722-162021/{spec.md,plan.md,tasks.md}`
  for rollback.
