# Spec Drift Report

Generated: 2026-07-22T22:33:36Z
Project: spdk-env

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 24 |
| Aligned | 19 (79%) |
| Drifted | 4 (17%) |
| Not Implemented | 1 (4%) |
| Unspecced Features | 3 |

## Spec: 001-spdk-vfio-env — (unfilled template)

This spec directory contains only the raw `spec-template.md` scaffold (`git log` shows it was committed as-is in `bc14ec2`, never filled in — placeholders like `System MUST [specific capability, e.g., "allow users to create accounts"]` remain verbatim). It has no concrete FR-*/SC-* requirements to check against code. The feature area it was meant to cover (SPDK/VFIO environment init) was fully specified later in `002-spdk-env-vfio-init`.

**Recommendation**: delete `specs/001-spdk-vfio-env/` or add a `Status: Superseded by 002-spdk-env-vfio-init` marker so it isn't mistaken for an active, unimplemented spec.

## Spec: 002-spdk-env-vfio-init — SPDK/DPDK Environment Component with VFIO Device Iteration

### Aligned (19)

| Req | Description | Location |
|-----|-------------|----------|
| FR-001 | Lib crate using `define_component!`/`define_interface!` | `src/lib.rs:30-67` |
| FR-002 | `ISPDKEnv` with `init()` + device query methods | `src/lib.rs:34-56` |
| FR-003 | SPDK/DPDK init happens in `init()`, not construction | `src/env.rs:17-34` |
| FR-004 | Verifies `/dev/vfio` + `vfio-pci` module before init | `src/checks.rs:16-38` |
| FR-005 | R/W permission checks on vfio paths, specific-path errors | `src/checks.rs:44-148` |
| FR-006 | NVMe enumeration via `spdk_pci_enumerate`, non-attaching callback | `src/env.rs:115-185` |
| FR-007 | `eprintln!` diagnostics, no receptacles | `src/env.rs:43-180`, `src/lib.rs:58-67` |
| FR-008 | Non-root operation via uid/gid permission checks | `src/checks.rs:78-148` |
| FR-009 | Empty device list (not error) when nothing bound | `src/env.rs:115-184` (`Ok(devices)`) |
| FR-010 | Runnable example, no logger wiring required | `examples/spdk-env-example.rs` |
| FR-011 | Procedural, no threads/actor/message queues | `src/lib.rs` (no actor infra) |
| FR-012 | Cleans up SPDK/DPDK resources on Drop | `src/lib.rs:100-107` |
| FR-013 | Hugepage check (2MB and 1GB pools) | `src/checks.rs:154-175` |
| FR-014 | Singleton `AtomicBool`, cleared on failure/Drop | `src/env.rs:11,19-33,188-200` |
| FR-016 | `is_initialized() -> bool` | `src/lib.rs:54,95-97` |
| FR-017 | `device_count() -> usize` (no clone) | `src/lib.rs:51,88-93` |
| FR-018 | Local + mirrored `ISPDKEnv` def kept in sync | `src/lib.rs:34-56` vs `interfaces/src/ispdk_env.rs:1-28` (verified identical signatures) |
| SC-003 | Example compiles/runs, prints device info | `examples/spdk-env-example.rs` |
| SC-004 | All ops synchronous, no threads spawned | `src/lib.rs`, `src/env.rs` |

### Drifted (4)

1. **SC-001** — severity: **medium**
   - *Spec*: "discovers 100% of available (not in-use) VFIO-bound devices ... matching the devices visible in /sys/bus/pci/drivers/vfio-pci."
   - *Actual*: `enumerate_devices()` calls `spdk_pci_enumerate()` scoped to `spdk_pci_get_driver("nvme")` only. virtio-blk and other SPDK-supported/VFIO-bound device types are never discovered — only NVMe.
   - *Location*: `components/spdk-env/src/env.rs:164-181`

2. **SC-002** — severity: **low**
   - *Spec*: "...missing logger..." listed as one of the misconfiguration causes the component reports.
   - *Actual*: There is no logger receptacle at all (per FR-007), so this clause is stale/inapplicable — a leftover from before the logger receptacle was removed via clarification.
   - *Location*: `components/spdk-env/src/lib.rs:58-67`; `specs/002-spdk-env-vfio-init/spec.md:135`

3. **SC-005** — severity: **high**
   - *Spec*: "produces structured log messages through the framework's logging system during initialization and device discovery."
   - *Actual*: All diagnostics are bare `eprintln!()` calls, not routed through any framework logging actor — this directly contradicts FR-007 in the same spec document ("there is no logger receptacle").
   - *Location*: `components/spdk-env/src/env.rs:43,48,53-56,173,176-180`

