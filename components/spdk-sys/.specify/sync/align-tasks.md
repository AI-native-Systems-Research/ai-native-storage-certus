# Align Tasks: spdk-sys

Tasks appended by spec-sync apply (AUTO-BACKFILL mode) that require follow-up work
outside the scope of a pure spec backfill (i.e. moderate-severity drift, defects,
or ambiguities that should not be resolved by silently weakening spec claims).

## Task: Extend sanity-test coverage to P1 NVMe FFI surface

- **Severity**: Medium
- **Type**: align-task (test-coverage gap)
- **Spec**: `components/spdk-sys/specs/001-spdk-sys/spec.md` — Success Criteria
  ("`cargo test -p spdk-sys` passes all sanity tests (type sizes, field access,
  function pointer existence)") and FR-3/FR-4/FR-5.
- **Source**: `components/spdk-sys/.specify/sync/drift-report.md` — Drifted Items,
  "Success Criteria (sanity test coverage)" (severity: medium).
- **Description**: `tests/bindings_sanity.rs` currently only exercises
  `spdk_env_opts`, `spdk_pci_addr`, `spdk_pci_id`, `spdk_pci_device` and their
  accessor functions. None of the P1 NVMe types/functions introduced by FR-3
  (`spdk_nvme_probe`, `spdk_nvme_detach`), FR-4 (`spdk_nvme_ctrlr`,
  `spdk_nvme_ns`, `spdk_nvme_qpair`, `alloc_io_qpair`/`free_io_qpair`,
  `create_ns`/`attach_ns`/`delete_ns`/`format`, `cmd_admin_raw`), or FR-5
  (`spdk_nvme_cpl`, `spdk_nvme_ns_cmd_{read,write,write_zeroes,flush}`,
  `spdk_nvme_qpair_process_completions`) have any sanity or
  function-pointer-existence test. This is a code-level gap, not a spec-text
  drift, so per the AUTO-BACKFILL hard rules it is NOT resolved by weakening
  the Success Criteria bullet — the criteria remain the target and the test
  suite must be brought up to meet them.
- **Pre-existing linkage**: This duplicates/confirms the still-open task in
  `components/spdk-sys/specs/001-spdk-sys/tasks.md` ("Validate that sanity
  tests cover all critical types used by downstream consumers"). No new
  tasks.md entry was added since one already exists; this align-task tracks
  it for the sync-apply record and links it to the drift report.
- **Suggested resolution** (implementation, out of scope for this spec-only
  apply): add sanity tests to `tests/bindings_sanity.rs` for `spdk_nvme_ctrlr`,
  `spdk_nvme_ns`, `spdk_nvme_qpair`, `spdk_nvme_cpl` (type sizes / field
  access) and at least the P1 NVMe function pointers (`spdk_nvme_probe`,
  `spdk_nvme_detach`, `spdk_nvme_ctrlr_alloc_io_qpair`, etc.) so that the
  Success Criteria claim is actually exercised.
- **Action owner**: implementation follow-up (requires editing
  `tests/bindings_sanity.rs`, which is source code and out of scope for this
  Markdown-only sync-apply).
