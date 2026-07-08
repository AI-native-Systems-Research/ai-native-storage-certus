# Implementation Plan: SPDK Environment Initialization

**Branch**: `001-spdk-env` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The `spdk-env` component provides a safe Rust wrapper around SPDK/DPDK environment initialization and VFIO device discovery. It translates opaque DPDK EAL failures into actionable error messages by performing ordered pre-flight checks (VFIO availability, file permissions, hugepage allocation) before calling into SPDK. The component enforces a process-global singleton constraint via `AtomicBool`, enumerates NVMe PCI devices without claiming them, and provides automatic teardown on Drop.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-framework` (workspace) -- `define_component!`, `define_interface!`, `IUnknown` trait system
- `component-core` (workspace) -- Core component traits and channel infrastructure
- `interfaces` (workspace, feature `spdk`) -- `DmaBuffer`, `SpdkEnvError`, `VfioDevice`, `PciAddress`, `PciId`, `set_spdk_env_active()`
- `spdk-sys` (workspace) -- Raw FFI bindings: `spdk_env_init`, `spdk_env_fini`, `spdk_pci_enumerate`, `spdk_pci_get_driver`, `spdk_dma_zmalloc`, `spdk_dma_free`
- `libc` 0.2 -- `getuid()`, `getegid()` for permission checks
- `tempfile` 3 (dev) -- Temporary directories for unit tests

## Architecture

### Component Layer

```
+----------------------------------------------------+
|                  Certus Application                 |
+----------------------------------------------------+
         |  query_interface!(ISPDKEnv)
         v
+----------------------------------------------------+
|           SPDKEnvComponent (spdk-env)               |
|                                                    |
|  +----------------------------------------------+  |
|  | ISPDKEnv trait impl                          |  |
|  |  init() -> do_init() -> do_init_inner()      |  |
|  |  fini() -> do_fini()                         |  |
|  |  devices() -> RwLock<Vec<VfioDevice>>.clone() |  |
|  |  device_count(), is_initialized()            |  |
|  +----------------------------------------------+  |
|                       |                            |
|  +------------------+ | +------------------------+ |
|  | Pre-flight       | | | SPDK FFI               | |
|  | checks (checks/) | | | (env.rs)               | |
|  |  - VFIO avail    | | |  - spdk_env_init       | |
|  |  - VFIO perms    | | |  - spdk_env_fini       | |
|  |  - Hugepages     | | |  - spdk_pci_enumerate  | |
|  +------------------+ | +------------------------+ |
+----------------------------------------------------+
         |                        |
         v                        v
+------------------+    +-------------------+
| interfaces crate |    | spdk-sys crate    |
| (spdk feature)   |    | (raw C bindings)  |
| DmaBuffer,       |    | spdk_env_opts,    |
| SpdkEnvError,    |    | spdk_pci_device,  |
| VfioDevice, ...  |    | spdk_pci_addr,    |
| set_spdk_env_    |    | spdk_pci_id       |
|   active()       |    +-------------------+
+------------------+
```

### Internal Module Structure

```
components/spdk-env/
  Cargo.toml              -- Package manifest (excluded from default-members)
  CLAUDE.md               -- Component-specific AI guidance
  examples/
    spdk-env-example.rs   -- Full lifecycle demo (create, init, enumerate, drop)
  src/
    lib.rs                -- Public API: define_interface!(ISPDKEnv),
                             define_component!(SPDKEnvComponent),
                             ISPDKEnv impl, Drop impl, unit tests
    env.rs                -- Singleton guard (SPDK_ENV_ACTIVE: AtomicBool),
                             do_init() / do_init_inner() / init_spdk_env() /
                             enumerate_devices() / do_fini()
    checks.rs             -- Pre-flight validation:
                             check_vfio_available[_at](),
                             check_vfio_permissions[_at](),
                             check_hugepages[_at](),
                             helpers: check_path_readable(), check_path_rw(),
                             read_hugepage_count()
    device.rs             -- PciAddress (Display, Copy, Eq, Hash),
                             PciId (Copy, Eq), VfioDevice (Clone)
    dma.rs                -- Re-export: pub use interfaces::DmaBuffer
    error.rs              -- Re-export: pub use interfaces::SpdkEnvError
  .specify/
    specs/001-spdk-env/
      spec.md             -- Feature specification (backfilled)
      plan.md             -- This file
      tasks.md            -- Task list
```

### Data Flow

```
Application calls ISPDKEnv::init()
         |
         v
[1] Singleton Guard (AtomicBool::compare_exchange AcqRel/Acquire)
    |-- Err: return AlreadyInitialized
    |-- Ok: proceed
         |
         v
[2] Pre-flight Checks (checks.rs)
    |-- check_vfio_available_at("/dev/vfio", "/sys/bus/pci/drivers/vfio-pci")
    |-- check_vfio_permissions_at("/dev/vfio")
    |     |-- check_path_readable(/dev/vfio)
    |     |-- check_path_rw(/dev/vfio/vfio)
    |     |-- for each numeric entry: check_path_rw(/dev/vfio/<N>)
    |-- check_hugepages_at(2MB_sysfs_path, 1GB_sysfs_path)
    |     |-- read_hugepage_count() for each size
    |-- On ANY failure: release singleton flag, return error
         |
         v
[3] SPDK Environment Init (env.rs::init_spdk_env)
    |-- spdk_env_opts_init(&mut opts)
    |-- Set opts.name = "certus-spdk-env", opts.shm_id = -1
    |-- Set opts.opts_size for version compat
    |-- spdk_env_init(&opts) -> rc
    |     |-- rc != 0: return InitFailed
    |-- interfaces::set_spdk_env_active(true)
         |
         v
