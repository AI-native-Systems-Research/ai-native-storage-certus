# Drift Report — `spdk-env`

Generated: 2026-08-07T15:31:01Z

Component: `components/spdk-env`
Specs analyzed:
- `specs/001-spdk-vfio-env/spec.md` (SUPERSEDED — unfilled template)
- `specs/002-spdk-env-vfio-init/spec.md` (Draft — authoritative)

## Summary

| Spec | Aligned | Drifted | Not Implemented | Notes |
|------|---------|---------|-----------------|-------|
| 001-spdk-vfio-env | 0 | 0 | 0 | Raw `spec-template.md` scaffold, self-marked SUPERSEDED. No real requirements to evaluate. |
| 002-spdk-env-vfio-init | 25 | 1 (doc, low) | 1 (spec-acknowledged) | Core functionality fully implemented. |

Overall: substantially aligned. The only true gap (FR-015 device-skip-on-busy) is explicitly documented in the spec as a future item. One low-severity stale wording issue in SC-002.

## Detailed Findings — 002-spdk-env-vfio-init

### Functional Requirements

| ID | Status | Evidence |
|----|--------|----------|
| FR-001 | Aligned | `define_component!`/`define_interface!` — `src/lib.rs:34,58` |
| FR-002 | Aligned | `ISPDKEnv` with device queries + `init()` — `src/lib.rs:34-56` |
| FR-003 | Aligned | Init on `init()` not construction; `do_init` — `src/lib.rs:70-72`, `src/env.rs:17` |
| FR-004 | Aligned | `/dev/vfio` + `vfio-pci` module check — `src/checks.rs:16-38` |
| FR-005 | Aligned | Permission checks on `/dev/vfio`, container, IOMMU groups — `src/checks.rs:44-149` |
| FR-006 | Aligned | `spdk_pci_enumerate` w/ NVMe driver, callback returns non-zero (no attach), collects BDF/IDs/class/NUMA/type — `src/env.rs:115-185` |
| FR-007 | Aligned | `eprintln!` diagnostics; component has no receptacles — `src/env.rs:43,48,53`, `src/lib.rs:58-67` |
| FR-008 | Aligned | Non-root path via per-uid/gid permission checks — `src/checks.rs:77-149` |
| FR-009 | Aligned | Empty enumerate returns `Ok(vec![])` — `src/env.rs:115-185` |
| FR-010 | Aligned | Runnable example — `examples/spdk-env-example.rs` |
| FR-011 | Aligned | Procedural component, no threads/queues — `src/lib.rs:58-67` |
| FR-012 | Aligned | `Drop` calls `do_fini` when initialized — `src/lib.rs:100-107` |
| FR-013 | Aligned | Hugepage (2MB/1GB) check — `src/checks.rs:154-175` |
| FR-014 | Aligned | Process-global `AtomicBool` singleton, cleared on failure & Drop — `src/env.rs:11,19-33,199` |
| FR-015 | **Not Implemented (spec-acknowledged)** | Spec text itself says "Future: not yet implemented. Currently all matching devices are claimed." Callback does not skip/warn per in-use device — `src/env.rs:115-185`. US1 scenario 4 and US2 depend on this. Low severity: documented. |
| FR-016 | Aligned | `is_initialized() -> bool` — `src/lib.rs:54,95-97` |
| FR-017 | Aligned | `device_count() -> usize` — `src/lib.rs:51,88-93` |
| FR-018 | Aligned | Local `ISPDKEnv` in `src/lib.rs:34`; mirror at `components/interfaces/src/ispdk_env.rs` — both define the same 5 methods (in sync) |
| FR-019 | Aligned | `fini()` calls `do_fini` + clears flag, idempotent (guarded by `initialized`) — `src/lib.rs:44,74-79`, `src/env.rs:188-200` |
| FR-020 | Aligned | `DmaBuffer` re-exported from `interfaces` — `src/dma.rs:6`; def + `from_raw` + Deref + active-flag gating at `components/interfaces/src/spdk_types.rs:190,208,213,293` |
| FR-021 | Aligned | All 5 scripts present: `scripts/{bind_vfio.sh,add_kernel_options.sh,cfg_user_spdk.sh,show_spdk_devices.sh,fix_dnf_cache.sh}` |

### Success Criteria

| ID | Status | Evidence |
|----|--------|----------|
| SC-001 | Aligned | Enumerates all VFIO-bound NVMe via `spdk_pci_enumerate` — `src/env.rs:164-181` |
| SC-002 | **Drifted (doc, low)** | Criterion still lists "missing logger" as a misconfiguration case, but the component has no logger receptacle (per FR-007 / removed US2 scenario 4). Stale wording; no code impact. |
| SC-003 | Aligned | Example builds & runs as non-root — `examples/spdk-env-example.rs` |
| SC-004 | Aligned | Synchronous, no threads spawned — `src/lib.rs`, `src/env.rs` |
| SC-005 | Aligned | `eprintln!` diagnostics; criterion already corrected to match FR-007 — `src/env.rs:43,48,53,173,176` |

## Detailed Findings — 001-spdk-vfio-env

This file is an unfilled `spec-template.md` scaffold, self-marked **SUPERSEDED** by 002 (banner lines 1-7). Its FR-001..FR-007 and SC-001..SC-004 are placeholder examples (e.g., "allow users to create accounts"), not requirements. Nothing to align against. Correctly handled — retained for history only.

Spec reference check: the banner points to `.specify/sync/drift-report.md (spec 001-spdk-vfio-env)`, which exists. No broken references.

## Unspecced Code

| Item | Location | Assessment |
|------|----------|------------|
| Testable `*_at` helper variants (`check_vfio_available_at`, `check_vfio_permissions_at`, `check_hugepages_at`) | `src/checks.rs:21,49,162` | Internal test seams; acceptable, not spec-worthy |
| `SpdkEnvError` variant set (`VfioNotAvailable`, `PermissionDenied`, `HugepagesNotConfigured`, `InitFailed`, `AlreadyInitialized`) | `components/interfaces/src/spdk_types.rs` | Supports FR-002/004/005/013/014 error reporting; implied by spec, no drift |

No unspecced public interface surface.

## Recommendations

1. Low priority: implement FR-015 (skip in-use devices with per-device warning) or downgrade US1 scenario 4 / US2 scenarios that assume it, to remove the acknowledged gap.
2. Trivial: drop the stale "missing logger" clause from SC-002 to match FR-007 (already corrected in SC-005).
3. Optional housekeeping: consider archiving/removing the superseded 001 template to avoid future confusion.