4. **Edge Case — "partial initialization cleanup"** — severity: **medium**
   - *Spec*: "The component cleans up any partially initialized state and returns an error from `init()`."
   - *Actual*: `do_init()`'s error path only clears the `SPDK_ENV_ACTIVE` singleton flag; it never calls `spdk_env_fini()` to unwind a successful `init_spdk_env()` if a later step failed. Currently masked because `enumerate_devices()` is written to always return `Ok(..)` — `SpdkEnvError::DeviceProbeFailed` is defined in `interfaces` but never constructed in `spdk-env`. If enumeration is ever made fallible (e.g., to implement FR-015), this gap becomes a real resource leak.
   - *Location*: `components/spdk-env/src/env.rs:17-34` (do_init), `:115-185` (enumerate_devices)

### Not Implemented (1)

- **FR-015** — "skip devices that cannot be probed ... log a warning ... return only successfully probed devices." The requirement's own text already flags this as future work ("Future: not yet implemented"), consistent with the code. However, Acceptance Scenario 4 under User Story 1 (`spec.md:33`) still describes this behavior as working today — an internal spec inconsistency, not just a code gap.

## Unspecced Code

| Feature | Location | Approx. Size | Suggested Spec Action |
|---------|----------|---------------|------------------------|
| `DmaBuffer` DMA-safe allocation API (`new()`, `from_raw()`, Deref/DerefMut, Drop-time SPDK dealloc gated by `set_spdk_env_active`) | `src/dma.rs:6` (re-export); `interfaces/src/spdk_types.rs:177-420` (def); flag set in `src/env.rs:102,191` | ~250 lines (shared crate) | Add FR/data-model coverage for the allocation contract, `from_raw()` safety obligations, and the active-flag coordination protocol. |
| Explicit `fini()` teardown method on `ISPDKEnv` | `src/lib.rs:40-45,74-79`; `src/env.rs:187-200` | ~20 lines | Add an FR describing explicit `fini()` and its precondition (controllers detached / DMA freed first), distinct from Drop-based cleanup (FR-012). |
| Operator shell scripts: VFIO binding (`bind_vfio.sh`, 355 lines, interactive/status/bind/reset/bind-all/reset-all), kernel boot params (`add_kernel_options.sh`), permission setup (`cfg_user_spdk.sh`), device listing (`show_spdk_devices.sh`), dnf cache repair (`fix_dnf_cache.sh`, 129 lines) | `components/spdk-env/scripts/*.sh` | ~565 lines total | Reference these scripts by name in the spec's Assumptions section, which currently only says binding/hugepage config is "performed externally." |

## Conflicts (spec-internal / artifact-vs-artifact)

1. **Device-type scope contradiction**: the Clarifications answer ("All SPDK-supported device types bound to VFIO (NVMe, virtio-blk, etc.)") and User Story 1's narrative both claim broad device-type coverage, but FR-006 explicitly narrows scope to NVMe-only — and the code follows FR-006. (`spec.md:12`, `:22`, `:104` vs `src/env.rs:164-181`)

2. **Stale supporting docs**: `contracts/ispdk-env.md` and `data-model.md` still describe a `logger: ILogger` receptacle, a `connect(logger)` step in the usage contract, and a `SpdkEnvError::LoggerNotConnected` variant/state — none of which exist in code or in the current `spec.md`, which explicitly marks these as removed. These two files were not updated when the logger receptacle was dropped via clarification. (`contracts/ispdk-env.md:16,25,56-58,69,83`; `data-model.md:58,69,81-90`)

## Recommendations

1. Update **SC-005** to say diagnostics use `eprintln!` (matching FR-007), or reinstate a logger receptacle if structured framework logging is actually desired — pick one and make FR-007/SC-005 consistent.
2. Update **SC-001**/User Story 1/Clarifications to explicitly scope device discovery to NVMe-only (matching FR-006 and the code), or extend `enumerate_devices()` to cover virtio-blk and other SPDK-supported types if broader discovery is still wanted.
3. Regenerate `contracts/ispdk-env.md` and `data-model.md` to drop the `ILogger` receptacle, `LoggerNotConnected`, and the `connect(logger)` step from the usage contract and state-transition diagram.
4. Add explicit cleanup (`spdk_env_fini()`) to `do_init()`'s error path so partial-init cleanup is real, not merely implied by the fact that `enumerate_devices()` currently never fails.
5. Remove SC-002's reference to "missing logger" as a misconfiguration cause.
6. Add FR coverage for `DmaBuffer` and the explicit `fini()` method, or explicitly mark them out-of-scope/implementation-detail if they're not meant to be part of the specified contract.
7. Mark `specs/001-spdk-vfio-env/` as superseded or delete it.