[4] Device Enumeration (env.rs::enumerate_devices)
    |-- spdk_pci_get_driver("nvme") -> driver ptr
    |-- spdk_pci_enumerate(driver, enum_cb, &mut devices)
    |     |-- enum_cb: read addr/id/numa/type from spdk_pci_device
    |     |-- Push VfioDevice into Vec
    |     |-- Return 1 (do NOT claim device)
    |-- Store Vec<VfioDevice> into RwLock
    |-- Set initialized = true (Release ordering)
         |
         v
[5] Teardown (on Drop or explicit fini())
    |-- interfaces::set_spdk_env_active(false)
    |-- spdk_env_fini()
    |-- SPDK_ENV_ACTIVE.store(false, Release)
```

### Key Design Decisions

1. **Singleton via AtomicBool**: DPDK maintains process-global state that cannot be safely initialized twice. The `SPDK_ENV_ACTIVE` static uses `compare_exchange(AcqRel/Acquire)` to guarantee exactly-once initialization across all threads.

2. **Enumerate-but-not-claim**: The `spdk_pci_enumerate` callback returns 1 (non-zero) so devices are discovered but not attached. This leaves them available for `spdk_nvme_probe` in the downstream `block-device-spdk-nvme` component.

3. **Pre-flight with `_at` variants**: Each check function has an internal variant accepting path parameters, enabling unit testing with `tempfile` mock directories without requiring actual VFIO hardware or root privileges.

4. **Error types in `interfaces` crate**: `SpdkEnvError`, `DmaBuffer`, `VfioDevice`, `PciAddress`, `PciId` are defined in the shared `interfaces` crate (behind `feature = "spdk"`) so downstream components can use these types without depending on `spdk-env` directly.

5. **DmaBuffer lifecycle coordination**: `interfaces::set_spdk_env_active(true/false)` signals to `DmaBuffer::drop()` whether it is safe to call SPDK deallocators. This prevents use-after-fini crashes during process teardown.

6. **Drop-based teardown**: The `Drop` impl on `SPDKEnvComponent` calls `do_fini()` if still initialized, ensuring SPDK resources are released even if the user forgets to call `fini()` explicitly.

7. **Explicit `opts_size`**: Set on `spdk_env_opts` because some SPDK/DPDK builds use this field for ABI version detection and will reject an options struct that has size 0.

## Dependencies

| Crate | Version | Role |
|-------|---------|------|
| `component-framework` | workspace | Proc macros (`define_component!`, `define_interface!`), `IUnknown` |
| `component-core` | workspace | Core component traits |
| `interfaces` | workspace (feature: `spdk`) | Shared types: `DmaBuffer`, `SpdkEnvError`, `VfioDevice`, `PciAddress`, `PciId`, `set_spdk_env_active()` |
| `spdk-sys` | workspace | Raw SPDK/DPDK FFI bindings (bindgen-generated) |
| `libc` | 0.2 | POSIX: `getuid()`, `getegid()` |
| `tempfile` | 3 (dev-only) | Unit test temp directories |

### System Prerequisites

- Linux kernel with IOMMU enabled (`intel_iommu=on` or `amd_iommu=on`)
- Hugepages allocated (2MB or 1GB pools)
- NVMe devices bound to `vfio-pci` driver
- SPDK built at `deps/spdk-build/`
- User read+write access to `/dev/vfio/` devices
- `memlock` ulimit set to unlimited

## Testing

### Unit Tests (no hardware required)

| Module | Test Coverage |
|--------|--------------|
| `src/lib.rs` | Component construction, version query, `IUnknown` interface discovery, `devices()` clone behavior, `device_count()` consistency, Drop safety |
| `src/checks.rs` | VFIO availability (missing dir, missing driver, both present), VFIO permissions (accessible, no-container, numeric groups, non-numeric skipped, no-access variants, uid/gid in errors, udev hint), hugepages (zero, 2MB, 1GB, both, missing files, non-numeric, empty, whitespace, allocation hint), `check_path_rw` (owner rw, world rw, nonexistent) |
| `src/device.rs` | `PciAddress` display/equality/hash/copy/debug, `PciId` equality/copy/debug, `VfioDevice` clone/debug/types |

### Integration Tests (require configured hardware)

- Full `init()` -> `devices()` -> `fini()` lifecycle via the example (`examples/spdk-env-example.rs`)
- Singleton enforcement: second `init()` returns `AlreadyInitialized`
- Teardown and re-initialization: after `fini()`, a new `init()` succeeds

### Test Execution

```bash
cargo test -p spdk-env                    # Unit tests (no hardware)
cargo run -p spdk-env --example spdk-env-example  # Integration (requires VFIO)
```

## Future Considerations

1. **Hot-plug support**: Currently devices are enumerated once at init. Future versions could re-enumerate on demand or via event callbacks.
2. **Multiple driver support**: Only the NVMe PCI driver is enumerated today. Could extend to virtio, vfio-user, or custom drivers.
3. **Configuration options**: Expose SPDK env options (core mask, memory channels, hugepage size preference) via the interface or a config receptacle.
4. **Async init**: The init sequence is synchronous and blocking. Consider an async variant for non-blocking startup in actor-based systems.
5. **Better error recovery**: If `spdk_env_init` fails, currently there is no retry logic. Could add backoff or alternative initialization paths.
6. **Formal verification**: The singleton guard and memory ordering could be verified with Creusot/Loom to prove absence of data races.
