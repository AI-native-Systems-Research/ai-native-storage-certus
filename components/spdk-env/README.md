# spdk-env

Safe Rust wrapper around SPDK environment initialization and VFIO device discovery. Implements the `ISPDKEnv` interface as a component in the Certus framework.

## Summary

`SPDKEnvComponent` manages the SPDK runtime lifecycle:

1. **Pre-flight checks** -- validates VFIO availability, permissions, and hugepage configuration
2. **Environment init** -- calls `spdk_env_init` (process-global singleton, enforced by `AtomicBool`)
3. **Device discovery** -- enumerates PCI devices bound to VFIO
4. **Cleanup** -- calls `spdk_env_fini` on `Drop`

Only one SPDK environment may exist per process. Attempting to initialize a second instance returns an error.

### ISPDKEnv Interface

| Method | Description |
|--------|-------------|
| `init()` | Initialize SPDK environment and discover devices |
| `devices()` | List discovered VFIO-bound PCI devices |
| `device_count()` | Number of discovered devices |
| `is_initialized()` | Whether the environment has been initialized |

### Pre-flight Checks

Before calling into SPDK, the component verifies:

- `/dev/vfio` exists and is accessible
- Current process has VFIO permissions
- Hugepages are configured in `/proc/meminfo`

## Structure

```
src/
  lib.rs       SPDKEnvComponent definition, ISPDKEnv implementation, Drop cleanup
  env.rs       do_init() orchestration: singleton guard, pre-flight, spdk_env_init, PCI enumeration
  checks.rs    Pre-flight validation (VFIO, permissions, hugepages)
  device.rs    PciAddress, PciId, VfioDevice types
  dma.rs       DmaBuffer wrapper for DMA-safe memory
  error.rs     SpdkEnvError type definitions
```

## Build & Test

### Prerequisites

- Linux host with IOMMU enabled and hugepages configured
- NVMe devices bound to VFIO (`deps/spdk/scripts/setup.sh`)
- SPDK built at `deps/spdk-build/` (run `deps/build_spdk.sh`)
- Rust stable toolchain (edition 2021, MSRV 1.75+)

### Build

```bash
cargo build -p spdk-env
```

This crate is excluded from the workspace `default-members` and must be built explicitly.

### Test

```bash
cargo test -p spdk-env
```
