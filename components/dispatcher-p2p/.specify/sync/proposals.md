# Spec Sync Proposals
Generated: 2026-07-21
Project: dispatcher-p2p
Spec: 001-gpudirect-cold-path
Base commit: 833e9f36e01f1df8a0e0fc57d5cd223d823d3199..HEAD

---

## Proposal 1 — Add FR-015: multi-GPU device selection via IGpuServices

- **Direction**: BACKFILL (code authoritative — align spec.md to the code change)
- **Classification**: UNSPECCED new behavior (new interface dependency). Not drift-vs-existing-FR: no current FR references per-device GPU selection.

### Current State
- The `IGpuServices` interface gained `set_device(device: i32)` and `device_of_ptr(ptr) -> i32` (`components/interfaces/src/igpu_services.rs:555,577`), part of the repo-wide multi-GPU work (mirrored in the standard dispatcher).
- In dispatcher-p2p, `src/lib.rs` implements these two methods in its `MockGpuServices` test mock (`src/lib.rs:3091-3096`) so the component keeps compiling against the expanded trait.
- The rest of the diff is `rustfmt` reflow with no behavioral change.
- Production cold-path code in `src/lib.rs` does **not** yet call `set_device`/`device_of_ptr` (they appear only in the mock). The component now depends on a device-selection-capable receptacle, but per-device P2P routing is not yet wired.
- No existing FR (FR-001..FR-014) covers GPU device selection.

### Proposed Resolution
Add to spec.md, Functional Requirements:

> **FR-015**: The component's `IGpuServices` receptacle MUST provide multi-GPU device selection — `set_device(device)` to bind the active CUDA device and `device_of_ptr(ptr)` to resolve the GPU a device pointer resides on — so that cold-path staging-ring D2D copies and CUDA streams can target the client destination's GPU in multi-GPU deployments.

### Rationale
The `IGpuServices` contract dispatcher-p2p depends on now includes device selection; the component's mock was updated to match, confirming the dependency at the interface boundary. Documenting FR-015 records this capability so a future cold-path change that routes staging-ring copies/streams to the destination pointer's device (via `device_of_ptr` + `set_device`) has a spec anchor. Text is scoped to the capability actually present in code (interface + mock), not to routing behavior that is not yet implemented.

### Confidence
- **High** that the `IGpuServices` dependency now exposes `set_device`/`device_of_ptr` (present in interface and satisfied by the mock).
- **Medium** on the routing intent — production P2P code does not yet call these methods, so the FR documents an available capability whose cold-path wiring is pending. Revisit wording if/when production routing lands.

### Approval
- [ ] Approved

---
