# Spec Sync: Align Tasks

Generated: 2026-07-22T21:28:13Z
Based on: `.specify/sync/drift-report.md` / `drift-report.json`
Mode: AUTO-BACKFILL apply pass

These are code-side (or code-adjacent) changes needed to align implementation
with the specification, or decisions requiring a human call. None of these
were applied automatically — spec-sync-apply only edits Markdown under
`specs/**` and `.specify/sync/**`.

---

## Task: Align 001-dispatch-map/FR-014

**Severity**: Moderate
**Spec Requirement**: FR-014 — "System MUST use the `ILogger` receptacle for info, debug, and error logging throughout the component."
**Current Code**: `components/dispatch-map/src/lib.rs` calls `logger.info(...)` and `logger.debug(...)` in various places, but `logger.error(...)` is never invoked anywhere in the component — despite numerous fallible paths (`KeyNotFound`, `Timeout`, `ActiveReferences`, `RefCountUnderflow`/`Overflow`, `InvalidState`, `InvalidSize`) that return `Err` without logging anything.
**Required Change**: Add `logger.error(...)` calls on the component's error-return paths (timeouts, invalid-state transitions, ref-count violations, etc.) so the implementation actually satisfies "error logging throughout the component" as the spec requires. The spec's wording is correct and intentional (per the original functional design) — this is a code defect, not a spec-wording issue, so the spec was left unchanged.
**Files to Modify**: `components/dispatch-map/src/lib.rs`
**Estimated Effort**: Small

### Acceptance Criteria
- [ ] Every `Err(...)` return path in `IDispatchMap` method implementations has a corresponding `logger.error(...)` call (or an explicit, documented rationale for why a given path is exempt).
- [ ] `cargo test -p dispatch-map` and `cargo clippy -p dispatch-map -- -D warnings` still pass.
- [ ] Drift report re-run shows FR-014 moved from "drifted" to "aligned".

---

## Task: Align 001-dispatch-map/US1-AS3 (null-pointer rejection)

**Severity**: Moderate
**Spec Requirement**: User Story 1, Acceptance Scenario 3 and Edge Cases: "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded in the map."
**Current Code**: `create_memory_tier_entry` (`components/dispatch-map/src/lib.rs:367-408`) has no null-pointer check — a null `*mut u8` is currently accepted and stored without error. `DispatchMapError` (`components/interfaces/src/idispatch_map.rs`) has no dedicated null-pointer variant to report this condition.
**Required Change**: Add a null-pointer check at the top of `create_memory_tier_entry` (and consider the same check in `promote_block_to_memory_tier`, which accepts the same kind of externally-allocated pointer). Add a new `DispatchMapError` variant (e.g. `NullPointer(CacheKey)`) and return it before any map mutation occurs. The spec's requirement is correct and was an explicit acceptance scenario / edge case in the original functional design — this is a code gap, not a spec error, so the spec was left unchanged.
**Files to Modify**: `components/dispatch-map/src/lib.rs`, `components/interfaces/src/idispatch_map.rs`
**Estimated Effort**: Small

### Acceptance Criteria
- [ ] `create_memory_tier_entry(key, std::ptr::null_mut(), size)` returns an error and does not insert an entry.
- [ ] New `DispatchMapError` variant added with `Display`/`Error` impl coverage.
- [ ] Unit test added covering the null-pointer rejection path (currently untested).
- [ ] Decide and apply the same check to `promote_block_to_memory_tier`, or explicitly document why it is exempt.

---

## Task: Align 001-dispatch-map/interfaces-verification-comment (stale `verif/` reference)

**Severity**: Minor (defect — stale/misleading comment, not a functional bug)
**Spec Requirement**: N/A — this is a code-comment defect uncovered during drift analysis, not a spec requirement mismatch.
**Current Code**: `components/interfaces/src/idispatch_map.rs:84-99` contains a comment block claiming 10 formally-verified properties (P1-P10) and "24 verification conditions discharged by SMT solvers," referencing `components/dispatch-map/verif/` — a directory that does not exist anywhere under this component.
**Required Change**: Either (a) correct the comment to state these are documented invariants / test-covered properties rather than Creusot-verified ones, and remove the `verif/` path reference, or (b) actually add the referenced Creusot verification harness under `components/dispatch-map/verif/` (see the `tools-creusot-*` skills) if formal verification is genuinely intended for this component.
**Files to Modify**: `components/interfaces/src/idispatch_map.rs`
**Estimated Effort**: Small (option a) / Medium (option b)

