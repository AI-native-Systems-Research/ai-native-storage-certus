# Sync Proposals: Dispatcher Component

**Generated**: 2026-05-21  
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`  
**Status**: 3 approved and applied, 2 deferred for human decision

---

## Applied Proposals

### BACKFILL-001: Remove BlockDeviceVersion selection requirement (FR-026)

**Drift**: drift-2 — No `block_device_version` field in `DispatcherConfig`. Single block device implementation hardcoded.

**Resolution**: FR-026 marked as REMOVED/superseded in spec.md. The `block_device_version` field removed from `DispatcherConfig` in contracts/idispatcher.md. Only one block device implementation exists; version selection is unnecessary overhead.

---

### BACKFILL-002: Remove ExtentManagerVersion selection requirement (FR-027)

**Drift**: drift-3 — No `extent_manager_version` field in `DispatcherConfig`. Single extent manager hardcoded.

**Resolution**: FR-027 marked as REMOVED/superseded in spec.md. The `extent_manager_version` field removed from `DispatcherConfig` in contracts/idispatcher.md. Only one extent manager implementation exists; version selection is unnecessary overhead.

---

### BACKFILL-003: Rename DispatcherComponentV0 to DispatcherComponent

**Drift**: Component type renamed in code to `DispatcherComponent` (dropped the V0 suffix).

**Resolution**: All references in spec companion files updated:
- `quickstart.md` — usage example
- `contracts/idispatcher.md` — wiring diagram
- `design/DESIGN.md` — architecture diagram
- `CLAUDE.md` — component wiring section

---

## Deferred Proposals (require human decision)

### DEFER-001: lookup_async not in spec (FR-001, drift-1)

The interface includes a `lookup_async` method not listed in spec FR-001. All spec-required methods are present. Options:
1. Add `lookup_async` to the FR-001 method list
2. Create a new requirement FR-036 for async lookup

### DEFER-002: initialize() rejects empty data_pci_addrs before spdk_env check (User Story 5 Scenario 4, drift-4)

`initialize()` rejects empty `data_pci_addrs` before checking whether `spdk_env` is connected. This means memory-tier-only mode requires dummy PCI addresses. Options:
1. Update code to move the empty-check inside the `spdk_env.is_connected()` block
2. Update spec to clarify that `data_pci_addrs` must always be non-empty regardless of mode
