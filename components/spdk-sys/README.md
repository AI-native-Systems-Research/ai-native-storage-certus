# spdk-sys

Raw unsafe FFI bindings to [SPDK](https://spdk.io/) (Storage Performance Development Kit) C libraries, generated at build time by [bindgen](https://github.com/rust-lang/rust-bindgen). Part of the Certus project.

## Summary

This crate provides auto-generated Rust bindings for a subset of the SPDK C API needed by the Certus storage system. All exported functions are `unsafe` C FFI calls. The bindings cover:

- **Environment**: `spdk_env_opts_init`, `spdk_env_init`, `spdk_env_fini`
- **PCI**: `spdk_pci_*` enumeration and device accessor functions
- **NVMe**: `spdk_nvme_probe`, `spdk_nvme_detach`, controller/namespace/qpair operations, IO submission and completion, namespace management
- **DMA**: `spdk_dma_zmalloc`, `spdk_dma_free`, `spdk_zmalloc`, `spdk_free`
- **Types**: `spdk_env_opts`, `spdk_pci_addr`, `spdk_pci_id`, `spdk_nvme_ctrlr`, `spdk_nvme_ns`, `spdk_nvme_qpair`, `spdk_nvme_transport_id`, and associated structs

## Structure

```
build.rs              Build script: locates SPDK, runs bindgen, emits linker flags
wrapper.h             C header includes for bindgen (spdk/env.h, spdk/nvme.h, etc.)
src/lib.rs            Includes the generated bindings via include! macro
tests/
  bindings_sanity.rs  Basic sanity checks on generated bindings
```

The build script (`build.rs`) performs:
1. Locates SPDK source at `deps/spdk/` and pre-built libraries at `deps/spdk-build/`
2. Runs bindgen with an allowlist of functions and types
3. Emits `rustc-link-lib` directives for SPDK, DPDK, and system libraries

### Linked Libraries

- **SPDK** (static, whole-archive): `spdk_env_dpdk`, `spdk_log`, `spdk_util`, `spdk_nvme`, `spdk_trace`, `spdk_dma`, `spdk_keyring`, `spdk_json`, `spdk_jsonrpc`, `spdk_rpc`, `spdk_sock`, `spdk_sock_posix`, `spdk_thread`
- **DPDK** (static, whole-archive): 28 `rte_*` libraries (EAL, mempool, ring, PCI, VFIO, etc.)
- **System** (dynamic): `pthread`, `dl`, `numa`, `uuid`, `ssl`, `crypto`, `m`, `fuse3`

## Build & Test

### Prerequisites

SPDK must be pre-built before this crate can compile:

```bash
# Install system dependencies (RHEL/Fedora)
deps/install_deps.sh

# Build SPDK to deps/spdk-build/
deps/build_spdk.sh
```

### Build

```bash
cargo build -p spdk-sys
```

This crate is excluded from the workspace `default-members` and must be built explicitly.

### Test

```bash
cargo test -p spdk-sys
```
