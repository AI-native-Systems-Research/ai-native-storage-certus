# Spec Drift Report
Generated: 2026-06-18
Project: spdk-env

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 18 |
| Aligned | 14 (78%) |
| Drifted | 3 (17%) |
| Not Implemented | 1 (6%) |
| Unspecced Code | 2 |

## Detailed Findings
### Spec: 001-spdk-vfio-env - (Empty Template)
#### Aligned
(none - this spec contains only template placeholders with no concrete FR-* requirements)
#### Drifted
(none)
#### Not Implemented
(none)

### Spec: 002-spdk-env-vfio-init - SPDK/DPDK Environment Component with VFIO Device Iteration
#### Aligned
- FR-001: Rust lib crate using `define_component!` and `define_interface!` macros → `src/lib.rs:30-67`
- FR-002: `ISPDKEnv` interface with `init()`, `devices()`, `device_count()`, `is_initialized()` → `src/lib.rs:34-56`
- FR-003: SPDK/DPDK environments initialized in `init()`, not during construction → `src/env.rs:17-34`; constructor only sets empty state
- FR-004: Verifies /dev/vfio exists and vfio-pci kernel module loaded → `src/checks.rs:16-38`
- FR-005: Checks R/W permissions on /dev/vfio, /dev/vfio/vfio, IOMMU group files with specific path and uid/gid in error messages → `src/checks.rs:44-148`
- FR-006: Enumerates NVMe devices via `spdk_pci_enumerate` with NVMe PCI driver, provides BDF address, vendor/device IDs, class ID, NUMA node, device type string; callback returns non-zero to avoid attaching → `src/env.rs:113-185`
- FR-008: Operates without root when /dev/vfio has appropriate user-level access; permission checks use getuid/getegid → `src/checks.rs:78-148`
- FR-009: Returns empty device list (not error) when no devices are bound → `src/env.rs:115` returns `Ok(devices)` where devices may be empty
- FR-011: Plain procedural component (not an actor); no thread spawning or message queues → no actor infrastructure present
- FR-012: Drop impl calls `spdk_env_fini()` and clears singleton flag → `src/lib.rs:100-107`
- FR-013: Checks hugepage availability (2MB and 1GB pools), reports clear error with allocation hint → `src/checks.rs:154-175`
- FR-014: Singleton via process-global `SPDK_ENV_ACTIVE` AtomicBool with CAS; flag cleared on failure path and on Drop → `src/env.rs:11,19-33` and `src/env.rs:188-200`
- FR-016: `is_initialized() -> bool` method on ISPDKEnv → `src/lib.rs:54,95-97`
- FR-017: `device_count() -> usize` method on ISPDKEnv → `src/lib.rs:51,88-93`

#### Drifted
- FR-007: Spec says "System uses `eprintln!` for diagnostic output. There is no logger receptacle; the component has no receptacles." Code is aligned on the eprintln! usage and absence of receptacles. However, the code also provides an explicit `fini()` method not covered by this requirement or any other FR-*.
  - Location: src/lib.rs:44-47, src/env.rs:188-200
  - Severity: minor
  - Notes: The spec describes the component as having no receptacles and using eprintln!, which matches. The `fini()` method is an unspecced addition to the interface beyond what FR-007 describes about the component's diagnostic behavior.

- FR-010: Spec says "System MUST include a test example (main.rs binary)" but code provides `examples/spdk-env-example.rs` (a Cargo example, not a binary target with main.rs). Spec also mentions "wires the logger" which contradicts FR-007's statement that no logger receptacle exists.
  - Location: examples/spdk-env-example.rs
  - Severity: minor
  - Notes: The example fulfills the same purpose (demonstrates construct-init-query lifecycle) but differs in project structure (Cargo example vs bin target). The logger-wiring reference is a spec-internal inconsistency.

- FR-018: Spec says ISPDKEnv must be defined in BOTH the `spdk-env` crate AND the shared `interfaces` crate and must stay in sync. The two definitions are currently identical in signature (`init`, `fini`, `devices`, `device_count`, `is_initialized`) but there is no compile-time mechanism to enforce sync.
  - Location: src/lib.rs:34-56 vs components/interfaces/src/ispdk_env.rs:1-28
  - Severity: minor
  - Notes: Definitions are in sync today. Drift risk exists because changes to one do not automatically fail compilation in the other until a downstream consumer triggers a type mismatch.

#### Not Implemented
- FR-015: System MUST skip devices that cannot be probed (e.g., in use by another process), log a warning for each skipped device, and return only successfully probed devices.
  - Notes: The current `enumerate_devices()` uses `spdk_pci_enumerate` which discovers all visible PCI devices. The callback deliberately does NOT attach (returns 1), so there is no probe attempt that could detect whether a device is in use by another process. All visible devices are returned unconditionally. The "skip unavailable devices with warning" logic is absent.

## Unspecced Code
- **DmaBuffer re-export and allocation API**: `src/dma.rs` re-exports `interfaces::DmaBuffer` which provides hugepage-backed DMA buffer management (`spdk_dma_zmalloc`, `from_raw` for external allocators, NUMA-aware allocation). No FR-* requirement covers DMA buffer functionality.
  - Location: src/dma.rs, interfaces/src/spdk_types.rs:180-420

- **Explicit `fini()` lifecycle method**: `ISPDKEnv` exposes `fini(&self)` for explicit SPDK environment teardown before Drop. This enables correct ordering when NVMe controllers or DMA buffers must be released before the environment shuts down. No FR-* requirement specifies this method.
  - Location: src/lib.rs:44-47, src/lib.rs:74-78, src/env.rs:188-200
