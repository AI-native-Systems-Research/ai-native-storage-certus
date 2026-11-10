# Research: SPDK/DPDK Environment Component

**Branch**: `002-spdk-env-vfio-init` | **Date**: 2026-04-07

## R1: SPDK Environment Initialization API

**Decision**: Use `spdk_env_opts_init()` + `spdk_env_init()` for initialization, `spdk_env_fini()` for cleanup.

**Rationale**: These are the canonical SPDK C APIs for environment setup. `spdk_env_init()` is process-global and can only be called once — aligns with our singleton requirement. The `spdk_env_opts` struct (128 bytes) allows configuring core mask, hugepage settings, PCI allow/block lists, and IOVA mode.

**Alternatives considered**:
- `spdk_app_start()` — higher-level, bundles reactor framework. Rejected: we need env-only init for a procedural component.
- Direct DPDK `rte_eal_init()` — lower-level, bypasses SPDK's PCI management. Rejected: SPDK wraps DPDK and provides PCI enumeration APIs we need.

## R2: PCI Device Enumeration API

**Decision**: Use `spdk_pci_enumerate()` with `spdk_pci_get_driver("vfio")` for initial probe, then `spdk_pci_for_each_device()` for iteration.

**Rationale**: `spdk_pci_enumerate()` triggers DPDK's bus scan and attaches devices via the VFIO driver. `spdk_pci_for_each_device()` iterates all attached PCI devices, providing `spdk_pci_device*` from which we extract BDF address, vendor/device IDs, class, NUMA node, and type string. This covers all SPDK-supported device types (NVMe, virtio-blk, etc.) bound to VFIO.

**Alternatives considered**:
- `spdk_nvme_probe()` — NVMe-specific. Rejected: spec requires all SPDK-supported device types.
- Direct sysfs scanning of `/sys/bus/pci/drivers/vfio-pci/` — userspace only. Rejected: doesn't integrate with SPDK's device management.

## R3: Device Information Available Per Device

**Decision**: Extract from `spdk_pci_device`: BDF address (domain:bus:dev.func), vendor_id, device_id, subvendor_id, subdevice_id, class_id, NUMA node ID, device type string.

**Rationale**: These fields are directly accessible via `spdk_pci_device_get_*` accessor functions. The `type` field on `spdk_pci_device` is a string like `"nvme"`, `"virtio"` identifying the device class.

**Alternatives considered**: None — this is the complete set available from SPDK's PCI layer.

## R4: Crate Architecture — FFI Isolation

**Decision**: Create two crates: `spdk-sys` (raw FFI bindings) and `spdk-env` (safe Rust wrapper + component).

**Rationale**: Constitution Principle VI requires "FFI boundaries MUST be isolated in dedicated -sys crates with safe Rust wrappers in companion crates." The `-sys` crate uses `bindgen` to generate bindings from SPDK headers and handles linking. The `spdk-env` crate provides safe wrappers and the component framework integration.

**Alternatives considered**:
- Single crate with inline `extern "C"` — simpler but violates constitution.
- Manual FFI declarations — error-prone, hard to maintain. Bindgen is standard practice.

## R5: Logging Architecture

**Decision**: Use a receptacle for `ILogger` (defined in example-logger or a shared interface crate). The `init()` method checks `self.logger.is_connected()` and fails if not wired.

**Rationale**: The component framework uses receptacles for dependency injection. The helloworld app demonstrates logger wiring via actor handles, but the receptacle pattern is the component-framework-canonical approach for cross-component dependencies. Since `example-logger` is declared in the workspace but not yet implemented, spdk-env should depend on whatever `ILogger` interface it exposes (or a shared trait crate if one is created).

**Alternatives considered**:
- Constructor field `ActorHandle<LogMessage>` — works but bypasses the component framework's receptacle wiring mechanism.
- Define a private `ILog` trait in spdk-env — creates coupling inversion problem; other components can't provide a compatible logger.

## R6: VFIO and Hugepage Pre-checks

**Decision**: Before calling `spdk_env_init()`, perform userspace checks:
1. `/dev/vfio` directory exists
2. `/dev/vfio/vfio` (container device) exists and is readable/writable by current user
3. IOMMU group dirs under `/dev/vfio/` are accessible
4. `vfio-pci` kernel module is loaded (check `/sys/bus/pci/drivers/vfio-pci/`)
5. Hugepages available (check `/sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages` or `/proc/meminfo`)

**Rationale**: These checks provide actionable error messages before SPDK/DPDK init, which would otherwise produce opaque DPDK EAL errors. Checking from userspace avoids needing root for diagnostics.

**Alternatives considered**:
- Let SPDK/DPDK fail and parse error output — rejected: error messages are often unhelpful ("EAL: Cannot init hugepage info").
- Check only `/dev/vfio` existence — insufficient; permission issues are the most common problem for non-root users.

## R7: Singleton Enforcement

**Decision**: Use a process-global `AtomicBool` flag set during `init()` and cleared during `Drop`. Second `init()` call checks the flag and returns an error.

**Rationale**: SPDK `spdk_env_init()` is process-global. Using an atomic flag is the simplest correct enforcement mechanism. The flag is cleared in `Drop::drop()` which calls `spdk_env_fini()`, allowing a new instance after the previous one is fully cleaned up.

**Alternatives considered**:
- `Once`/`OnceLock` — doesn't allow re-initialization after cleanup.
- `Mutex<Option<...>>` global — heavier than needed; an atomic bool suffices.

## R8: Build System Integration

**Decision**: The `spdk-sys` crate uses a `build.rs` that:
1. Runs `bindgen` on SPDK headers from `deps/spdk-build/include/`
2. Emits `cargo:rustc-link-search` for `deps/spdk-build/lib/`
3. Links `spdk_env_dpdk`, `spdk_nvme`, and required DPDK libraries
4. Uses `pkg-config` via the `.pc` files in `deps/spdk-build/lib/pkgconfig/`

**Rationale**: The SPDK build already produces pkg-config files. Using `pkg-config` from `build.rs` is the standard approach for Rust FFI crates. Bindgen generates correct, up-to-date bindings from the installed headers.

**Alternatives considered**:
- Manual `extern "C"` blocks — fragile, doesn't track API changes.
- Vendored pre-generated bindings — stale quickly, doesn't match local SPDK build.
