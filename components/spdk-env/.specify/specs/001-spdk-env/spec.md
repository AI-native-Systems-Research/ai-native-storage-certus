# Feature Specification: SPDK Environment Initialization

**Feature Branch**: `001-spdk-env`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `spdk-env` component provides a safe Rust wrapper around SPDK/DPDK environment initialization and VFIO device discovery for the Certus storage system. It implements the `ISPDKEnv` interface as a component in the Certus COM-inspired component framework, managing the full SPDK runtime lifecycle: pre-flight system validation, singleton-enforced environment initialization via `spdk_env_init`, PCI device enumeration through VFIO, and clean teardown on drop.

The component exists to translate opaque DPDK EAL initialization failures into actionable, user-friendly error messages. Rather than letting SPDK fail with cryptic error codes, it performs ordered pre-flight checks (VFIO availability, file permissions, hugepage allocation) and returns structured errors with specific remediation commands (e.g., `modprobe vfio-pci`, hugepage allocation instructions).

## User Scenarios & Testing

### User Story 1 - Environment Initialization (Priority: P1)

As a Certus storage system operator, I want to initialize the SPDK/DPDK environment with a single method call, so that NVMe devices become available for the block device component to use.

**Acceptance Scenarios**:

- **Given** a Linux host with IOMMU enabled, hugepages allocated, and NVMe devices bound to VFIO, **when** `ISPDKEnv::init()` is called, **then** the SPDK environment is initialized and all VFIO-attached NVMe devices are enumerated and available via `devices()`.
- **Given** a host without the `vfio-pci` kernel module loaded, **when** `init()` is called, **then** it returns `SpdkEnvError::VfioNotAvailable` with a message containing `modprobe vfio-pci`.
- **Given** a host with no hugepages allocated, **when** `init()` is called, **then** it returns `SpdkEnvError::HugepagesNotConfigured` with allocation instructions.
- **Given** the SPDK environment is already initialized in this process, **when** `init()` is called a second time (on a different component instance), **then** it returns `SpdkEnvError::AlreadyInitialized`.

### User Story 2 - Device Discovery (Priority: P1)

As the block device component, I want to enumerate available NVMe devices after environment initialization, so that I can probe and attach them via `spdk_nvme_probe`.

**Acceptance Scenarios**:

- **Given** the environment is initialized with N VFIO-bound NVMe devices, **when** `devices()` is called, **then** it returns a `Vec<VfioDevice>` of length N with correct PCI addresses, vendor/device IDs, NUMA node assignments, and device type strings.
- **Given** the environment is initialized, **when** `device_count()` is called, **then** it returns the same value as `devices().len()`.
- **Given** the environment is not yet initialized, **when** `devices()` is called, **then** it returns an empty vector.
- **Given** `devices()` is called multiple times, **when** the results are compared, **then** each call returns an independent clone (mutations to one do not affect the other).

### User Story 3 - Pre-flight Validation (Priority: P1)

As a system administrator deploying Certus, I want clear error messages when system prerequisites are not met, so that I can quickly resolve configuration issues.

**Acceptance Scenarios**:

- **Given** `/dev/vfio` does not exist, **when** the VFIO availability check runs, **then** the error message includes `modprobe vfio-pci`.
- **Given** the `/dev/vfio/vfio` container device exists but the user lacks read+write permissions, **when** the permissions check runs, **then** the error includes the user's UID/GID, the file mode, and a hint about udev rules.
- **Given** IOMMU group device files under `/dev/vfio/` have restrictive permissions, **when** the permissions check runs, **then** each inaccessible numeric group entry is flagged.
- **Given** non-numeric entries under `/dev/vfio/` (e.g., `vfio` container), **when** the permissions check runs for IOMMU groups, **then** non-numeric entries are skipped.
- **Given** hugepage sysfs files contain non-numeric or empty content, **when** the hugepages check runs, **then** it treats them as zero allocation and fails with an appropriate message.

### User Story 4 - Clean Teardown (Priority: P2)

As the Certus process lifecycle manager, I want the SPDK environment to be cleanly torn down when the component is dropped, so that DPDK atexit handlers do not access freed resources.

**Acceptance Scenarios**:

