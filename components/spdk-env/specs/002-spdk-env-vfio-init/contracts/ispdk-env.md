# Interface Contract: ISPDKEnv

**Branch**: `002-spdk-env-vfio-init` | **Date**: 2026-04-07

## Overview

`ISPDKEnv` is the public interface of the SPDK environment component, exposed via the component framework's `define_interface!` macro. Consumers obtain it through `query_interface!(component, ISPDKEnv)`.

## Interface Methods

### `fn init(&self) -> Result<(), SpdkEnvError>`

Initialize the SPDK/DPDK environment, perform pre-flight checks, and discover devices.

**Preconditions**:
- No other SPDKEnv instance may be active in the process (returns `SpdkEnvError::AlreadyInitialized` otherwise)
- (There is no logger receptacle to wire; the component has no receptacles — see FR-007.)

**Postconditions**:
- SPDK/DPDK environment is fully initialized
- All available VFIO-bound devices have been probed (NVMe-class only — see spec.md SC-001/align-tasks.md for the "all VFIO device types" gap)
- Process-global singleton flag is set

**Errors**:
- `SpdkEnvError::AlreadyInitialized` — another instance active
- `SpdkEnvError::VfioNotAvailable` — /dev/vfio missing or vfio-pci module not loaded
- `SpdkEnvError::PermissionDenied` — insufficient access to VFIO paths (message includes specific path)
- `SpdkEnvError::HugepagesNotConfigured` — no hugepages available
- `SpdkEnvError::InitFailed` — SPDK env init returned non-zero

### `fn fini(&self)`

Explicitly tear down the SPDK/DPDK environment: calls `spdk_env_fini()` and
clears the process-global singleton flag. No-op if not currently initialized.

**Preconditions**:
- All NVMe controllers detached and all `DmaBuffer` instances freed (calling
  `fini()` before this may cause DPDK `atexit` handlers to touch freed
  resources on process exit)

**Postconditions**:
- SPDK/DPDK environment is finalized; singleton flag cleared and available for
  a subsequent `init()` in the same process
- `interfaces::set_spdk_env_active(false)` has been called, so any remaining
  `DmaBuffer` drops will skip calling their SPDK deallocator

### `fn devices(&self) -> Vec<VfioDevice>`

Return all successfully probed VFIO-attached devices.

**Preconditions**: `init()` must have been called successfully (returns empty vec if not initialized).

**Postconditions**: Returns a snapshot of devices discovered during `init()`. The list is immutable after initialization.

### `fn device_count(&self) -> usize`

Return the number of discovered devices.

**Preconditions**: None (returns 0 if not initialized).

### `fn is_initialized(&self) -> bool`

Check whether the SPDK environment has been successfully initialized.

## Component Declaration

```
SPDKEnvComponent {
    version: "0.1.0",
    provides: [ISPDKEnv],
    fields: {
        discovered_devices: RwLock<Vec<VfioDevice>>,
        initialized: AtomicBool,
    },
}
```

The component has no receptacles (per FR-007/Clarifications, the logger
receptacle originally scoped in the feature description was removed).

## Usage Contract

```
1. let comp = SPDKEnvComponent::new(...);          // Construct
2. let env = query_interface!(comp, ISPDKEnv);     // Get interface
3. env.init()?;                                     // Initialize (fallible)
4. let devices = env.devices();                     // Query devices
5. // ... use devices, allocate/free DmaBuffer instances ...
6. env.fini();                                       // Explicit teardown (optional)
7. drop(comp);                                       // Cleanup (calls spdk_env_fini if fini() wasn't already called)
```

## Thread Safety

All methods take `&self` and are safe to call from multiple threads. Internal state is protected by `RwLock`. The component is `Send + Sync` (enforced by `define_component!`).
