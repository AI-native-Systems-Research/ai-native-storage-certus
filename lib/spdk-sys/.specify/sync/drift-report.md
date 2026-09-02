---
spec_sync_component: spdk-sys
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:47:19Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 31c8fc9d52a7ebb3da2b49ae6cd981a63a45713c0864f8908b9f547e53188fe8
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report: spdk-sys

**Generated**: 2026-09-02T21:47:19Z
**Component**: `lib/spdk-sys` (relocated from `components/spdk-sys`)
**Scope**: `specs/001-spdk-sys/{spec,plan,tasks}.md` vs implementation
`build.rs`, `src/lib.rs`, `wrapper.h`, `Cargo.toml`, `tests/bindings_sanity.rs`.
Interface context: `components/interfaces/src/`. READ-ONLY analysis (this run
also applied one confident BACKFILL — see apply-report.md).

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (FR + NFR + Success Criteria) | 18 |
| Aligned | 18 |
| Drifted | 1 (test-coverage gap, medium — open align-task) |
| Not Implemented | 0 |
| Unspecced Features | 0 (prior "abort/max-xfer" finding backfilled into FR-4 this run) |

The bindgen allowlist (50 functions, 17 types, 2 `allowlist_var` patterns) and
the SPDK/DPDK/system linker directives in `build.rs` match every FR/NFR with
concrete line evidence. `src/lib.rs` is a single `include!` of the generated
bindings, as the spec describes. The one substantive open item is a
test-coverage gap: the sanity suite exercises only env/PCI types, not the P1
NVMe FFI surface — tracked as an open align-task (code-side, out of scope for a
Markdown-only sync).

---

## Detailed Findings

### Spec 001-spdk-sys — spdk-sys FFI bindings

**Functional Requirements — all Aligned:**

- FR-1 (env API: `spdk_env_opts_init`, `spdk_env_init`, `spdk_env_fini`) ✓ `build.rs:165-167`
- FR-2 (PCI enumeration + accessors `spdk_pci_*`) ✓ `build.rs:168-182` (`spdk_pci_enumerate`, `spdk_pci_for_each_device`, `spdk_pci_get_driver`, plus 12 `spdk_pci_device_get_*` accessors)
- FR-3 (NVMe probe/attach/detach lifecycle) ✓ `build.rs:184-185` (`spdk_nvme_probe`, `spdk_nvme_detach`; attach is handled via the probe attach callback, not a separately allowlisted fn)
- FR-4 (controller ops: ns mgmt, qpair alloc, admin cmds, abort, max-xfer) ✓ `build.rs:187-205,197,220` — includes `get_num_ns`/`get_ns`/`alloc_io_qpair`/`free_io_qpair`/`process_admin_completions`/`get_default_ctrlr_opts`/`reset`/`get_data`/`cmd_admin_raw`/`create_ns`/`attach_ns`/`delete_ns`/`format`/`get_id`, plus `spdk_nvme_ctrlr_get_max_xfer_size` (`build.rs:197`) and `spdk_nvme_ctrlr_cmd_abort_ext` (`build.rs:220`), now explicitly named in FR-4 after this run's backfill.
- FR-5 (I/O ops: read/write/write_zeroes/flush/completion) ✓ `build.rs:213-221` (`ns_cmd_read`/`ns_cmd_write`/`ns_cmd_write_zeroes`/`ns_cmd_flush`/`qpair_process_completions`)
- FR-6 (DMA alloc: `spdk_dma_zmalloc`/`spdk_dma_free`/`spdk_zmalloc`/`spdk_free`) ✓ `build.rs:223-226`
- FR-7 (export SPDK/DPDK/system linker directives) ✓ `build.rs:45-138`
- FR-8 (conditional ISA-L linkage) ✓ `build.rs:40-42` (detect `SPDK_CONFIG_ISAL`), `build.rs:123-125` (link)
- FR-9 (constants `SPDK_PCI_*`, `SPDK_NVME_TRANSPORT_*` via `allowlist_var`) ✓ `build.rs:246-247`

**Non-Functional Requirements — all Aligned:**

