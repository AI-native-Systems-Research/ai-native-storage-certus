---
spec_sync_component: spdk-sys
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T17:59:17Z
spec_sync_git_commit: 3a988d9a
spec_sync_inputs_sha256: 740544ac22940f3af59d6d8f4ce0a133d4f86b8bc49a73a6be2b035e70ed8c4d
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: spdk-sys

**Generated**: 2026-09-03
**Component**: `lib/spdk-sys` (relocated from `components/spdk-sys`)
**Scope**: `specs/001-spdk-sys/spec.md` (+ plan/tasks skim) vs implementation in
`build.rs`, `src/lib.rs`, `wrapper.h`, `Cargo.toml`, and `tests/bindings_sanity.rs`.
One BACKFILL (code→spec) and one ALIGN (stale path) resolution applied this
sweep. No source or build files were changed.

> **Correction of the prior artifact.** The previous report read
> "**Generated**: pending" and classified the abort (`spdk_nvme_ctrlr_cmd_abort_ext`)
> and max-transfer-size (`spdk_nvme_ctrlr_get_max_xfer_size`) bindings as an
> "Unspecced Feature — plausibly covered by FR-4 but not named," recommending
> only that FR-4 *optionally* be extended. This sweep re-verifies the current
> `build.rs` against the current spec and **resolves** that gap (plus a third,
> previously-unnoticed cluster — the five `spdk_nvme_ns_*` info accessors) by
> backfilling FR-4, rather than leaving it as a standing recommendation.

> **Note on the CI input hash.** `scripts/spec-sync-hash.sh lib/spdk-sys` hashes
> `<dir>/src/**` + `<dir>/specs/**` + the `components/interfaces` tree. For this
> crate `src/` is a **one-line `include!` shim** (`src/lib.rs:19`); the actual
> binding-generation logic — the bindgen allowlist, linker directives, and
> SPDK-discovery error handling that FR-1..9 / NFR-1..5 describe — lives in
> **`build.rs`**, and the sanity tests live in **`tests/`**. Neither `build.rs`,
> `tests/`, `wrapper.h`, nor `Cargo.toml` is under `src/` or `specs/`, so **none
> is covered by the committed digest**. The findings below were nonetheless
> verified by hand against `build.rs` and `tests/bindings_sanity.rs`.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (FR + NFR + Success Criteria + Key Entities) | 9 FR + 5 NFR + 4 SC + 17 entities |
| Aligned (behavior) | all |
| Drifted this sweep | 1 unspecced-binding gap (BACKFILL) + 1 doc-only stale path (ALIGN) |
| Not Implemented | 0 |
| Parked / tracked (not spec↔impl behavioral drift) | 2 (NVMe sanity-test coverage task; README rte_* count) |

---

## Detailed Findings

### Spec 001-spdk-sys — spdk-sys FFI bindings

**Behavior: all 9 FR + 5 NFR + Success Criteria + 17 Key Entities verified
CONFIRMED against the current `build.rs`.** Every allowlisted function, type,
and constant maps to a requirement (after the FR-4 backfill below); every FR/NFR
maps to concrete `build.rs` logic.

**Functional Requirements — all Aligned:**

