# spdk-env

## Summary

`spdk-env` is a safe Rust wrapper around SPDK/DPDK environment initialization and VFIO device discovery for the Certus system. It implements the `ISPDKEnv` interface as a component in the Certus COM-inspired component framework.

The component manages the full SPDK runtime lifecycle: pre-flight system validation, singleton-enforced environment initialization via `spdk_env_init`, PCI device enumeration through VFIO, and clean teardown on drop. Error messages are actionable, providing specific remediation steps (e.g., `modprobe vfio-pci`, hugepage allocation commands) rather than opaque DPDK EAL failures.

## Architecture

### Initialization Sequence

`SPDKEnvComponent::init()` executes the following ordered steps:

1. **Singleton guard** -- An `AtomicBool` (`compare_exchange`) ensures only one SPDK environment exists per process. A second call returns `SpdkEnvError::AlreadyInitialized`.
2. **VFIO availability** -- Checks that `/dev/vfio` exists and the `vfio-pci` kernel module is loaded (via `/sys/bus/pci/drivers/vfio-pci/`).
3. **VFIO permissions** -- Verifies read/write access to `/dev/vfio/vfio` (container device) and all numeric IOMMU group entries under `/dev/vfio/`.
4. **Hugepages** -- Confirms that either 2MB or 1GB hugepages are allocated by reading sysfs counters.
5. **SPDK env init** -- Calls `spdk_env_init()` via FFI with default options (app name `certus-spdk-env`, shared memory ID -1, no core mask restriction).
6. **PCI enumeration** -- Uses `spdk_pci_enumerate` with the NVMe driver to discover devices. The callback returns non-zero so devices are enumerated but not claimed, leaving them available for later `spdk_nvme_probe` in the block device component.
7. **Cleanup on drop** -- `spdk_env_fini()` is called and the singleton flag is released.

### ISPDKEnv Interface

| Method | Description |
|--------|-------------|
| `init()` | Run pre-flight checks, initialize SPDK, enumerate devices |
| `devices()` | Return a clone of all discovered `VfioDevice` entries |
| `device_count()` | Number of discovered devices |
| `is_initialized()` | Whether the environment has been successfully initialized |

### Key Types

- `VfioDevice` -- Immutable snapshot of a discovered PCI device (address, vendor/device IDs, NUMA node, device type string).
- `PciAddress` -- Domain:Bus:Device.Function identifier, displayed as `DDDD:BB:DD.F`.
- `PciId` -- PCI class, vendor, device, and subsystem identifiers.
- `DmaBuffer` -- DMA-safe buffer (definition lives in the `interfaces` crate; re-exported here).
- `SpdkEnvError` -- Enum covering all failure modes (re-exported from `interfaces`).

### Module Layout

```
src/
  lib.rs       Component definition (SPDKEnvComponent), ISPDKEnv impl, Drop
  env.rs       Initialization orchestration: singleton, FFI calls, PCI enumeration
  checks.rs    Pre-flight validation (VFIO, permissions, hugepages)
  device.rs    PciAddress, PciId, VfioDevice types
  dma.rs       DmaBuffer re-export
  error.rs     SpdkEnvError re-export
```

## Build

### Prerequisites

- Linux host with IOMMU enabled and hugepages configured
- NVMe devices bound to VFIO (via `deps/spdk/scripts/setup.sh`)
- SPDK built and installed at `deps/spdk-build/` (run `deps/build_spdk.sh`)
- Rust stable toolchain (edition 2021, MSRV 1.75)

### Compile

```bash
cargo build -p spdk-env
```

This crate depends on `spdk-sys` (raw FFI bindings) and is excluded from the workspace `default-members`. It must be built explicitly with `-p spdk-env` or via `cargo build --workspace`.

## Test

```bash
cargo test -p spdk-env
```

Unit tests for pre-flight checks (`checks.rs`) and device types (`device.rs`) run without hardware by using `tempfile`-based mock paths. Tests that require a live SPDK environment (actual `init()` calls) require VFIO-bound devices and hugepages.

## Benchmarks

No Criterion benchmarks are currently defined for this crate. The component's critical path is one-shot initialization rather than a hot loop, so benchmarking has not been prioritized.
