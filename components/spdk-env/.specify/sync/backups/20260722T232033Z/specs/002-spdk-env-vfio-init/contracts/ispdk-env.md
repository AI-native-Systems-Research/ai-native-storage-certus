# Interface Contract: ISPDKEnv

**Branch**: `002-spdk-env-vfio-init` | **Date**: 2026-04-07

## Overview

`ISPDKEnv` is the public interface of the SPDK environment component, exposed via the component framework's `define_interface!` macro. Consumers obtain it through `query_interface!(component, ISPDKEnv)`.

## Interface Methods

### `fn init(&self) -> Result<(), SpdkEnvError>`

Initialize the SPDK/DPDK environment, perform pre-flight checks, and discover devices.

**Preconditions**:
- Logger receptacle MUST be connected (returns `SpdkEnvError::LoggerNotConnected` otherwise)
- No other SPDKEnv instance may be active in the process (returns `SpdkEnvError::AlreadyInitialized` otherwise)

**Postconditions**:
- SPDK/DPDK environment is fully initialized
- All available VFIO-bound devices have been probed
- Process-global singleton flag is set

**Errors**:
- `SpdkEnvError::LoggerNotConnected` — logger receptacle not wired
- `SpdkEnvError::AlreadyInitialized` — another instance active
- `SpdkEnvError::VfioNotAvailable` — /dev/vfio missing or vfio-pci module not loaded
- `SpdkEnvError::PermissionDenied` — insufficient access to VFIO paths (message includes specific path)
- `SpdkEnvError::HugepagesNotConfigured` — no hugepages available
- `SpdkEnvError::InitFailed` — SPDK env init returned non-zero

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
    receptacles: {
        logger: ILogger,
    },
    fields: {
        // Internal: managed via interior mutability
    },
}
```

## Usage Contract

```
1. let comp = SPDKEnvComponent::new();           // Construct
2. comp.logger.connect(logger_arc);               // Wire logger
3. let env = query_interface!(comp, ISPDKEnv);    // Get interface
4. env.init()?;                                    // Initialize (fallible)
5. let devices = env.devices();                    // Query devices
6. // ... use devices ...
7. drop(comp);                                     // Cleanup (calls spdk_env_fini)
```

## Thread Safety

All methods take `&self` and are safe to call from multiple threads. Internal state is protected by `RwLock`. The component is `Send + Sync` (enforced by `define_component!`).
