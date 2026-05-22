# Drift Resolution Proposals

Generated: 2026-05-21
Based on: drift-report from 2026-05-21

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 2 |
| Align (Spec -> Code) | 0 |
| Human Decision | 1 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Applied Proposals

### Proposal 2: Naming — DispatchMapComponentV0 -> DispatchMapComponent

**Direction**: BACKFILL (applied)

**Change**: Renamed all references from `DispatchMapComponentV0` to `DispatchMapComponent` in:
- `specs/001-dispatch-map/quickstart.md`
- `specs/001-dispatch-map/data-model.md`
- `CLAUDE.md`

**Rationale**: The component was renamed to drop the V0 suffix. Specs now reflect current naming.

---

### Proposal 3: FR-018 — MemoryTier field types

**Direction**: BACKFILL (applied)

**Change**: Updated FR-018 and Key Entities in `spec.md`:
- `pointer: u64` -> `pointer: *mut u8` (idiomatic Rust raw pointer)
- `size: usize` -> `size: u32` (consistent with size_blocks in BlockDevice variant)

**Rationale**: `*mut u8` is the standard Rust type for raw memory pointers. `u32` for size is consistent with the block-count type used elsewhere in the component (BlockDevice's `size_blocks: u32` and DispatchEntry's size field).

---

## Pending Proposals (Require Human Decision)

### Proposal 1: FR-004 — Missing size in LookupResult::BlockDevice

**Direction**: HUMAN_DECISION

**Current State**:
- Spec says: `BlockDevice(offset, size)` — lookup returns both offset and size
- Code does: `LookupResult::BlockDevice { offset }` — only offset, no size field

**Options**:
- A) **ALIGN** (Code -> match Spec): Add `size_blocks: u32` field to `LookupResult::BlockDevice`. This changes the interface trait and all pattern matches on `LookupResult` across the codebase.
- B) **BACKFILL** (Spec -> match Code): Update spec FR-004 to document offset-only return. Rationale: callers already know the size from their initial `create_staging` request or from the extent manager metadata, so returning it in the lookup result is redundant.

**Questions**:
- Do any callers of `lookup()` need the size without having it from another source?
- Is the size in the lookup result needed for safety checks (e.g., preventing out-of-bounds reads)?

**Confidence**: MEDIUM