- **FR-1** (env API: `spdk_env_opts_init`/`spdk_env_init`/`spdk_env_fini`) ✓ `build.rs:165-167`
- **FR-2** (PCI enumeration + accessors `spdk_pci_*`) ✓ `build.rs:168-182` (15 fns: enumerate, for_each_device, get_driver, and 12 device accessors incl. `get_numa_id:181`, `get_serial_number:182`)
- **FR-3** (NVMe probe/attach/detach lifecycle) ✓ `build.rs:184-185` (`spdk_nvme_probe`, `spdk_nvme_detach`; attach is delivered via the probe attach callback, not a separately allowlisted symbol — accurate)
- **FR-4** (controller + namespace operations) ✓ `build.rs:187-205,207-211,220` — **BACKFILLED this sweep** (see below). Covers get_num_ns/get_ns (`187-188`), alloc/free_io_qpair (`189-190`), process_admin_completions (`191`), get_default_ctrlr_opts (`192`), reset (`193`), get_data (`194`), **get_max_xfer_size (`197`)**, cmd_admin_raw (`199`), create/attach/delete_ns + format + get_id (`201-205`), **ns info accessors is_active/get_data/get_sector_size/get_num_sectors/get_size (`207-211`)**, and **cmd_abort_ext (`220`)**.
- **FR-5** (I/O ops: read/write/write_zeroes/flush + completion processing) ✓ `build.rs:213-216,221` (`ns_cmd_read/write/write_zeroes/flush`, `qpair_process_completions`)
- **FR-6** (DMA alloc) ✓ `build.rs:223-226` (`spdk_dma_zmalloc`/`spdk_dma_free`/`spdk_zmalloc`/`spdk_free`)
- **FR-7** (SPDK/DPDK/system linker directives) ✓ `build.rs:45` (search path), `58-76` (13 SPDK libs), `116-118` (30 DPDK libs), `128-138` (8 system libs)
- **FR-8** (conditional ISA-L linkage) ✓ `build.rs:40-42` (detect `SPDK_CONFIG_ISAL` from `config.h`), `123-125` (conditional link)
- **FR-9** (constants `SPDK_PCI_*`, `SPDK_NVME_TRANSPORT_*` via `allowlist_var`) ✓ `build.rs:246-247`

**Non-Functional Requirements — all Aligned:**

- **NFR-1** (clear error if SPDK source/build missing) ✓ `build.rs:13-19` (src), `22-28` (build dir), `33-39` (config header)
- **NFR-2** (regenerate on `wrapper.h` / lib dir change) ✓ `build.rs:263-267`
- **NFR-3** (layout tests disabled) ✓ `build.rs:253`
- **NFR-4** (all SPDK libs `+whole-archive`) ✓ `build.rs:58-76` (SPDK), `117` (DPDK), `124` (ISA-L)
- **NFR-5** (GCC internal include path detected for clang/bindgen) ✓ `build.rs:142-153,160-162`

**Success Criteria — Aligned:**

- `cargo build -p spdk-sys` succeeds against a pre-built SPDK ✓ (build logic present; not re-run this sweep — requires `deps/spdk-build/`).
- `cargo test -p spdk-sys` passes all sanity tests ✓ for the tests present (`tests/bindings_sanity.rs`: type sizes, field access, function-pointer existence). Coverage is env/PCI-only — see the tracked task below; this is a code-completeness gap, **not** a false SC statement (the tests that exist do pass and do exercise all three check kinds).
- Higher-level crates (`spdk-env`, `block-device-spdk-nvme`) import the bindings ✓ (they consume `spdk-sys` in-tree).
- `links = "spdk"` dedup metadata ✓ `Cargo.toml`.

**Key Entities — all 17 Aligned:** every entity row maps to an `allowlist_type`
at `build.rs:228-245`; `spdk_nvme_ctrlr_data` is marked `opaque_type` (`:235`),
matching the spec's opaque note. No allowlisted type is missing from the table
and no table row lacks an allowlist entry.

**Implementation Notes — Aligned:** "30 DPDK `rte_*` libraries" matches the
`dpdk_libs` array (counted: 30, `build.rs:83-114`); `spdk_nvme_ctrlr_data`
opaque ✓; `wrapper.h` includes `spdk/env.h`, `spdk/env_dpdk.h`, `spdk/nvme.h` ✓.

### Resolved this sweep

- **FR-4 (BACKFILL — code→spec).** Three allowlisted function clusters existed
  in `build.rs` but were named by no requirement:
  - `spdk_nvme_ctrlr_get_max_xfer_size` (`build.rs:197`) — MDTS-derived max
    transfer size, used by the block-device actor to fragment I/O to the
    device's real limit.
  - `spdk_nvme_ctrlr_cmd_abort_ext` (`build.rs:220`) — in-flight command abort,
    used on `AbortOp` so the controller does not DMA into a released buffer.
  - `spdk_nvme_ns_is_active` / `_get_data` / `_get_sector_size` /
    `_get_num_sectors` / `_get_size` (`build.rs:207-211`) — namespace info
    accessors (state + geometry), distinct from the namespace *management*
    (`create/attach/delete/format`) that FR-4 already named and from the I/O
    ops in FR-5.

  FR-4's requirement text was extended to name all three clusters explicitly,
  and an audit-trail note was added under Implementation Notes. **No source or
  build file was touched** — the bindings already existed; only the spec was
  made to describe them. (This is the gap the prior report had left as an
  optional recommendation.)
