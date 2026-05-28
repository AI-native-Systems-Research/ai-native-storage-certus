# Sync Apply Report: Dispatcher Component

**Date**: 2026-05-21  
**Operator**: speckit-sync-apply (automated)  
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`

## Summary

| Metric | Count |
|--------|-------|
| Proposals applied | 5 |
| Proposals deferred | 0 |
| Files modified | 6 |

## Changes Applied

### 1. FR-026 marked REMOVED (BACKFILL-001)

**File**: `spec.md` line 237  
**Before**: `FR-026: The dispatcher MUST support BlockDeviceVersion selection (V1, V2) via DispatcherConfig.`  
**After**: Marked as `~~REMOVED~~ (superseded 2026-05-21)` with explanation that a single block device is hardcoded.

**File**: `contracts/idispatcher.md`  
**Change**: Removed `block_device_version: BlockDeviceVersion` field from `DispatcherConfig` struct.

### 2. FR-027 marked REMOVED (BACKFILL-002)

**File**: `spec.md` line 238  
**Before**: `FR-027: The dispatcher MUST support ExtentManagerVersion selection via DispatcherConfig.`  
**After**: Marked as `~~REMOVED~~ (superseded 2026-05-21)` with explanation that a single extent manager is hardcoded.

**File**: `contracts/idispatcher.md`  
**Change**: Removed `extent_manager_version: ExtentManagerVersion` field from `DispatcherConfig` struct.

### 3. DispatcherComponentV0 renamed to DispatcherComponent (BACKFILL-003)

All occurrences of `DispatcherComponentV0` replaced with `DispatcherComponent` in:
- `specs/001-dispatcher-cache-interface/quickstart.md`
- `specs/001-dispatcher-cache-interface/contracts/idispatcher.md`
- `design/DESIGN.md`
- `CLAUDE.md`

### 4. FR-036 added for lookup_async (BACKFILL-004)

**File**: `spec.md`  
**Change**: Added FR-036 documenting the `lookup_async` method that returns a `GpuStream` for non-blocking H2D DMA. Updated FR-001 method list to include `lookup_async`.

**Rationale**: The `IDispatcher` interface defines `lookup_async` which performs the same cache lookup as `lookup` but returns asynchronously via a GpuStream. The synchronous `lookup` delegates to it internally. This was previously drift-1 (deferred).

### 5. User Story 5 Scenario 4 updated for data_pci_addrs requirement (BACKFILL-005)

**File**: `spec.md`  
**Change**: Updated US5-S4 scenario text to clarify that `data_pci_addrs` must always be provided (non-empty) regardless of spdk_env connection state. The `initialize()` method rejects empty `data_pci_addrs` with `InvalidParameter` before evaluating spdk_env.

**Rationale**: The spec previously implied memory-tier-only mode could work without data PCI addresses, but the code validates `data_pci_addrs` is non-empty before checking SPDK. This was previously drift-4 (deferred).

## Deferred Items

None. All previously deferred items have been resolved.

## Post-Apply Drift Status

| Requirement | Previous Status | New Status |
|-------------|----------------|------------|
| FR-026 | not_implemented | resolved (requirement removed) |
| FR-027 | not_implemented | resolved (requirement removed) |
| FR-001 | drifted | resolved (lookup_async added) |
| US5-S4 | drifted | resolved (data_pci_addrs clarified) |
