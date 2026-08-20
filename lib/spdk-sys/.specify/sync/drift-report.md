# Drift Report — `spdk-sys`

Generated: 2026-08-07T15:31:01Z

Component: `components/spdk-sys`
Spec analyzed: `specs/001-spdk-sys/spec.md` (Backfilled)

## Summary

| Category | Aligned | Drifted | Not Implemented | Unspecced |
|----------|---------|---------|-----------------|-----------|
| FR (1-9) | 9 | 0 | 0 | — |
| NFR (1-5) | 5 | 0 | 0 | — |
| Success Criteria | 4 | 0 | 0 | — |

Result: **clean**. This is a backfilled spec that documents existing bindgen FFI; implementation and spec are fully consistent, including the "30 DPDK `rte_*` libraries" note (exact count verified).

## Detailed Findings

### Functional Requirements

| ID | Status | Evidence |
|----|--------|----------|
| FR-1 (env API) | Aligned | `spdk_env_opts_init`/`spdk_env_init`/`spdk_env_fini` allowlisted — `build.rs:165-167` |
| FR-2 (PCI enum + accessors) | Aligned | `spdk_pci_enumerate`, `spdk_pci_get_driver`, `spdk_pci_device_get_*` — `build.rs:168-182` |
| FR-3 (NVMe probe/attach/detach) | Aligned | `spdk_nvme_probe`, `spdk_nvme_detach` (attach via probe callback) — `build.rs:184-185` |
| FR-4 (controller ops) | Aligned | ns mgmt / qpair alloc / admin cmd functions — `build.rs:187-202` |
| FR-5 (NVMe I/O) | Aligned | `spdk_nvme_ns_cmd_{read,write,write_zeroes,flush}`, `spdk_nvme_qpair_process_completions` — `build.rs:210-214` |
| FR-6 (DMA alloc) | Aligned | `spdk_dma_zmalloc`/`spdk_dma_free`/`spdk_zmalloc`/`spdk_free` — `build.rs:216-219` |
| FR-7 (linker directives) | Aligned | SPDK/DPDK/system link libs — `build.rs:58-138` |
| FR-8 (conditional ISA-L) | Aligned | Detects `SPDK_CONFIG_ISAL` and links `isal` conditionally — `build.rs:40-42,123-125` |
| FR-9 (constants) | Aligned | `allowlist_var("SPDK_PCI_.*")`, `allowlist_var("SPDK_NVME_TRANSPORT_.*")` — `build.rs:239-240` |

### Non-Functional Requirements

| ID | Status | Evidence |
|----|--------|----------|
| NFR-1 (clear error if SPDK missing) | Aligned | `panic!` messages for missing src/build/config — `build.rs:13-39` |
| NFR-2 (regen on header/lib change) | Aligned | `rerun-if-changed=wrapper.h` + lib dir — `build.rs:256-260` |
| NFR-3 (layout tests disabled) | Aligned | `.layout_tests(false)` — `build.rs:246` |
| NFR-4 (whole-archive) | Aligned | `static:+whole-archive=` on all SPDK + DPDK libs — `build.rs:58-118` |
| NFR-5 (GCC internal include detection) | Aligned | `gcc -print-file-name=include` — `build.rs:142-153` |

### Success Criteria

| ID | Status | Evidence |
|----|--------|----------|
| Build succeeds w/ prebuilt SPDK | Aligned | build.rs resolves `deps/spdk-build/` |
| Sanity tests pass | Aligned | `tests/bindings_sanity.rs` (type sizes, field access, fn-ptr existence) |
| Higher-level crates consume bindings | Aligned | consumed by `spdk-env` (`src/env.rs`) and `interfaces/spdk_types.rs` |
| `links = "spdk"` prevents duplicate linkage | Aligned | `Cargo.toml:7` |

### Implementation Notes verification

- `links = "spdk"` — present (`Cargo.toml:7`). Aligned.
- `spdk_nvme_ctrlr_data` opaque — `build.rs:227-228`. Aligned.
- "30 DPDK `rte_*` libraries with +whole-archive" — `dpdk_libs` array has exactly 30 entries (`build.rs:83-114`). Aligned.
- `wrapper.h` includes `spdk/env.h`, `spdk/env_dpdk.h`, `spdk/nvme.h` — verified (`wrapper.h`). Aligned.

All Key Entities in the spec table are covered by `allowlist_type` calls (`build.rs:221-238`).

## Unspecced Code

None material. `build.rs` allowlists additional accessor functions (e.g. `spdk_pci_device_get_domain/bus/dev/func`, `spdk_pci_device_get_serial_number`) beyond those named in the spec examples, but all fall within FR-2's "PCI enumeration and device accessor functions" umbrella. No undocumented behavior.

## Recommendations

None. Spec and implementation are consistent. If the DPDK library list changes in future, keep the "30 libraries" note in the spec in sync with the `dpdk_libs` array.
