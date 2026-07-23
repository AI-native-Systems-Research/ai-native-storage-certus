# Spec-Drift Report: spdk-sys

**Generated**: 2026-07-22T21:28:01Z
**Spec analyzed**: `components/spdk-sys/specs/001-spdk-sys/spec.md` (status: Backfilled)
**Implementation**: `components/spdk-sys/src/lib.rs`, `build.rs`, `Cargo.toml`, `README.md`, `tests/bindings_sanity.rs`

## Summary

| Metric | Count |
|---|---|
| Specs analyzed | 1 |
| Requirements checked (FR + NFR) | 13 |
| Aligned | 13 |
| Drifted | 3 |
| Not implemented | 0 |
| Unspecced features | 2 |

All 8 functional requirements (FR-1..FR-8) and all 5 non-functional requirements (NFR-1..NFR-5) are implemented as described. Drift exists only in narrative/documentation-level claims (DPDK library count, an omitted system-lib dependency, and test-coverage claims), not in the requirements table itself.

## Per-Spec Findings — `001-spdk-sys`

### Aligned Requirements

| ID | Requirement | Evidence |
|---|---|---|
| FR-1 | Env API bindings (`spdk_env_opts_init`, `spdk_env_init`, `spdk_env_fini`) | `build.rs:165-167` allowlists all three |
| FR-2 | PCI enumeration/accessor bindings | `build.rs:169-182` allowlists `spdk_pci_enumerate`, `spdk_pci_for_each_device`, `spdk_pci_get_driver`, and 12 `spdk_pci_device_get_*` accessors |
| FR-3 | NVMe probe/attach/detach lifecycle | `build.rs:184-185` allowlists `spdk_nvme_probe`, `spdk_nvme_detach` (SPDK has no separate "attach" function — attach happens via a caller-supplied callback passed to `probe`, so this is correctly scoped) |
| FR-4 | NVMe controller ops (ns mgmt, qpair alloc, admin cmds) | `build.rs:187-202` allowlists `alloc_io_qpair`/`free_io_qpair`, `create_ns`/`attach_ns`/`delete_ns`/`format`, `cmd_admin_raw`, etc. |
| FR-5 | NVMe I/O ops (read/write/write_zeroes/flush/completions) | `build.rs:210-214` allowlists `spdk_nvme_ns_cmd_{read,write,write_zeroes,flush}` and `spdk_nvme_qpair_process_completions` |
| FR-6 | DMA allocation bindings | `build.rs:216-219` allowlists `spdk_dma_zmalloc`, `spdk_dma_free`, `spdk_zmalloc`, `spdk_free` |
| FR-7 | Linker directives for SPDK/DPDK/system libs | `build.rs:58-138` emits `cargo:rustc-link-lib` for 13 SPDK libs, 30 DPDK libs, and 7 system libs |
| FR-8 | Conditional ISA-L linking | `build.rs:40-42,123-125` reads `SPDK_CONFIG_ISAL` from `config.h` and conditionally links `isal` |
| NFR-1 | Clear build failure if SPDK missing | `build.rs:13-19,21-28,33-39` panics with actionable messages for missing source, missing build dir, missing config header |
| NFR-2 | Rebuild on `wrapper.h`/lib-dir change | `build.rs:256,257-260` emits `rerun-if-changed` for both |
| NFR-3 | Layout tests disabled | `build.rs:246` `.layout_tests(false)` |
| NFR-4 | All SPDK libs use `+whole-archive` | `build.rs:58-76,124` — every `spdk_*` and `isal` link uses `static:+whole-archive` |
| NFR-5 | GCC internal include path detected | `build.rs:142-153,160-162` runs `gcc -print-file-name=include` and adds it as a clang arg |

### Drifted Items

| Requirement / Section | Spec Text | Actual | Location | Severity |
|---|---|---|---|---|
| Implementation Notes | "28 DPDK `rte_*` libraries are linked statically with `+whole-archive`." (also repeated in README.md:33) | The `dpdk_libs` array contains **30** entries, not 28 | `components/spdk-sys/build.rs:83-114` (30 string literals); `components/spdk-sys/README.md:33` | Low |
| Dependencies table | Lists system libs as "pthread, dl, numa, uuid, ssl, crypto, fuse3" | Build also dynamically links `m` (libm), omitted from the spec's dependency table | `components/spdk-sys/build.rs:134` vs `components/spdk-sys/specs/001-spdk-sys/spec.md:93` | Low |
| Success Criteria | "`cargo test -p spdk-sys` passes all sanity tests (type sizes, field access, function pointer existence)" — implies coverage of the crate's bindings | `tests/bindings_sanity.rs` only exercises `spdk_env_opts`, `spdk_pci_addr`, `spdk_pci_id`, `spdk_pci_device` and their functions. None of the P1 NVMe types/functions (`spdk_nvme_ctrlr`, `spdk_nvme_ns`, `spdk_nvme_qpair`, `spdk_nvme_cpl`, `spdk_nvme_probe`, `spdk_nvme_ctrlr_alloc_io_qpair`, etc., from FR-3/4/5) have any sanity or function-pointer-existence test | `components/spdk-sys/tests/bindings_sanity.rs:1-97` | Medium — the still-unchecked backfill task "Validate that sanity tests cover all critical types used by downstream consumers" (`specs/001-spdk-sys/tasks.md:8`) confirms this is a known, open gap |

## Unspecced Code

| Feature | Location | Lines | Suggested Spec Addition |
|---|---|---|---|
| Additional FFI types allowlisted but absent from the spec's "Key Entities" table: `spdk_pci_driver`, `spdk_nvme_io_qpair_opts`, `spdk_nvme_cmd`, `spdk_nvme_format`, `spdk_nvme_ctrlr_list` | `components/spdk-sys/build.rs:225,234,235,237,238` | 5 lines | Add these 5 rows to the "Key Entities" table in `spec.md`, since they back FR-3/FR-4/FR-5 functionality already in scope |
| Constant/enum bindings via `allowlist_var` (`SPDK_PCI_*`, `SPDK_NVME_TRANSPORT_*`) — no FR describes exporting constants, only functions and types | `components/spdk-sys/build.rs:239-240` | 2 lines | Add an FR (e.g. "FR-9: Generate bindings for SPDK PCI and NVMe transport constants needed by callers") to cover this generated surface |

## Conflicts

None found — no spec statement was contradicted by another spec statement.

## Recommendations

1. Fix the "28 DPDK libraries" claim in both `spec.md` (Implementation Notes) and `README.md` — update to 30, or better, drop the exact count and just say "see `build.rs` for the current list" so it doesn't drift again as libraries are added/removed.
2. Add `m` (libm) to the Dependencies table in `spec.md`.
3. Either extend `tests/bindings_sanity.rs` to cover NVMe types/functions (`spdk_nvme_ctrlr`, `spdk_nvme_ns`, `spdk_nvme_qpair`, `spdk_nvme_cpl`, and at least the P1 NVMe function pointers) or narrow the Success Criteria bullet to explicitly scope "sanity tests" to env/PCI types only, so the spec stops overclaiming coverage. This lines up with the still-open backfill task in `tasks.md`.
4. Add the 5 missing types to "Key Entities" and add an FR for constant/var bindings (`SPDK_PCI_*`, `SPDK_NVME_TRANSPORT_*`) to close the unspecced-code gaps.
5. Once the above are addressed, flip the spec `Status` from "Backfilled" to "Reviewed" per the outstanding task in `tasks.md:10`.