- NFR-1 (clear error if SPDK source/build/config missing) ✓ `build.rs:13-19,22-28,33-39`
- NFR-2 (regenerate on `wrapper.h`/lib dir change) ✓ `build.rs:263-267`
- NFR-3 (layout tests disabled) ✓ `build.rs:253`
- NFR-4 (all SPDK libs `+whole-archive`) ✓ `build.rs:58-76` (SPDK) and `build.rs:116-118` (DPDK)
- NFR-5 (GCC internal include path detected) ✓ `build.rs:142-153,160-162`

**Success Criteria:**

- `links = "spdk"` metadata (dedup linkage) ✓ `Cargo.toml:8`
- Key Entities: all 17 listed types are on the allowlist ✓ `build.rs:228-245`; `spdk_nvme_ctrlr_data` marked opaque ✓ `build.rs:234-235`
- Implementation Notes verified: "30 DPDK `rte_*` libraries" ✓ (exactly 30 entries `build.rs:83-114`); `wrapper.h` includes `spdk/env.h`, `spdk/env_dpdk.h`, `spdk/nvme.h` ✓ `wrapper.h:2-4`; system libs `pthread, dl, numa, uuid, ssl, crypto, fuse3, m` ✓ `build.rs:128-138`
- Sanity tests present and pass (type sizes, field access, fn-pointer existence) ✓ `tests/bindings_sanity.rs` — **BUT coverage is env/PCI-only** (see Drifted Items #1)

---

## Drifted Items ⚠️

| # | Requirement | Spec vs Actual | Location | Severity |
|---|-------------|----------------|----------|----------|
| 1 | Success Criteria sanity-test coverage + FR-3/4/5 | Spec Success Criteria claims the sanity suite covers "type sizes, field access, function pointer existence"; the suite covers only `spdk_env_opts`, `spdk_pci_addr`, `spdk_pci_id`, `spdk_pci_device` and env/PCI fn-pointers. No P1 NVMe type (`spdk_nvme_ctrlr`/`_ns`/`_qpair`/`_cpl`) or NVMe fn-pointer is exercised. | `tests/bindings_sanity.rs:9-97` | medium |

This is a **code-side** gap (missing tests), not spec-text drift. Per the
AUTO-BACKFILL hard rules it is NOT resolved by weakening the Success Criteria
bullet — the criteria remain the target and the suite should be extended.
Routed to `align-tasks.md` (pre-existing task, still open). Also mirrors the
open item in `specs/001-spdk-sys/tasks.md:8`.

**Resolved this run (no longer drift):**
- Prior "stale `components/spdk-sys` path references" drift in
  `.specify/sync/align-tasks.md` — fixed to `lib/spdk-sys/...` this run
  (`align-tasks.md:11,14,29`). The regenerated `apply-report.md` uses correct
  `lib/spdk-sys` paths; the old apply-report content is superseded.

---

## Unspecced Features

None. The prior run's finding — `spdk_nvme_ctrlr_cmd_abort_ext` (`build.rs:220`)
and `spdk_nvme_ctrlr_get_max_xfer_size` (`build.rs:197`) allowlisted but not
named by any FR — was BACKFILLED into FR-4 this run (both are controller
operations already within FR-4's scope; the code comments document their use by
the block-device actor for in-flight abort and MDTS-based I/O fragmentation).

---

## Recommendations

1. **(Code / align-task, medium)** Extend `tests/bindings_sanity.rs` to cover
   the P1 NVMe FFI surface (type sizes/field access for `spdk_nvme_ctrlr`,
   `spdk_nvme_ns`, `spdk_nvme_qpair`, `spdk_nvme_cpl`; fn-pointer existence for
   `spdk_nvme_probe`, `spdk_nvme_detach`, `spdk_nvme_ctrlr_alloc_io_qpair`, the
   `ns_cmd_*` I/O fns, `spdk_nvme_ctrlr_cmd_abort_ext`). Then the Success
   Criteria claim is actually exercised. Tracked in `align-tasks.md`.
2. No source/build changes required for spec↔code alignment — the bindings and
   linker directives fully satisfy every FR/NFR.