- **Stale path (ALIGN — spec→reality).** `.specify/sync/align-tasks.md` referred
  to the pre-relocation `components/spdk-sys/...` paths at lines 11, 14, 29;
  corrected to `lib/spdk-sys/...`. `.specify/sync/**` is outside the CI hash
  scope, so this edit does not affect the digest.

---

## Parked / tracked (not spec↔impl behavioral drift)

1. **NVMe sanity-test coverage gap** (medium, tracked). `tests/bindings_sanity.rs`
   exercises only `spdk_env_opts` / `spdk_pci_addr` / `spdk_pci_id` /
   `spdk_pci_device` and a handful of env/PCI function pointers. None of the P1
   NVMe types/functions from FR-3/FR-4/FR-5 (`spdk_nvme_ctrlr`, `spdk_nvme_ns`,
   `spdk_nvme_qpair`, `spdk_nvme_cpl`, `spdk_nvme_probe`, `alloc_io_qpair`, …)
   has a sanity or fn-pointer test. This is a **code-level test-coverage gap,
   not a spec-text contradiction** — the Success Criteria bullet remains the
   target and must not be weakened. Tracked as the still-open item in
   `specs/001-spdk-sys/tasks.md` and mirrored in
   `.specify/sync/align-tasks.md` ("Extend sanity-test coverage to P1 NVMe FFI
   surface"). Resolving it requires editing `tests/bindings_sanity.rs` (source),
   out of scope for a spec-sync apply.

2. **`README.md:33` DPDK library count** (doc-only, below threshold). The README
   says "28 `rte_*` libraries" while `build.rs` links **30** (counted from the
   `dpdk_libs` array). `README.md` is neither a spec nor code under the hash
   scope, and the spec-sync apply scope is limited to Markdown under `specs/**`
   and `.specify/sync/**`, so it was **not** edited this sweep. Recommend
   correcting "28" → "30" (and the SPDK/system-lib bullets, which also lag
   `build.rs`) the next time `README.md` is touched. `spec.md`'s own
   Implementation Notes already say "30", so the spec is correct.

## Historical artifact (not edited)

- `.specify/sync/apply-report.md` records the 2026-07-22 AUTO-BACKFILL apply and
  still cites `components/spdk-sys/...` paths (lines 4-7, 11) and a stale
  `allowlist_var` anchor (`build.rs:239-240`; now `:246-247`). It is a
  **dated point-in-time record** whose paths were correct when written; it was
  left intact rather than retroactively rewritten. Not in hash scope.

## Stamp rationale

`drift_status: clean`. Every FR (1-9), NFR (1-5), Success-Criteria statement,
and Key-Entity row is behaviorally aligned with the shipped `build.rs` /
`Cargo.toml` / `wrapper.h`, independently re-verified this sweep (not carried
over from the stale "pending" report). The one genuine spec-level gap — three
allowlisted binding clusters unnamed by any FR — was resolved in-place via a
BACKFILL to `specs/001-spdk-sys/spec.md` (FR-4 + an Implementation Notes audit
line); a stale relocation path in `align-tasks.md` was ALIGNed. **No `src/`,
`build.rs`, `tests/`, or interface source was changed**, so no
build/test/clippy/doc state changed. The two remaining items — the NVMe
sanity-test coverage task and the README `rte_*` count — are a tracked
code-completeness follow-up and a doc-only nit respectively; **neither is a
spec↔implementation behavioral contradiction**. This is not a clean stamp over
an unacknowledged mismatch: every remaining gap is documented here, in the spec,
and in the task tracker.
