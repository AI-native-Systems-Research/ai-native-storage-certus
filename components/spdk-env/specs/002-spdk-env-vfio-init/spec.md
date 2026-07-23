# Feature Specification: SPDK/DPDK Environment Component with VFIO Device Iteration

**Feature Branch**: `002-spdk-env-vfio-init`  
**Created**: 2026-04-07  
**Status**: Draft  
**Input**: User description: "Build a component, as a lib-based crate, that initializes the SPDK and DPDK environments and iterates over available VFIO attached devices. The component should use the framework provided in ../component-framework. The component interface, ISPDKEnv, should provide methods for iterating over available devices. The component must verify the availability of VFIO and raise an error if the system is not configured correctly. The component should use the logging APIs provided by the framework. Add a test example main.rs that instantiates the component. The component should run without root permissions providing that /dev/vfio directories are user accessible. The component should check for permission and report an error as needed. This component is not an actor, but a plain procedural component."

## Clarifications

### Session 2026-04-07

- Q: Which device types should the component discover? → A: All SPDK-supported device types bound to VFIO (NVMe, virtio-blk, etc.)
- Q: Should the component enforce singleton semantics? → A: Enforce singleton — second instantiation returns an error
- Q: When should SPDK/DPDK initialization occur? → A: Explicit `init()` method on ISPDKEnv — caller constructs, wires receptacles, then calls `init()`
- Q: How should the component behave if the logging receptacle is not connected? → A: Fail `init()` with an error requiring the logger to be connected first
- Q: How should the component handle devices that are in use by another process? → A: Skip unavailable devices, log a warning for each, return only successfully probed devices

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Initialize SPDK Environment and Discover VFIO Devices (Priority: P1)

A developer instantiates the SPDKEnv component, wires the logging receptacle, and calls `init()` to initialize the SPDK/DPDK runtime and discover all VFIO-attached devices available on the system. The component performs system prerequisite checks (VFIO availability, permissions, hugepages), initializes the SPDK/DPDK environment, probes for all SPDK-supported device types (NVMe, virtio-blk, etc.), and exposes them through the ISPDKEnv interface.

**Why this priority**: This is the core purpose of the component. Without environment initialization and device discovery, no other functionality is possible.

**Independent Test**: Can be tested by instantiating the component on a system with VFIO-bound devices and verifying that device information is returned through the interface.

**Acceptance Scenarios**:

1. **Given** a system with VFIO enabled and at least one VFIO-bound device, **When** the component is constructed, logger wired, and `init()` called, **Then** it returns a list of discovered devices with identifying information (PCI address, device type).
2. **Given** a successful initialization, **When** the developer queries available devices through ISPDKEnv, **Then** each device entry includes sufficient information to identify the device (BDF address, vendor/device IDs) for all SPDK-supported device types.
3. **Given** a system with no VFIO-bound devices, **When** the component initializes successfully, **Then** it returns an empty device list without error.
4. **Given** a system where some VFIO-bound devices are in use by another SPDK process, **When** the component initializes, **Then** it skips unavailable devices with a logged warning and returns only successfully probed devices.

---

### User Story 2 - VFIO Availability and Permission Validation (Priority: P1)

A developer instantiates the component and calls `init()` on a system where VFIO may not be available, where /dev/vfio directories are not user-accessible, or where the logging receptacle has not been connected. The component detects these issues and reports clear, actionable error messages.

**Why this priority**: Without proper VFIO configuration and required receptacle wiring, the component cannot function. Early, clear error reporting prevents debugging confusion and is equally critical to core initialization.

**Independent Test**: Can be tested by running on a system without VFIO, with restricted /dev/vfio permissions, or without a connected logger, and verifying that specific, descriptive errors are raised.

**Acceptance Scenarios**:

1. **Given** a system where /dev/vfio does not exist or the vfio-pci kernel module is not loaded, **When** `init()` is called, **Then** it returns an error indicating VFIO is not available with guidance on how to enable it.
2. **Given** a system where /dev/vfio exists but the current user lacks read/write permissions, **When** `init()` is called, **Then** it returns an error indicating insufficient permissions with the specific path that is inaccessible.
3. **Given** a system where /dev/vfio/vfio (the VFIO container device) is not user-accessible, **When** `init()` is called, **Then** it reports the specific permission issue and does not proceed with initialization.
4. ~~(Removed: no logger receptacle exists in the implementation.)~~
5. **Given** an SPDKEnv instance already exists in the process, **When** a second instance calls `init()`, **Then** it returns an error indicating only one SPDK environment may be active per process.

