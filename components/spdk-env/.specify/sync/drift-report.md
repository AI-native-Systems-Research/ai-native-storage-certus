---
spec_sync_component: spdk-env
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:46:08Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 91b8d9b3f59c4bf2b14342e91a3d2959f66e0d8c3a32b972a6a7941da13b7fed
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report: spdk-env

**Generated**: 2026-09-02T21:46:08Z
**Project**: spdk-env

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 2 |
| Requirements Checked | 26 |
| Aligned | 24 |
| Drifted | 1 |
| Not Implemented | 1 |
| Unspecced Features | 0 |

Spec `001-spdk-vfio-env` is a **superseded, unfilled `spec-template.md`
scaffold** (its own banner says so and points to `002`); its placeholder
FR/SC are excluded from the requirements count. Spec `002-spdk-env-vfio-init`
is the live spec (FR-001..FR-021, SC-001..SC-005 = 26). Source (`src/*.rs`)
is unchanged since 2026-06-11; independently re-verified against the spec this
run. One spec-acknowledged Not-Implemented item (FR-015) and one genuine
device-type **scope drift** (SC-001 / User Story 1 narrative / Clarifications
promise "all SPDK-supported device types" but the code enumerates NVMe only).
The previously-reported `stale-crate-paths` drift has been **resolved** by the
2026-08-20 backfill (verified present this run).

## Detailed Findings

### 001-spdk-vfio-env — SUPERSEDED (unfilled template)

No analysis performed. The document is a raw `spec-template.md` scaffold with
placeholder text (`[FEATURE NAME]`, `FR-001: System MUST [specific
capability...]`). The banner (lines 1-7) marks it Superseded by
`002-spdk-env-vfio-init` and instructs "Do not implement against this
document." Not a true drift — reported as superseded. No requirements counted.

### 002-spdk-env-vfio-init — SPDK/DPDK Environment with VFIO Device Iteration

FR-001..FR-021 and SC-001..SC-005.

- ✓ FR-001 (`define_component!`/`define_interface!`): `SPDKEnvComponent`
  `src/lib.rs:58`, `ISPDKEnv` `src/lib.rs:34`.
- ✓ FR-002 (ISPDKEnv device query + `init()`): `src/lib.rs:34-56`.
- ✓ FR-003 (init on `init()`, not construction): `src/lib.rs:70` →
  `env.rs:17` `do_init`; construction (`new`) sets no SPDK state.
- ✓ FR-004 (VFIO presence: /dev/vfio + vfio-pci module): `checks.rs:16`
  `check_vfio_available`.
- ✓ FR-005 (permission checks with specific path): `checks.rs:44`
  `check_vfio_permissions`, per-path `PermissionDenied` messages include the
  offending path, uid/gid, mode (`checks.rs:96-101,135-140`).
- ✓ FR-006 (`spdk_pci_enumerate` w/ NVMe driver, non-attach callback returns
  non-zero → devices preserved for later probe): `env.rs:115-185`, `enum_cb`
  returns `1` (`env.rs:161`). NOTE: FR-006 itself scopes enumeration to the
  **NVMe** PCI driver only (`spdk_pci_get_driver("nvme")`, `env.rs:165`) — this
  is the narrower reality that SC-001/US1/Clarifications contradict (see
  Drift below).
- ✓ FR-007 (`eprintln!` diagnostics, no logger receptacle): `env.rs:43,48,53,
  173,176`; `SPDKEnvComponent` declares no receptacles (`src/lib.rs:58-67`).
- ✓ FR-008 (non-root operation): permission checks target user-configurable
  paths; `init_spdk_env` requests no root-only opts (`env.rs:73-105`, shm_id
  = -1, no `--no-huge`).
- ✓ FR-009 (empty list, not error, when no devices): `enumerate_devices`
  returns `Ok(Vec::new())` when nothing matches (`env.rs:116,184`).
- ✓ FR-010 (runnable example instantiating, init, query, print):
  `examples/spdk-env-example.rs` (no logger wiring, per FR-007).
- ✓ FR-011 (plain procedural, non-actor): no actor macro / thread spawn /
  message queue in `src/`.
- ✓ FR-012 (Drop cleanup): `src/lib.rs:100-107` Drop → `env.rs:188` `do_fini`.
- ✓ FR-013 (hugepage check, 2MB + 1GB pools): `checks.rs:154-175`
  `check_hugepages`.
- ✓ FR-014 (singleton via process-global `AtomicBool`, cleared on failure and
  on Drop/fini): `env.rs:11` `SPDK_ENV_ACTIVE`, `env.rs:19-26` compare_exchange,
  `env.rs:30-32` clear-on-failure, `src/lib.rs:75-79`/`:102-106` clear on
  fini/Drop.
- ✓ FR-016 (`is_initialized`): `src/lib.rs:95`.
- ✓ FR-017 (`device_count`, no clone): `src/lib.rs:88-93`.
- ✓ FR-018 (ISPDKEnv defined locally + mirrored in `interfaces` crate, manually
  kept in sync): local `src/lib.rs:34-56`; mirror
  `components/interfaces/src/ispdk_env.rs:5-27`. **Verified identical** — both
  expose the same 5 methods (`init`, `fini`, `devices`, `device_count`,
  `is_initialized`) with matching signatures/doc comments.
