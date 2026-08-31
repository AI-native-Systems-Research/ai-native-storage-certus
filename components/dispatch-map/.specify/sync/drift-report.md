# Dispatch-Map — Spec ↔ Implementation Drift Report

**Generated**: 2026-08-31 (re-analysis)
**Component**: `components/dispatch-map`
**Spec analyzed**: `001-dispatch-map` — *Dispatch Map Component* (Status: Complete, Last Synced 2026-08-20)

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 35 (29 FR + 6 SC) |
| Aligned | 33 |
| Drifted (code-side ALIGN) | 2 |
| Not Implemented | 0 |
| Unspecced | 0 |

**No commits have touched `components/dispatch-map/` or `components/interfaces/src/idispatch_map.rs` since the 2026-08-20 sync** (`git log --since=2026-08-20` is empty for both paths). The implementation is therefore byte-identical to the last analysis, and the drift picture is unchanged: the same two code-side ALIGN items remain open. The previously-unspecced `reuse_count` field was resolved by the 2026-08-20 backfill (now FR-029 + Key Entities), so there is no remaining unspecced code.

## Detailed Findings — `001-dispatch-map`

### Aligned ✓

All FR-001..FR-029 (excluding the two drifted below) and SC-001..SC-006 match the implementation, unchanged from the 2026-08-20 report. Notably confirmed this run:

- **FR-029** (`reuse_count: AtomicU32`, initialized to 0, incremented on `lookup`/`take_read`/`downgrade_reference`, internal-only via `Debug`): present at `src/entry.rs`, initialized at `src/lib.rs:105` (recovery walk), `:410` (`create_memory_tier_entry`), and in `recover_extent`. This was **unspecced** in the prior cycle and is now fully aligned — no longer drift.
- **FR-027/FR-028** (`integrity-check` feature: `set_checksum`/`get_checksum`, feature-gated field): aligned.
- **FR-020** (`initialize` rebuilds via `for_each_extent`; `Ok` with empty map when no extent manager bound): aligned (`src/lib.rs:67-116`).

### Drifted ⚠️ (both are code-side ALIGN — spec is correct, code violates it)

- **FR-012 — `initialize` must return an error (not panic) if `IEvictionPolicy` is unbound** — *moderate*. **STILL OPEN.**
  - Spec (FR-012): "On initialization, the `IEvictionPolicy` receptacle MUST be connected (returns an error if unbound)."
  - Actual: `initialize` calls `self.get_pool_id()` **first** (`src/lib.rs:68`); `get_pool_id` does `self.eviction_policy.get().unwrap()` (`src/lib.rs:55`), which **panics** when the receptacle is unbound, before the graceful `.map_err(|_| NotInitialized(...))?` guard at `src/lib.rs:69-71` is reachable. Same unwrap-on-unbound pattern in `create_memory_tier_entry` (`src/lib.rs:392`) and `recover_extent`.
  - Location: `src/lib.rs:55` (and `:68`, `:392`).

- **FR-003 / US1-AS3 — null pointer to `create_memory_tier_entry` must return an error** — *moderate*. **STILL OPEN.**
  - Spec: US1 Acceptance Scenario 3 (`spec.md:23`), Edge Cases (`spec.md:221`): "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded."
  - Actual: `create_memory_tier_entry` (`src/lib.rs:381-424`) validates only `size == 0` → `InvalidSize` (`:387-389`); there is **no `pointer.is_null()` guard** (`grep -n null src/lib.rs` → none). A null pointer is accepted and an entry is inserted.
  - Location: `src/lib.rs:381-424`.

### Not Implemented ✗

None.

## Unspecced Code

None. (`reuse_count` was backfilled as FR-029 in the 2026-08-20 sync.)

## Out-of-Scope Observations (informational — not editable by this sync)

| Item | Location | Note |
|---|---|---|
| Stale crate path in component docs | `components/dispatch-map/CLAUDE.md:35` | Points at `../../component-framework/crates/`; should be `../../lib/component-framework/crates/` (same relocation the dispatcher CLAUDE.md already fixed). CLAUDE.md is outside this sync's editable scope (`.specify/sync/**`, `specs/**`). |
| The two ALIGN items are code changes | `src/lib.rs` | Modifying `src/**` is out of this sync's scope. Retained as follow-up tasks in `align-tasks.md` for a code pass (`speckit-implement`). |

## Recommendations

1. Resolve the two outstanding ALIGN tasks in code (see `align-tasks.md`): reorder the `eviction_policy` binding check ahead of `get_pool_id()` / make `get_pool_id` fallible (FR-012); add a `pointer.is_null()` guard to `create_memory_tier_entry` (FR-003/US1-AS3).
2. No spec change required this cycle — the spec is correct and current; the two drift items are code defects, not spec lag.