---

### User Story 3 - Non-Root Operation (Priority: P2)

A developer runs the component as an unprivileged user. The component operates correctly without requiring root permissions, provided that /dev/vfio directories and device files have been configured with appropriate user access (e.g., via udev rules or group membership).

**Why this priority**: Running without root is important for security and usability in development and production environments, but depends on P1 stories functioning first.

**Independent Test**: Can be tested by running the example main.rs as a non-root user on a system with properly configured VFIO permissions and verifying successful device enumeration.

**Acceptance Scenarios**:

1. **Given** a non-root user with appropriate /dev/vfio permissions, **When** the component is initialized, **Then** it successfully enumerates all VFIO-bound devices.
2. **Given** a non-root user without appropriate permissions, **When** the component is initialized, **Then** it reports which specific files or directories lack permissions rather than a generic "access denied" error.

---

### User Story 4 - Component Framework Integration (Priority: P2)

A developer integrates the SPDKEnv component with other Certus components via the component framework. The component follows framework conventions: it is constructed via `define_component!`, exposes `ISPDKEnv` through `query_interface!`, and uses the framework's logging actor for all diagnostic output. The caller follows a construct-wire-init lifecycle: create the component, connect the logging receptacle, then call `init()`.

**Why this priority**: Framework integration is required for the component to be useful within the Certus system, but the core SPDK/VFIO functionality must work first.

**Independent Test**: Can be tested by writing a main.rs that constructs the component, wires the logger, calls `init()`, queries ISPDKEnv, and verifies that log messages appear through the framework's log handler.

**Acceptance Scenarios**:

1. **Given** the component is constructed using `define_component!` conventions, **When** a caller uses `query_interface!` for ISPDKEnv, **Then** it receives a valid interface reference.
2. **Given** `init()` is called, **When** the component performs initialization, **Then** diagnostic messages are emitted via `eprintln!` (no framework logging receptacle).
3. **Given** the component is a plain procedural (non-actor) component, **When** it is used, **Then** it does not spawn threads or use message queues for its core operation.

---

### Edge Cases