- ✓ FR-019 (`fini()` explicit teardown + idempotent flag): `src/lib.rs:74-79`
  (no-op if not initialized), `env.rs:188-200` (`set_spdk_env_active(false)` →
  `spdk_env_fini()` → clear flag).
- ✓ FR-020 (`DmaBuffer` re-export + SPDK-active guard): `src/dma.rs:6`
  (`pub use interfaces::DmaBuffer;`); backing impl in
  `interfaces/src/spdk_types.rs` — `new` (`:238`, `spdk_zmalloc`/
  `spdk_dma_zmalloc`), `unsafe from_raw` (`:293`), `Deref`/`DerefMut` (`:392,
  :401`), `drop` guarded by `is_spdk_env_active()` (`:376-381`). Active flag set
  true by `init_spdk_env` (`env.rs:102`) and false by `do_fini` (`env.rs:191`).
- ✓ FR-021 (5 operator scripts under `scripts/`): all present —
  `bind_vfio.sh`, `add_kernel_options.sh`, `cfg_user_spdk.sh`,
  `show_spdk_devices.sh`, `fix_dnf_cache.sh`.
- ✓ SC-002 (specific issue in first error message): each check returns a
  distinct `SpdkEnvError` variant with actionable text before proceeding
  (`checks.rs`); stale "missing logger" case already backfilled out.
- ✓ SC-003 (example runs non-root, prints devices): `examples/spdk-env-example.rs`.
- ✓ SC-004 (synchronous, no threads → procedural): confirmed no thread spawn.
- ✓ SC-005 (diagnostics via `eprintln!`): `env.rs` progress/warning prints.

- ✗ **FR-015** (Not Implemented — spec-acknowledged): skip devices in use by
  another process, log a per-device warning, return only successfully probed
  devices. The spec itself states "(Future: not yet implemented. Currently all
  matching devices are claimed; user must ensure exclusive access via system
  configuration.)". `enumerate_devices` (`env.rs:115-185`) uses a non-attach
  callback (FR-006) and has no probe-and-skip path. User Story 1 Acceptance
  Scenario 4 and its Edge Case describe this behavior in the present tense and
  are therefore unmet. Known/documented gap — leave + note (see align Task 4).

- ⚠️ **SC-001 / User Story 1 narrative / Clarifications — device-type scope
  drift** (medium): SC-001 promises the component "discovers 100% of available
  (not in-use) VFIO-bound devices ... matching the devices visible in
  /sys/bus/pci/drivers/vfio-pci"; the User Story 1 narrative and the recorded
  Clarifications answer (Session 2026-04-07) both say "All SPDK-supported device
  types bound to VFIO (NVMe, virtio-blk, etc.)". The implementation enumerates
  **only** the NVMe PCI driver (`spdk_pci_get_driver("nvme")`,
  `env.rs:164-181`), so a non-NVMe device (e.g. virtio-blk) bound to vfio-pci
  would appear in `/sys/bus/pci/drivers/vfio-pci` yet never be discovered. FR-006
  already narrows scope to NVMe-only and the code matches FR-006 — so this is
  both a spec-vs-code gap and an internal spec contradiction (FR-006 vs
  SC-001/US1/Clarifications). Resolution is a scope decision (extend code to
  enumerate all SPDK device types, OR narrow SC-001/US1/Clarifications to
  NVMe-only to match FR-006) → **HUMAN_DECISION**, tracked in align Task 1. Not
  auto-backfilled: narrowing a recorded product/clarification decision to match a
  partial implementation is out of scope for a confident text backfill.
  - Location: `spec.md` SC-001 (line 174), User Story 1 narrative (line 22),
    Clarifications (line 12); code `components/spdk-env/src/env.rs:164-181`.

## Resolved Since Last Run

- **stale-crate-paths** (was ⚠️ minor): `spdk-sys` moved to `lib/spdk-sys/` and
  `component-framework` to `lib/component-framework/`; supporting docs cited the
  old `components/` paths. Backfilled 2026-08-20 and **verified present** this
  run: `tasks.md:10` dated note + `lib/spdk-sys/...` in T001/T004/T005/T006;
  `spec.md:6` bracketed editorial note. `spec.md:99,188` reference the unchanged
  **crate name** (still resolves) — correctly left unedited. No longer drift.

## Unspecced Features

None found. `checks.rs`, `device.rs` structs (`PciAddress`, `PciId`,
`VfioDevice`) all map to FR-004/FR-005/FR-006 and Key Entities.

## Recommendations

- **SC-001 scope (HUMAN_DECISION)**: decide NVMe-only vs. all-SPDK-device-types.
  If NVMe-only is intended, a follow-up BACKFILL should narrow SC-001, the
  User Story 1 narrative, and the Clarifications answer to match FR-006 + code.
  If broader discovery is intended, file a code task to enumerate additional
  SPDK PCI drivers. (align Task 1.)
- **FR-015**: implement probe-and-skip with per-device warning, or soften the
  present-tense wording of User Story 1 Acceptance Scenario 4 / its Edge Case to
  match the "(Future: not yet implemented)" caveat. (align Task 4.)
