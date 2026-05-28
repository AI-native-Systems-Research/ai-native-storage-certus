# Sync Apply Report

**Date**: 2026-05-21
**Component**: dispatch-map
**Spec**: 001-dispatch-map

## Actions Taken

### 1. BACKFILL: Component naming (DispatchMapComponentV0 -> DispatchMapComponent)

**Files modified**:
- `specs/001-dispatch-map/data-model.md` — updated relationship diagram
- `specs/001-dispatch-map/quickstart.md` — updated usage example (2 occurrences)
- `CLAUDE.md` — updated architecture wiring diagram

**Verification**: All occurrences of `DispatchMapComponentV0` replaced with `DispatchMapComponent` in spec and doc files.

### 2. BACKFILL: FR-018 MemoryTier field types

**Files modified**:
- `specs/001-dispatch-map/spec.md` — FR-018 requirement text updated
- `specs/001-dispatch-map/spec.md` — Key Entities section updated

**Changes**:
- `pointer: u64` -> `pointer: *mut u8`
- `size: usize` -> `size: u32`
- Added clarifying note that `create_memory_tier_entry` accepts `*mut u8` pointer and `u32` size

**Verification**: Key Entities Location enum definition now matches FR-018 and the implementation.

### 3. NO-OP: SC-004 Memory bound

No changes needed. The spec language already says "varies by variant" which is permissive enough to cover the implementation's 56-byte bound.

### 4. BACKFILL: FR-004 LookupResult::BlockDevice returns only offset

**Files modified**:
- `specs/001-dispatch-map/spec.md` — FR-004 requirement text updated: `BlockDevice(offset, size)` → `BlockDevice(offset)`
- `specs/001-dispatch-map/spec.md` — Key Entities Location enum updated: removed `size_blocks: u32` from `BlockDevice` variant, added note that `size_blocks` lives on `DispatchEntry`

**Changes**:
- FR-004: `BlockDevice(offset, size)` → `BlockDevice(offset)` with clarifying note that size is stored internally in `DispatchEntry` but not exposed in the `LookupResult::BlockDevice` variant
- Key Entities: `BlockDevice { offset: u64, size_blocks: u32 }` → `BlockDevice { offset: u64 }` with note that `size_blocks` is on `DispatchEntry`

**Verification**: `LookupResult::BlockDevice { offset: u64 }` in `components/interfaces/src/idispatch_map.rs` and `Location::BlockDevice { offset: u64 }` in `components/dispatch-map/src/entry.rs` both confirm only `offset` is present. The `data-model.md` LookupResult table already showed `offset: u64` only — no change needed there.

## Not Applied (Pending Human Decision)

(None — all identified discrepancies have been resolved.)

## Post-Apply State

| Requirement | Status |
|-------------|--------|
| FR-018 | Aligned (spec updated to match code) |
| SC-004 | Aligned (no change needed) |
| Naming | Aligned (spec updated to match code) |
| FR-004 | Aligned (spec updated to match code) |
