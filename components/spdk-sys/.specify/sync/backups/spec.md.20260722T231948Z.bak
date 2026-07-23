# Feature Specification: spdk-sys

**Feature Branch**: `001-spdk-sys`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice
> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `spdk-sys` crate provides raw, unsafe FFI bindings to the SPDK (Storage Performance Development Kit) C libraries, generated at build time by `bindgen`. It serves as the lowest-level Rust interface to SPDK, exposing environment initialization, PCI device enumeration, NVMe driver operations, and DMA memory allocation.

This crate is consumed by higher-level safe wrappers (`spdk-env`, `block-device-spdk-nvme`) and is excluded from the workspace default members since it requires a pre-built SPDK installation.

## User Scenarios & Testing

### User Story 1 - NVMe Driver Development (Priority: P1)

**As** a Certus storage component developer,
**I want** type-safe Rust declarations of SPDK C functions and types,
**So that** I can build safe abstractions over SPDK's NVMe driver without manually writing FFI declarations.

**Acceptance Criteria**:
- All allowlisted SPDK functions are callable from Rust (via `unsafe` blocks).
- Generated types have correct sizes and field accessibility (validated by sanity tests).
- The crate links all required SPDK, DPDK, and system libraries.

### User Story 2 - Build System Integration (Priority: P1)

**As** a developer building the Certus workspace,
**I want** the SPDK bindings to be generated automatically from the pre-built SPDK installation,
**So that** I do not need to maintain hand-written FFI declarations that drift from the C headers.

**Acceptance Criteria**:
- `build.rs` locates SPDK at `deps/spdk/` and `deps/spdk-build/`.
- Bindgen generates bindings from `wrapper.h` with the correct include paths.
- Clear error messages are emitted if SPDK is not found or not built.

## Requirements

### Functional Requirements

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-1 | Generate Rust bindings for SPDK environment API (`spdk_env_opts_init`, `spdk_env_init`, `spdk_env_fini`) | P1 |
| FR-2 | Generate bindings for PCI enumeration and device accessor functions (`spdk_pci_*`) | P1 |
| FR-3 | Generate bindings for NVMe probe/attach/detach lifecycle | P1 |
| FR-4 | Generate bindings for NVMe controller operations (namespace mgmt, qpair alloc, admin commands) | P1 |
| FR-5 | Generate bindings for NVMe I/O operations (read, write, write_zeroes, flush, completion processing) | P1 |
| FR-6 | Generate bindings for DMA memory allocation (`spdk_dma_zmalloc`, `spdk_dma_free`, `spdk_zmalloc`, `spdk_free`) | P1 |
| FR-7 | Export all necessary SPDK/DPDK/system linker directives | P1 |
| FR-8 | Detect and conditionally link Intel ISA-L based on SPDK build configuration | P2 |

### Non-Functional Requirements

| ID | Requirement | Priority |
|----|-------------|----------|
| NFR-1 | Build fails with clear error if SPDK source or build directory is missing | P1 |
| NFR-2 | Bindings are regenerated when `wrapper.h` or the SPDK lib directory changes | P1 |
| NFR-3 | Layout tests are disabled (SPDK uses C bitfields that bindgen cannot always reproduce) | P1 |
| NFR-4 | All SPDK libraries are linked with `+whole-archive` to satisfy C-level cross-references | P1 |
| NFR-5 | GCC internal include path is detected for clang/bindgen compatibility | P2 |

## Key Entities

| Entity | Description |
|--------|-------------|
| `spdk_env_opts` | SPDK environment configuration structure |
| `spdk_pci_addr` | PCI bus/device/function address |
| `spdk_pci_id` | PCI vendor/device/class identifiers |
| `spdk_pci_device` | Opaque PCI device handle |
| `spdk_nvme_ctrlr` | NVMe controller handle |
| `spdk_nvme_ns` | NVMe namespace handle |
| `spdk_nvme_qpair` | NVMe I/O queue pair |
| `spdk_nvme_transport_id` | NVMe transport identifier |
| `spdk_nvme_cpl` | NVMe completion entry |
| `spdk_nvme_ctrlr_opts` | Controller attachment options |
| `spdk_nvme_ctrlr_data` | Controller identify data (opaque) |
| `spdk_nvme_ns_data` | Namespace identify data |

## Dependencies

| Dependency | Type | Purpose |
|------------|------|---------|
| `bindgen` 0.71 | Build | Generates Rust FFI bindings from C headers |
| SPDK source (`deps/spdk/`) | External | Provides C header files |
| SPDK build (`deps/spdk-build/`) | External | Provides compiled static libraries and `config.h` |
| GCC | System | Provides internal include paths for bindgen/clang |
| System libs: pthread, dl, numa, uuid, ssl, crypto, fuse3 | System | Runtime dependencies of SPDK/DPDK |

## Success Criteria

- `cargo build -p spdk-sys` succeeds when SPDK is pre-built at `deps/spdk-build/`.
- `cargo test -p spdk-sys` passes all sanity tests (type sizes, field access, function pointer existence).
- Higher-level crates (`spdk-env`, `block-device-spdk-nvme`) can import and use the generated bindings.
- The `links = "spdk"` metadata prevents duplicate SPDK linkage in the dependency graph.

## Implementation Notes

- The crate uses `links = "spdk"` in `Cargo.toml` to ensure only one crate in the dependency graph links SPDK.
- `spdk_nvme_ctrlr_data` is marked as an opaque type because its C layout includes complex bitfields.
- 28 DPDK `rte_*` libraries are linked statically with `+whole-archive`.
- The `wrapper.h` file includes `spdk/env.h`, `spdk/env_dpdk.h`, and `spdk/nvme.h`.
