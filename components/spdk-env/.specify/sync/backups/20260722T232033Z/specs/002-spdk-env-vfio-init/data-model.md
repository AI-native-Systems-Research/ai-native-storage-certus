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
| LoggerNotConnected   | Logging receptacle not wired before init()                 |
| AlreadyInitialized   | Another SPDKEnv instance is active in this process         |
| InitFailed           | SPDK/DPDK environment initialization failed                |
| DeviceProbeFailed    | PCI device enumeration failed (after env init succeeded)   |

Each variant carries a descriptive `String` message with actionable guidance.

## Relationships

```
SPDKEnvComponent --provides--> ISPDKEnv
SPDKEnvComponent --receptacle--> ILogger
ISPDKEnv::devices() --returns--> Vec<VfioDevice>
VfioDevice --contains--> PciAddress
VfioDevice --contains--> PciId
ISPDKEnv::init() --may-return--> SpdkEnvError
```

## State Transitions

### SPDKEnvComponent Lifecycle

```
Constructed --> [logger.connect()] --> LoggerWired --> [init()] --> Initialized --> [drop()] --> Finalized
     |                                    |                            |
     |--- [init() without logger] ------->| ERROR: LoggerNotConnected  |
     |                                    |--- [init() fails] -------->| ERROR: (various)
     |--- [drop() before init()] -------->| (no-op cleanup)            |
```

- **Constructed**: Component created via `new()`. No SPDK state. Logger not connected.
- **LoggerWired**: Logger receptacle connected. Ready for initialization.
- **Initialized**: SPDK/DPDK environment active. Devices discovered. Queries available.
- **Finalized**: `spdk_env_fini()` called. Global singleton flag cleared.

### Singleton State (Process-Global)

```
Available --> [init() succeeds] --> Occupied --> [drop()] --> Available
                                       |
                                       |--- [second init()] --> ERROR: AlreadyInitialized
```