- **Given** the environment is initialized, **when** `fini()` is called explicitly, **then** `spdk_env_fini()` is invoked, the singleton flag is released, and `is_initialized()` returns false.
- **Given** the environment is initialized and `fini()` was not called, **when** the `SPDKEnvComponent` is dropped, **then** `spdk_env_fini()` is called automatically.
- **Given** the environment was never initialized, **when** the component is dropped, **then** no finalization occurs and no panic is raised.
- **Given** `fini()` has already been called, **when** the component is subsequently dropped, **then** `spdk_env_fini()` is not called a second time.

### User Story 5 - Singleton Enforcement (Priority: P1)

As the Certus framework, I want at most one SPDK environment active per process, so that DPDK's process-global state is not corrupted by concurrent initializations.

**Acceptance Scenarios**:

- **Given** one component has successfully called `init()`, **when** a second component calls `init()`, **then** it receives `SpdkEnvError::AlreadyInitialized`.
- **Given** `init()` fails during pre-flight checks, **when** the singleton flag state is inspected, **then** it has been released (allowing a retry).
- **Given** the environment was initialized and then finalized (via `fini()` or drop), **when** a new component calls `init()`, **then** it succeeds (singleton flag was released).

## Requirements

### Functional Requirements

- **FR-001**: The component SHALL implement `ISPDKEnv` with methods `init()`, `fini()`, `devices()`, `device_count()`, and `is_initialized()`.
- **FR-002**: `init()` SHALL enforce a process-global singleton via an `AtomicBool` with `compare_exchange` (AcqRel/Acquire ordering).
- **FR-003**: `init()` SHALL execute pre-flight checks in order: VFIO availability, VFIO permissions, hugepages.
- **FR-004**: If any pre-flight check fails, `init()` SHALL release the singleton flag and return an appropriate `SpdkEnvError` variant.
- **FR-005**: `init()` SHALL call `spdk_env_init()` with app name `certus-spdk-env` and `shm_id = -1`.
- **FR-006**: `init()` SHALL enumerate NVMe PCI devices using `spdk_pci_enumerate` with the NVMe driver, returning non-zero from the callback to avoid claiming devices.
- **FR-007**: Discovered devices SHALL be stored in a `RwLock<Vec<VfioDevice>>` and returned as clones from `devices()`.
- **FR-008**: `fini()` SHALL call `spdk_env_fini()` only if the environment is currently initialized, then set `initialized` to false.
- **FR-009**: The component SHALL implement `Drop` to call `fini()` if still initialized, preventing resource leaks.
- **FR-010**: VFIO availability check SHALL verify both `/dev/vfio` existence and `/sys/bus/pci/drivers/vfio-pci/` existence.
- **FR-011**: VFIO permissions check SHALL verify read access to `/dev/vfio`, read+write access to `/dev/vfio/vfio`, and read+write access to all numeric entries (IOMMU groups) under `/dev/vfio/`.
- **FR-012**: Hugepages check SHALL verify that at least one of 2MB or 1GB hugepage pools has a non-zero allocation count.
- **FR-013**: `init()` SHALL call `interfaces::set_spdk_env_active(true)` after successful SPDK initialization, and `do_fini()` SHALL call `interfaces::set_spdk_env_active(false)` before calling `spdk_env_fini()`.
- **FR-014**: The component SHALL implement `IUnknown` (via `define_component!`) for runtime interface discovery.

### Non-Functional Requirements

- **NFR-001**: Error messages SHALL be actionable, including specific remediation commands (e.g., `modprobe vfio-pci`, hugepage allocation commands).
- **NFR-002**: Error messages for permission failures SHALL include the current UID, GID, file mode, and owner information.
- **NFR-003**: The component SHALL use `unsafe` code only for FFI calls to SPDK/DPDK and libc, with `// SAFETY:` justification comments on each block.
- **NFR-004**: Pre-flight checks SHALL be testable without hardware via dependency injection of filesystem paths (internal `_at` variants).
- **NFR-005**: The component SHALL be safe to use from multiple threads (`Send + Sync`), with device access protected by `RwLock` and initialization state by `AtomicBool`.
- **NFR-006**: The singleton guard SHALL use appropriate memory ordering (`AcqRel` for compare-exchange, `Acquire` for loads, `Release` for stores) to ensure correctness on weakly-ordered architectures.
- **NFR-007**: The component SHALL NOT claim/attach discovered PCI devices during enumeration, leaving them available for subsequent `spdk_nvme_probe` by downstream components.
- **NFR-008**: Diagnostic messages during initialization (pre-flight pass, device count) SHALL be written to stderr via `eprintln!`.