### Acceptance Criteria
- [ ] The comment no longer references a non-existent directory.
- [ ] If claiming formal verification, a corresponding `verif/` harness exists and is buildable; otherwise the claim is removed or downgraded to "documented invariant."

---

## Task: Align 001-dispatch-map/reuse_count (dead metric)

**Severity**: Minor / Ambiguous — human decision needed
**Spec Requirement**: N/A — `reuse_count` is unspecced code, not a spec requirement.
**Current Code**: `DispatchEntry.reuse_count: AtomicU32` (`components/dispatch-map/src/entry.rs:37`) is incremented on `lookup`/`take_read`/`downgrade_reference` (`components/dispatch-map/src/lib.rs:101,137,213,301,396,567`) but is never read or exposed via any `IDispatchMap` method.
**Required Change**: Decide the field's fate:
  - (A) Expose it via a new `IDispatchMap` method (e.g. `reuse_count(key) -> Result<u32, DispatchMapError>`) and add a corresponding FR + acceptance scenario to `spec.md`, or
  - (B) Remove the field and its increment call sites as dead instrumentation.
This decision was not made during this sync pass (AUTO-BACKFILL only covers code that is genuinely consumed prod API; `reuse_count` has zero consumers) — deferred to a human.
**Files to Modify**: `components/dispatch-map/src/entry.rs`, `components/dispatch-map/src/lib.rs` (and `specs/001-dispatch-map/spec.md` if option A is chosen)
**Estimated Effort**: Small

### Acceptance Criteria
- [ ] A decision (A or B) is recorded and implemented.
- [ ] If (A): spec.md gains a new FR/scenario and data-model.md's note on this field is updated to reflect it is now exposed.
- [ ] If (B): data-model.md's note on this field is removed once the code is removed.

---

## Task: Align 001-dispatch-map/US2-AS4 (lookup size-mismatch path)

**Severity**: Minor — spec-acknowledged, pre-existing gap
**Spec Requirement**: User Story 2, Acceptance Scenario 4: "size mismatch → `ErrorMismatchSize`."
**Current Code**: `lookup(key)` (`components/interfaces/src/idispatch_map.rs`; `components/dispatch-map/src/lib.rs:115-158`) takes no expected-size parameter, so no caller can trigger `LookupResult::MismatchSize`. FR-004 itself already documents this as "not currently triggered" — this is a known, spec-acknowledged gap rather than newly discovered drift.
**Required Change**: No action required unless product requirements change. If size-checked lookups become a real need, add an optional expected-size parameter (or a separate `lookup_checked(key, expected_size)` method) and wire `MismatchSize` to it, updating FR-004 and User Story 2/AS4 accordingly. Left as-is (DEFERRED) for this sync pass since neither the spec nor any consumer currently requires this to be closed.
**Files to Modify**: `components/interfaces/src/idispatch_map.rs`, `components/dispatch-map/src/lib.rs` (only if/when actioned)
**Estimated Effort**: Small

### Acceptance Criteria
- [ ] N/A until a product decision is made to implement size-checked lookups.

---

## Task: Align 001-dispatch-map/SC-004 (struct-size wording)

**Severity**: Minor — spec-wording, not a functional defect
**Spec Requirement**: SC-004 — "Per-entry metadata is kept compact. The `DispatchEntry` struct size **varies** by `Location` variant."
**Current Code**: `Location` is a Rust `enum`; the compiler sizes `DispatchEntry` to fit its largest variant (`MemoryTier`) at compile time. `size_of::<DispatchEntry>()` (exposed via `entry_size()` in `lib.rs`) is a fixed constant — it does not vary per-instance based on which variant is active.
**Required Change**: This is the spec's wording that is imprecise about Rust enum memory layout, not a code defect — SC-004 was **not** modified during this AUTO-BACKFILL pass because the task's authorized scope was limited to backfilling FR-014 (defect) and the null-pointer gap (defect); SC-004 is neither. Recommend a follow-up wording fix: reword SC-004 to something like "The `DispatchEntry` struct has a fixed, compact size (`entry_size()`) regardless of which `Location` variant is active; per-variant fields (`BlockDevice`'s offset vs. `MemoryTier`'s pointer/size/ssd_offset) are compactly packed within that fixed layout by the compiler." Requires a human sign-off since it changes an existing Success Criterion's claim rather than adding new, uncontested coverage.
**Files to Modify**: `specs/001-dispatch-map/spec.md` (SC-004 only)
**Estimated Effort**: Small