- What happens when VFIO is available but hugepages are not configured (required by DPDK)? — `init()` returns an error with a message about hugepage configuration.
- What happens when a device is bound to VFIO but is in use by another SPDK process? — The device is skipped with a logged warning; only successfully probed devices are returned.
- What happens when /dev/vfio exists but contains no IOMMU group directories? — Initialization succeeds; device list is empty.
- What happens when the SPDK/DPDK initialization fails mid-way (partial initialization cleanup)? — The component cleans up any partially initialized state and returns an error from `init()`.
- ~~(Removed: no logger receptacle exists — component uses eprintln! for diagnostics.)~~
- What happens if a second SPDKEnv instance is created in the same process? — `init()` returns an error; only one SPDK environment per process is allowed.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a Rust lib crate structured as a component using `define_component!` and `define_interface!` macros from the component-framework.
- **FR-002**: System MUST expose an `ISPDKEnv` interface with methods for querying available VFIO-attached devices and an explicit `init()` method for initialization.
- **FR-003**: System MUST initialize the SPDK and DPDK environments when `init()` is called, not during construction. The caller follows a construct-wire-init lifecycle.
- **FR-004**: System MUST verify the presence of /dev/vfio and the vfio-pci kernel module before attempting initialization.
- **FR-005**: System MUST check read/write permissions on /dev/vfio, /dev/vfio/vfio, and IOMMU group device files, and report specific permission errors identifying the inaccessible path.
- **FR-006**: System MUST enumerate NVMe devices bound to VFIO via `spdk_pci_enumerate` with the NVMe PCI driver after successful initialization, providing PCI BDF address, vendor/device IDs, class ID, NUMA node, and device type string for each. Devices are NOT attached during enumeration (callback returns non-zero), preserving them for later `spdk_nvme_probe`.
- **FR-007**: System uses `eprintln!` for diagnostic output (initialization progress, warnings). There is no logger receptacle; the component has no receptacles.
- **FR-008**: System MUST operate without root permissions when /dev/vfio directories have appropriate user-level access configured.
- **FR-009**: System MUST return an empty device list (not an error) when VFIO is properly configured but no devices are bound.
- **FR-010**: System MUST include a runnable example (`examples/spdk-env-example.rs`) that instantiates the component, calls `init()`, queries ISPDKEnv, and prints discovered devices. (No logger wiring is required — the component uses `eprintln!` for diagnostics per FR-007.)
- **FR-011**: System MUST be a plain procedural component (not an actor) that does not spawn its own threads or manage message queues.
- **FR-012**: System MUST properly clean up SPDK/DPDK resources when the component is dropped.
- **FR-013**: System MUST check for hugepage availability (required by DPDK) and report a clear error if hugepages are not configured.
- **FR-014**: System MUST enforce singleton semantics via a process-global `AtomicBool` — only one SPDK environment instance may be active per process. A second call to `init()` on a new instance MUST return an error. The flag is cleared on failure (allowing retry) and on Drop.
- **FR-015**: System MUST skip devices that cannot be probed (e.g., in use by another process), log a warning for each skipped device, and return only successfully probed devices. (Future: not yet implemented. Currently all matching devices are claimed; user must ensure exclusive access via system configuration.)
- **FR-016**: The `ISPDKEnv` interface MUST provide `is_initialized() -> bool` to check whether the environment has been successfully initialized.
- **FR-017**: The `ISPDKEnv` interface MUST provide `device_count() -> usize` to query the number of discovered devices without cloning the device vector.
- **FR-018**: The `ISPDKEnv` interface is defined locally in the
  `spdk-env` crate via `define_interface!` (at `src/lib.rs`). A
  mirror definition also exists in the shared `interfaces` crate
  (`interfaces/src/ispdk_env.rs`) for downstream consumers. The
  `spdk-env` crate does not import the `interfaces` version for its
  own implementation — it uses its local definition. Both definitions
  MUST be kept in sync manually.
- **FR-019**: The `ISPDKEnv` interface MUST provide an explicit `fini(&self)`
  method that tears down the SPDK/DPDK environment (calls `spdk_env_fini()`
  and clears the process-global singleton flag) without requiring the
  component to be dropped. Callers MUST detach all NVMe controllers and free
  all `DmaBuffer` instances before calling `fini()`, since DPDK's `atexit`
  handlers may otherwise access freed resources on process exit. `fini()` is
  idempotent with respect to the singleton flag: it is a no-op if the
  component is not currently initialized. Drop-based cleanup (FR-012) calls
  the same underlying teardown if `fini()` was not already called explicitly.
- **FR-020**: System MUST provide a `DmaBuffer` type (re-exported from the
  shared `interfaces` crate at `interfaces::DmaBuffer` via
  `spdk_env::dma::DmaBuffer`) representing a DMA-safe buffer for direct NVMe
  I/O. `DmaBuffer::new(size, align, numa_node)` allocates zero-initialized,
  hugepage-backed memory via SPDK (`spdk_dma_zmalloc`/`spdk_zmalloc`) and
  requires the SPDK environment to already be initialized. `unsafe fn
  DmaBuffer::from_raw(ptr, len, free_fn, numa_node)` wraps caller-supplied
  memory (e.g. GPU device memory) with a caller-supplied deallocation
  function, transferring ownership to the `DmaBuffer`. `DmaBuffer` implements
  `Deref`/`DerefMut` to `[u8]` for direct byte access and calls its stored
  `free_fn` on `Drop`. To prevent a `Drop` from calling into SPDK after the
  environment has been torn down (which would crash), a process-global
  "SPDK active" flag (`interfaces::set_spdk_env_active`/`is_spdk_env_active`)
  is set to `true` by `init()` (`init_spdk_env()`) and to `false` by `fini()`/
  `do_fini()` before `spdk_env_fini()` is called; `DmaBuffer::drop` checks this
  flag and skips calling `free_fn` if the environment is no longer active.
