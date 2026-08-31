# Drift Resolution Proposals — Phase B

Generated: 2026-08-20
Based on: `components/dispatch-map/.specify/sync/drift-report.json`
Policy: `.specify/sync/PHASE_B_POLICY.md` (per-component note for `dispatch-map`)

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (drifted requirement, code → spec) | 0 |
| Backfill-Unspecced (new FR + scenarios) | 1 |
| Align (spec → code, task only, no code edit) | 2 |
| Resolved (already fixed on main thread) | 0 |
| Human Decision | 0 |
| New Specs | 0 |

The current drift report lists 2 drifted requirements (FR-012, FR-003) and 1 unspecced feature (`reuse_count`). Per the Phase B policy note, both drift items are genuine code bugs against correct spec requirements (→ ALIGN tasks), and the single unspecced feature is backfilled into the spec.

---

## Proposal 1: 001-dispatch-map / FR-012 (unbound `IEvictionPolicy` panics instead of erroring)

**Direction**: ALIGN (spec → code; task only, no code edit)

**Location**: `src/lib.rs:55` (also `:68`, `:392`, `:573`)

**Current State**:
- Spec (FR-012 + contract "no panics"): on initialization, the `IEvictionPolicy` receptacle MUST be connected and `initialize()` returns an error if it is unbound.
- Code: `initialize()` calls `self.get_pool_id()` first (`src/lib.rs:68`); `get_pool_id` does `self.eviction_policy.get().unwrap()` (`src/lib.rs:55`), which **panics** when the receptacle is unbound — before the graceful `map_err(NotInitialized)` guard at `:69-71` can run. The same unwrap-on-unbound pattern recurs in `create_memory_tier_entry` (`:392`), `recover_extent` (`:573`), and via `get_pool_id` in `oldest_keys` (`:373`).

**Rationale**: The spec/contract is the agreed, correct behavior; the panic is a real defect. Correct spec + buggy code ⇒ ALIGN. No spec change; queued as a code-side task in `align-tasks.md`.

**Before / After (spec text)**: unchanged — FR-012 already states the correct behavior.

---

## Proposal 2: 001-dispatch-map / FR-003 · US1-AS3 (missing null-pointer check)

**Direction**: ALIGN (spec → code; task only, no code edit)

**Location**: `src/lib.rs:381` (`create_memory_tier_entry`, `:381-424`)

**Current State**:
- Spec (FR-003, US1 Acceptance Scenario 3, Edge Cases bullet 1): `create_memory_tier_entry` with a null pointer returns an error and records no entry.
- Code: only `size == 0` is validated (`InvalidSize`, `src/lib.rs:387-389`); there is **no** `pointer.is_null()` guard, so a null `*mut u8` is accepted and an entry is recorded. `DispatchMapError` has no null-pointer variant.

**Rationale**: The spec is the agreed, correct behavior; the missing guard is a real defect. Correct spec + buggy code ⇒ ALIGN. No spec change; queued as a code-side task in `align-tasks.md`.

**Before / After (spec text)**: unchanged — US1/AS3 + Edge Cases already state the correct behavior.

---

## Proposal 3: 001-dispatch-map / `reuse_count` (unspecced feature)

**Direction**: BACKFILL-UNSPECCED (code → spec; applied to `spec.md`)

**Location**: `src/entry.rs:37`; `src/lib.rs:142-143, 220-222, 313-315`

**Current State**:
- Code: `DispatchEntry` carries a `reuse_count: AtomicU32` (`src/entry.rs:37`), initialized to 0 at entry creation and incremented (`fetch_add(1, Relaxed)`) on every read-reference acquisition — `lookup` (`:142`), `take_read` (`:221`), `downgrade_reference` (`:314`). It is read only in the `Debug` impl; no `IDispatchMap` accessor exposes it.
- Spec: no requirement covered it.

**Rationale**: This is a real, intentional, working per-entry field. Per the Phase B policy (dispatch-map: 1 unspecced ⇒ BACKFILL), the spec is brought up to reality rather than the field removed. The field is already part of the documented 56-byte `DispatchEntry` layout (FR-028), so SC-004's compactness statement is unaffected.

**Before (spec.md)**: No FR covering `reuse_count`. Dispatch Entry key entity listed location, size_blocks, read_ref, write_ref, EvictionHandle (+ feature-gated checksum) only.

**After (spec.md)**:
- Added **FR-029**: `DispatchEntry` MUST carry a per-entry `reuse_count` (`AtomicU32`), initialized to 0 at creation and incremented by 1 (relaxed ordering) on each read-reference acquisition (`lookup`, `take_read`, `downgrade_reference`); non-read-acquiring operations MUST NOT modify it; it is internal instrumentation surfaced only via `Debug` and MUST NOT be exposed through any `IDispatchMap` method; it does not affect eviction ordering or reference-count semantics.
- Updated **Key Entities → Dispatch Entry** to list the `reuse_count` (`AtomicU32`) field.
- Added **US2 / AS6** and **US4 / AS9** acceptance scenarios describing the increment behavior.
