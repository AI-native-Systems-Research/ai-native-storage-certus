# Drift Report: spdk-sys

**Generated**: pending
**Component**: `lib/spdk-sys` (relocated from `components/spdk-sys`)
**Scope**: `specs/001-spdk-sys/spec.md` (+ plan/tasks skim) vs implementation
`build.rs`, `src/lib.rs`, `tests/bindings_sanity.rs`. READ-ONLY analysis.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (FR + NFR + Success Criteria) | 18 |
| Aligned | 18 |
| Drifted | 2 (doc-only, stale path refs) |
| Not Implemented | 0 |
| Unspecced Features | 1 |

The bindgen allowlist and linker directives in `build.rs` match every FR/NFR.
The only drift is stale `components/spdk-sys` path references in prior
spec-sync artifact files left from the crate relocation to `lib/`.

---

## Detailed Findings

### Spec 001-spdk-sys — spdk-sys FFI bindings

**Functional Requirements — all Aligned:**

- FR-1 (env API: `spdk_env_opts_init`, `spdk_env_init`, `spdk_env_fini`) ✓ `build.rs:165-167`
- FR-2 (PCI enumeration + accessors `spdk_pci_*`) ✓ `build.rs:168-182`
- FR-3 (NVMe probe/attach/detach lifecycle) ✓ `build.rs:184-185` (`spdk_nvme_probe`, `spdk_nvme_detach`; attach is handled via the probe attach callback, not a separate allowlisted fn)
- FR-4 (controller ops: ns mgmt, qpair alloc, admin cmds) ✓ `build.rs:187-205` (get_num_ns/get_ns/alloc_io_qpair/free_io_qpair/process_admin_completions/get_default_ctrlr_opts/reset/get_data/cmd_admin_raw/create_ns/attach_ns/delete_ns/format/get_id)
- FR-5 (I/O ops: read/write/write_zeroes/flush/completion) ✓ `build.rs:213-221`
- FR-6 (DMA alloc: `spdk_dma_zmalloc`/`spdk_dma_free`/`spdk_zmalloc`/`spdk_free`) ✓ `build.rs:223-226`
- FR-7 (export SPDK/DPDK/system linker directives) ✓ `build.rs:45-138`
- FR-8 (conditional ISA-L linkage) ✓ `build.rs:40-42,123-125`
- FR-9 (constants `SPDK_PCI_*`, `SPDK_NVME_TRANSPORT_*` via `allowlist_var`) ✓ `build.rs:246-247`

**Non-Functional Requirements — all Aligned:**

- NFR-1 (clear error if SPDK source/build missing) ✓ `build.rs:13-19,22-28,33-39`
- NFR-2 (regenerate on `wrapper.h`/lib dir change) ✓ `build.rs:263-267`
- NFR-3 (layout tests disabled) ✓ `build.rs:253`
- NFR-4 (all SPDK libs `+whole-archive`) ✓ `build.rs:58-76`
- NFR-5 (GCC internal include path detected) ✓ `build.rs:142-153`

**Success Criteria — Aligned:**

- Sanity tests present (type sizes, field access, fn-pointer existence) ✓ `tests/bindings_sanity.rs`
- `links = "spdk"` metadata (dedup linkage) ✓ per spec Implementation Notes / Cargo.toml
- Key Entities: all listed types are on the allowlist incl. `spdk_nvme_ctrlr_list` ✓ `build.rs:228-245`; `spdk_nvme_ctrlr_data` marked opaque ✓ `build.rs:235`

---

## Drifted Items ⚠️

| # | Requirement | Spec vs Actual | Location | Severity |
|---|-------------|----------------|----------|----------|
| 1 | Post-relocation path references | Doc references old `components/spdk-sys/specs/...` and `.specify/sync/...` paths after crate moved to `lib/spdk-sys/` | `.specify/sync/align-tasks.md:11,14,29` | minor |
| 2 | Post-relocation path references | Apply-report references old `components/spdk-sys/...` paths (drift report path, spec path, scope, backups) | `.specify/sync/apply-report.md:4-7,11` | minor |

The prior `.specify/sync/drift-report.md:5` also carried `components/spdk-sys`,
but that file is overwritten by this report. No spec.md, plan.md, tasks.md,
README.md, or `Cargo.toml` retains the old path. Severity minor — historical
commands/paths, no active build breakage.

---

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `spdk_nvme_ctrlr_cmd_abort_ext` and `spdk_nvme_ctrlr_get_max_xfer_size` allowlisted | `build.rs` | 197, 220 | Plausibly covered by FR-4 ("admin commands" / controller ops), but not named. Consider adding an explicit FR line noting abort-in-flight and MDTS-derived max-transfer bindings used by the block-device actor. |

---

## Recommendations

1. Update the stale `components/spdk-sys` references in
   `.specify/sync/align-tasks.md` (lines 11, 14, 29) and
   `.specify/sync/apply-report.md` (lines 4-7, 11) to `lib/spdk-sys/...`, or
   annotate as historical. Minor.
2. Optionally extend FR-4 to explicitly name the abort (`spdk_nvme_ctrlr_cmd_abort_ext`)
   and max-transfer-size (`spdk_nvme_ctrlr_get_max_xfer_size`) bindings.
3. No source/build changes required — bindings fully satisfy the spec.
