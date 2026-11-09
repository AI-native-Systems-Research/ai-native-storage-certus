# spdk-env

**Crate**: `spdk-env`
**Path**: `components/spdk-env/`
**Version**: 0.1.0

## Description

SPDK/DPDK userspace environment initialization component. Performs pre-flight checks (VFIO device availability, permissions, hugepages), calls `spdk_env_init`, enumerates VFIO-attached PCIe NVMe devices, and cleans up via `spdk_env_fini` on drop.

## Component Definition

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

## Interface Definition

```rust
define_interface! {
    pub ISPDKEnv {
        fn init(&self) -> Result<(), SpdkEnvError>;
        fn fini(&self);
        fn devices(&self) -> Vec<VfioDevice>;
        fn device_count(&self) -> usize;
        fn is_initialized(&self) -> bool;
    }
}
```

## Verified Properties

None. No formal verification model exists for this component.

## Receptacles

None.

## Key Types

- `PciAddress { domain, bus, dev, func }` — PCI BDF address
- `PciId { vendor_id, device_id, class_code }` — vendor/device/class IDs
- `VfioDevice { address: PciAddress, id: PciId, numa_node: i32 }` — discovered NVMe device
- `DmaBuffer` — DMA-safe hugepage buffer with pluggable allocator/deallocator
- `SpdkEnvError` — `InitFailed`, `NotInitialized`, `DeviceNotFound`, `HugepagesUnavailable`, `VfioError`
