# Data Model: SPDK/DPDK Environment Component

**Branch**: `002-spdk-env-vfio-init` | **Date**: 2026-04-07

## Entities

### PciAddress

Represents a PCI Bus-Device-Function address.

| Field    | Type   | Description                        |
|----------|--------|------------------------------------|
| domain   | u32    | PCI domain (segment)               |
| bus      | u8     | PCI bus number                     |
| dev      | u8     | PCI device number                  |
| func     | u8     | PCI function number                |

**Display format**: `DDDD:BB:DD.F` (e.g., `0000:01:00.0`)

**Uniqueness**: The tuple (domain, bus, dev, func) uniquely identifies a PCI device in the system.

### PciId

Identifies the type/model of a PCI device.

| Field        | Type   | Description                    |
|--------------|--------|--------------------------------|
| class_id     | u32    | PCI class code                 |
| vendor_id    | u16    | PCI vendor ID                  |
| device_id    | u16    | PCI device ID                  |
| subvendor_id | u16    | Subsystem vendor ID            |
| subdevice_id | u16    | Subsystem device ID            |

### VfioDevice

Represents a discovered VFIO-attached device managed by SPDK.

| Field       | Type       | Description                                        |
|-------------|------------|----------------------------------------------------|
| address     | PciAddress | PCI BDF address uniquely identifying the device     |
| id          | PciId      | Vendor/device/class identification                  |
| numa_node   | i32        | NUMA node the device is attached to (-1 = unknown) |
| device_type | String     | SPDK device type string (e.g., "nvme", "virtio")   |

**Identity**: A VfioDevice is uniquely identified by its `address`.

**Lifecycle**: VfioDevice instances are created during `init()` and are immutable snapshots. They do not track runtime state changes (device removal, etc.).

### SpdkEnvError

Error conditions reported by the component.

| Variant              | Description                                                |
|----------------------|------------------------------------------------------------|
| VfioNotAvailable     | /dev/vfio not found or vfio-pci module not loaded          |
| PermissionDenied     | Insufficient permissions on a specific VFIO path           |
| HugepagesNotConfigured | No hugepages available for DPDK                          |
| AlreadyInitialized   | Another SPDKEnv instance is active in this process         |
| InitFailed           | SPDK/DPDK environment initialization failed                |
| DeviceProbeFailed    | PCI device enumeration failed (after env init succeeded)   |
| DmaAllocationFailed  | `DmaBuffer` allocation failed (hugepage exhaustion or env not initialized) |

Each variant carries a descriptive `String` message with actionable guidance.
The component has no receptacles, so there is no `LoggerNotConnected` variant
(the logger receptacle originally scoped in the feature description was
removed — see FR-007/Clarifications; `spec.md` User Story 2 Acceptance
Scenario 4 and the "missing logger" edge case are marked removed accordingly).

### DmaBuffer

A DMA-safe buffer for direct NVMe I/O, defined in the shared `interfaces`
crate and re-exported at `spdk_env::dma::DmaBuffer` (see FR-020).

| Field       | Type                                          | Description                                              |
|-------------|------------------------------------------------|------------------------------------------------------------|
| ptr         | `*mut c_void`                                 | Raw pointer to the underlying memory                       |
| len         | `usize`                                       | Buffer length in bytes                                     |
| free_fn     | `unsafe extern "C" fn(*mut c_void)`           | Deallocator invoked on `Drop`                               |
| numa_node   | `i32`                                         | NUMA node of the allocation, or -1 if unknown               |
| metadata    | `BTreeMap<String, String>`                    | Optional key-value metadata (e.g. `"gpu_device" => "0"`)    |

**Identity**: Not identity-bearing; owned/moved like any other buffer value.

**Lifecycle**: Created via `DmaBuffer::new()` (SPDK hugepage memory) or the
`unsafe` `DmaBuffer::from_raw()` (externally-allocated memory with a
caller-supplied deallocator). On `Drop`, `free_fn` is invoked only if the
process-global "SPDK active" flag (`interfaces::is_spdk_env_active()`) is
still `true`; otherwise the free is skipped to avoid crashing after
`spdk_env_fini()`.

## Relationships

```
SPDKEnvComponent --provides--> ISPDKEnv
ISPDKEnv::devices() --returns--> Vec<VfioDevice>
VfioDevice --contains--> PciAddress
VfioDevice --contains--> PciId
ISPDKEnv::init() --may-return--> SpdkEnvError
ISPDKEnv::init() --sets--> "SPDK active" flag (interfaces::set_spdk_env_active(true))
ISPDKEnv::fini() --clears--> "SPDK active" flag (interfaces::set_spdk_env_active(false))
DmaBuffer::drop() --checks--> "SPDK active" flag
```

## State Transitions

### SPDKEnvComponent Lifecycle

```
Constructed --> [init()] --> Initialized --> [fini() or drop()] --> Finalized
     |                            |
     |--- [init() fails] ------->| ERROR: (various — VfioNotAvailable, PermissionDenied,
     |                           |         HugepagesNotConfigured, AlreadyInitialized, InitFailed)
     |--- [drop() before init()] --> (no-op cleanup)
```

- **Constructed**: Component created via `new()`. No SPDK state. (No receptacles to wire — see FR-007.)
- **Initialized**: SPDK/DPDK environment active. NVMe devices discovered (see spec.md SC-001 / align-tasks.md for the NVMe-only enumeration gap vs. "all VFIO device types"). Queries available. `interfaces::is_spdk_env_active()` returns `true`.
- **Finalized**: `spdk_env_fini()` called (via explicit `fini()` or Drop), `interfaces::set_spdk_env_active(false)` called first, global singleton flag cleared. A subsequent `init()` in the same process may re-enter `Initialized`.

### Singleton State (Process-Global)

```
Available --> [init() succeeds] --> Occupied --> [drop()] --> Available
                                       |
                                       |--- [second init()] --> ERROR: AlreadyInitialized
```