- **FR-021**: System MUST provide operator setup scripts under
  `components/spdk-env/scripts/` as sanctioned tooling for the external
  configuration referenced in the Assumptions section: `bind_vfio.sh`
  (interactive and scripted `status`/`bind <BDF>...`/`reset <BDF>...`/
  `bind-all`/`reset-all` workflow for detaching NVMe SSDs from their kernel
  driver and binding them to `vfio-pci`, or reversing that), `add_kernel_options.sh`
  (sets IOMMU and 1G-hugepage kernel boot parameters via `grubby`),
  `cfg_user_spdk.sh` (grants non-root read/write access to `/dev/vfio` and
  `/dev/hugepages`, tunes `net.core.rmem_max`), `show_spdk_devices.sh` (lists
  PCI devices currently bound to `vfio-pci`, table or quiet BDF-only output),
  and `fix_dnf_cache.sh` (diagnoses/repairs DNF cache issues that can block
  `deps/install_deps.sh`). These scripts are not part of the component's
  runtime code path; they are developer/operator convenience tooling for the
  external configuration steps FR-008/Assumptions already require.

### Key Entities

- **VfioDevice**: Represents a discovered VFIO-attached device. Key attributes: `address: PciAddress` (domain:bus:dev.func), `id: PciId` (class_id, vendor_id, device_id, subvendor_id, subdevice_id), `numa_node: i32` (-1 = unknown), `device_type: String` (e.g., "nvme").
- **ISPDKEnv**: The component interface providing device iteration, environment status queries, and explicit `init()`/`fini()` methods.
- **SPDKEnvComponent**: The concrete component implementing ISPDKEnv, managing SPDK/DPDK lifecycle with singleton enforcement.
- **DmaBuffer**: A DMA-safe, hugepage- or externally-backed byte buffer (see FR-020) used by downstream components (e.g. NVMe block device I/O) for zero-copy transfers; owns a deallocation function invoked on `Drop`, gated by the process-global SPDK-active flag.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: The component discovers 100% of available (not in-use) VFIO-bound devices on a properly configured system, matching the devices visible in /sys/bus/pci/drivers/vfio-pci minus those locked by other processes.
- **SC-002**: On a misconfigured system (no VFIO, wrong permissions, no hugepages, missing logger), the component reports the specific issue within its first error message, enabling the user to resolve the problem without additional debugging.
- **SC-003**: The example main.rs compiles and runs successfully as a non-root user on a system with correct VFIO permissions, printing device information to the console.
- **SC-004**: All component operations complete synchronously without spawning threads, confirming procedural (non-actor) behavior.
- **SC-005**: The component produces diagnostic output via `eprintln!` during initialization and device discovery (progress messages, permission/hugepage/VFIO check failures, enumeration warnings). Per FR-007/Clarifications, the component has no logger receptacle and does not route diagnostics through the component-framework logging actor; "structured log messages through the framework's logging system" in earlier drafts of this criterion was stale and has been corrected here to match FR-007 and the implementation (`eprintln!` in `src/env.rs`).

## Assumptions

- SPDK and DPDK libraries are pre-built and available at the paths configured by `../../deps/build_spdk.sh` (i.e., `../../deps/spdk-build/`).
- The target platform is Linux with IOMMU support (Intel VT-d or AMD-Vi).
- The host system uses a RHEL/Fedora-family distribution consistent with the existing `install_deps.sh` script.
- VFIO device binding (e.g., via `dpdk-devbind.py`, manual sysfs writes, or the sanctioned `scripts/bind_vfio.sh` helper — see FR-021) is performed externally before the component is used.
- Hugepage configuration is performed externally (e.g., via kernel boot parameters set through `scripts/add_kernel_options.sh`, or sysctl); non-root `/dev/vfio` and `/dev/hugepages` access can be granted via `scripts/cfg_user_spdk.sh`, and currently-bound devices can be listed via `scripts/show_spdk_devices.sh` (see FR-021).
- The component links against SPDK/DPDK C libraries via Rust FFI (bindgen or manual bindings).
- The component-framework crate is available as a workspace dependency.
- The component uses `eprintln!` for all diagnostic output. There is no logging receptacle; the component has no receptacles.
- SPDK environment initialization is process-global; the component enforces this via singleton semantics.