### Acceptance Criteria
- [ ] Human reviews and approves the reworded SC-004 text.
- [ ] Drift report re-run shows SC-004 moved from "drifted" to "aligned" (or the criterion is retired if deemed not worth keeping).

---

## Task: Align 001-dispatch-map/test-coverage (promote/evict methods)

**Severity**: Minor — test-debt, not a spec or behavioral defect
**Spec Requirement**: User Story 10 and User Story 11 (backfilled this sync) — both list an "Independent Test" description.
**Current Code**: Neither `components/dispatch-map/src/lib.rs` (`#[cfg(test)]` unit tests) nor `components/dispatch-map/tests/integration.rs` currently contains any test exercising `promote_block_to_memory_tier` or `try_evict_to_block`, even though both are real, actively-consumed production API (by `components/dispatcher` and `components/dispatcher-p2p`). Coverage for these two methods currently exists only indirectly, via consumer-side mocks/tests in those two downstream components.
**Required Change**: Add unit tests in `dispatch-map` itself covering: (1) `promote_block_to_memory_tier` happy path, preserved-reference case, already-MemoryTier error, KeyNotFound, and size=0; (2) `try_evict_to_block` happy path, active-references rejection, missing-ssd_offset rejection, already-BlockDevice rejection, and KeyNotFound — matching the acceptance scenarios now documented in User Story 10/11 of `spec.md`.
**Files to Modify**: `components/dispatch-map/src/lib.rs` (test module), `components/dispatch-map/tests/integration.rs`
**Estimated Effort**: Small

### Acceptance Criteria
- [ ] All five User Story 10 acceptance scenarios and all five User Story 11 acceptance scenarios have a corresponding automated test.
- [ ] `cargo test -p dispatch-map -- --test-threads 1` passes.

---

## Task: Align 001-dispatch-map/companion-docs (README.md, CLAUDE.md)

**Severity**: Minor — documentation drift, outside this pass's edit scope
**Spec Requirement**: N/A — these are component-level docs, not spec artifacts. Called out by `.specify/sync/drift-report.md` recommendation #6.
**Current Code/Docs**: `components/dispatch-map/README.md` describes a `Staging`/`DmaBuffer` location variant and RDTSC `tsc` timestamps that no longer exist, and never mentions `IEvictionPolicy`, `promote_block_to_memory_tier`, `try_evict_to_block`, `is_evictable`, `recover_extent`, or `entry_size`. `components/dispatch-map/CLAUDE.md`'s "Component Wiring" diagram omits the mandatory `IEvictionPolicy` receptacle. Neither file is under `specs/**` or `.specify/sync/**`, so this AUTO-BACKFILL pass (scoped to those two paths only) could not update them directly.
**Required Change**: Refresh `README.md` and `CLAUDE.md` to match the current implementation and the now-updated `specs/001-dispatch-map/{data-model,contracts/idispatch_map,quickstart}.md`. Use the `component-update-docs` skill (`components/dispatch-map:component-update-docs`).
**Files to Modify**: `components/dispatch-map/README.md`, `components/dispatch-map/CLAUDE.md`
**Estimated Effort**: Small

### Acceptance Criteria
- [ ] README.md's Architecture/Data Structure section matches `entry.rs`/`state.rs` (no `Staging`/`DmaBuffer`/`tsc` references).
- [ ] README.md documents `IEvictionPolicy`, `promote_block_to_memory_tier`, `try_evict_to_block`, `is_evictable`, `recover_extent`, `entry_size`.
- [ ] CLAUDE.md's Component Wiring diagram includes the `IEvictionPolicy` receptacle.