## Key Entities

| Entity | Description |
|--------|-------------|
| `SPDKEnvComponent` | The component struct holding discovered devices and initialization state. Created via `define_component!`. |
| `ISPDKEnv` | The interface trait defining the public API. Created via `define_interface!`. |
| `VfioDevice` | Immutable snapshot of a discovered PCI device (address, IDs, NUMA node, type). |
| `PciAddress` | Domain:Bus:Device.Function identifier. `Copy`, `Eq`, `Hash`. Displays as `DDDD:BB:DD.F`. |
| `PciId` | PCI class, vendor, device, and subsystem identifiers. `Copy`, `Eq`. |
| `DmaBuffer` | DMA-safe buffer type (defined in `interfaces` crate, re-exported here). |
| `SpdkEnvError` | Error enum covering all failure modes (defined in `interfaces` crate, re-exported here). |
| `SPDK_ENV_ACTIVE` | Process-global `AtomicBool` enforcing the singleton constraint. |

## Dependencies

| Dependency | Type | Purpose |
|------------|------|---------|
| `component-framework` | Workspace crate | `define_component!`, `define_interface!`, `IUnknown` traits |
| `component-core` | Workspace crate | Core component traits and channel infrastructure |
| `interfaces` | Workspace crate (feature: `spdk`) | `DmaBuffer`, `SpdkEnvError`, `set_spdk_env_active()` |
| `spdk-sys` | Workspace crate | Raw FFI bindings to SPDK C libraries (`spdk_env_init`, `spdk_env_fini`, `spdk_pci_enumerate`, etc.) |
| `libc` | External (0.2) | `getuid()`, `getegid()` for permission checks |
| `tempfile` | Dev dependency (3) | Temporary directories for unit tests |

### System Prerequisites

- Linux kernel with IOMMU enabled (boot parameter: `intel_iommu=on` or `amd_iommu=on`)
- Hugepages allocated (2MB or 1GB)
- NVMe devices bound to `vfio-pci` driver (via SPDK `setup.sh`)
- SPDK built and installed at `deps/spdk-build/`
- User has read+write access to `/dev/vfio/` devices

## Success Criteria

1. `cargo build -p spdk-env` compiles without warnings under `clippy -D warnings`.
2. `cargo test -p spdk-env` passes all unit tests (pre-flight checks and device types) without requiring hardware.
3. On a properly configured host, `ISPDKEnv::init()` initializes SPDK, enumerates devices, and `devices()` returns the expected device list.
4. All error variants include actionable remediation instructions.
5. The singleton constraint prevents double-initialization and properly releases on failure or teardown.
6. Drop behavior is correct: finalizes if initialized, no-ops if not.
7. The component is discoverable via `IUnknown` interface query for `ISPDKEnv`.

## Implementation Notes

- The component is excluded from workspace `default-members` and must be built explicitly with `-p spdk-env` or `--workspace`.
- `DmaBuffer` and `SpdkEnvError` are defined in the `interfaces` crate (behind the `spdk` feature flag) and re-exported here for API ergonomics.
- The `enumerate_devices` callback returns non-zero (1) to enumerate-but-not-claim devices. This is critical: if the callback returned 0, SPDK would attach the device and it would be unavailable for `spdk_nvme_probe` in the block device component.
- `interfaces::set_spdk_env_active()` is called to coordinate with `DmaBuffer` drop handlers -- when the SPDK environment is inactive, DMA buffers must not call SPDK deallocators.
- Pre-flight check functions have internal `_at` variants that accept configurable paths, enabling unit testing with `tempfile` mock directories without requiring actual VFIO hardware.
- The `opts_size` field is explicitly set on `spdk_env_opts` because some SPDK/DPDK builds use it for version compatibility detection.
