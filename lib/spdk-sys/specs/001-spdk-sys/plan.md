# Implementation Plan: spdk-sys

**Branch**: `001-spdk-sys` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.

## Summary

This crate uses `bindgen` to auto-generate Rust FFI bindings for a curated subset of the SPDK C API at build time. The build script handles SPDK discovery, library linking (SPDK + DPDK + system), and conditional ISA-L support. The runtime code is a single `include!` of the generated bindings file.

## Technical Context

- **Bindgen version**: 0.71
- **SPDK location**: `deps/spdk/` (source), `deps/spdk-build/` (pre-built libraries and headers)
- **Linking strategy**: All SPDK/DPDK libraries are linked statically with `+whole-archive` to satisfy C-level cross-references between SPDK subsystems. System libraries are linked dynamically.
- **Workspace integration**: Excluded from `default-members`; must be built explicitly with `-p spdk-sys`.

## Architecture

```
wrapper.h (C headers)
    |
    v
build.rs
    |-- Locate deps/spdk/ and deps/spdk-build/
    |-- Configure bindgen allowlist (functions + types)
    |-- Generate $OUT_DIR/bindings.rs
    |-- Emit cargo:rustc-link-lib directives
    v
src/lib.rs
    |-- include!(concat!(env!("OUT_DIR"), "/bindings.rs"))
    |-- #![allow(...)] for FFI naming conventions
    v
Consumers: spdk-env, block-device-spdk-nvme
```

### Key Design Decisions

1. **Allowlist approach**: Only the functions and types actually needed by Certus are generated. This keeps compile times reasonable and avoids pulling in the entire SPDK header tree.
2. **Whole-archive linking**: SPDK's internal C cross-references require all libraries to be present unconditionally, regardless of what Rust code references directly.
3. **Layout tests disabled**: SPDK NVMe spec headers use C bitfields that bindgen cannot always reproduce with correct sizes. The bindings remain usable; only compile-time size assertions would fail.
4. **Opaque ctrlr_data**: `spdk_nvme_ctrlr_data` is opaque because its bitfield layout is too complex for bindgen.

## Dependencies

| Crate / System | Role |
|----------------|------|
| `bindgen` 0.71 (build-dep) | C-to-Rust binding generation |
| SPDK pre-built (`deps/spdk-build/`) | Static libraries + headers |
| GCC (host) | Internal include path for clang compatibility |
| System: pthread, dl, numa, uuid, ssl, crypto, m, fuse3 | Dynamic runtime deps |

## Testing

- **Sanity tests** (`tests/bindings_sanity.rs`): Verify type sizes are non-zero, fields are accessible, and function pointers exist. These run without SPDK hardware.
- **Build verification**: `cargo build -p spdk-sys` confirms bindgen succeeds and all libraries resolve.
- **Integration**: Downstream crates (`spdk-env`, `block-device-spdk-nvme`) exercise the bindings against real or mocked SPDK environments.

## Future Considerations

- Expand the allowlist as new SPDK features are needed (e.g., blobstore, RDMA transport).
- Consider generating a `spdk-sys` version tag that encodes the SPDK commit hash for reproducibility.
- Investigate `bindgen` persistent caching to speed up incremental builds.
- Evaluate moving to `cbindgen`-style approach if Certus ever needs to expose Rust APIs back to SPDK C code.
