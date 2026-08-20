# Drift Report: spdk-env

**Generated**: pending
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
is the live spec and is largely aligned, with one spec-acknowledged
Not-Implemented item (FR-015) and one minor stale-path drift.

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
- ✓ FR-002 (ISPDKEnv device query + `init()`): `src/lib.rs:34`.
- ✓ FR-003 (init on `init()`, not construction): `env.rs:102`
  `init_spdk_env`.
- ✓ FR-004/FR-005 (VFIO presence + permission checks): `checks.rs:16`
  `check_vfio_available`, `checks.rs:44` `check_vfio_permissions`.
- ✓ FR-006 (`spdk_pci_enumerate`, non-attach callback returns non-zero):
  `env.rs:115-185`, `enum_cb` returns 1.
- ✓ FR-007 (`eprintln!` diagnostics, no logger receptacle): confirmed in
  `env.rs`; component has no receptacles.
- ✓ FR-009 (empty list, not error, when no devices): enumerate path in
  `env.rs`.
- ✓ FR-010 (runnable example): `examples/spdk-env-example.rs` exists.
- ✓ FR-011 (plain procedural, non-actor): no actor macro / threads.
- ✓ FR-012 (Drop cleanup): `src/lib.rs:100` Drop, `env.rs:188` `do_fini`.
- ✓ FR-013 (hugepage check): `checks.rs:154` `check_hugepages`
  (2MB + 1GB pools).
- ✓ FR-014 (singleton via process-global `AtomicBool`, cleared on
  failure/Drop): `env.rs:11` `SPDK_ENV_ACTIVE`, `env.rs:19` `do_init`
  compare_exchange.
- ✓ FR-016 (`is_initialized`), FR-017 (`device_count`): `src/lib.rs:34`
  interface methods.
- ✓ FR-018 (ISPDKEnv defined locally + mirrored in `interfaces` crate, kept in
  sync manually): local `src/lib.rs:34`; mirror
  `components/interfaces/src/ispdk_env.rs` exists.
- ✓ FR-019 (`fini()` explicit teardown + idempotent flag): `env.rs:188`.
- ✓ FR-020 (`DmaBuffer` re-export + SPDK-active guard): `src/dma.rs`
  (`pub use interfaces::DmaBuffer;`), `interfaces::set_spdk_env_active(true)`
  at `env.rs:102`.
- ✓ FR-021 (5 operator scripts under `scripts/`): all present —
  `bind_vfio.sh`, `add_kernel_options.sh`, `cfg_user_spdk.sh`,
  `show_spdk_devices.sh`, `fix_dnf_cache.sh`.
- ✓ SC-001..SC-005: consistent with implementation (synchronous, `eprintln!`
  diagnostics, non-root operation, example runs).

- ✗ **FR-015** (Not Implemented — spec-acknowledged): skip devices in use by
  another process, log a warning per skipped device, return only successfully
  probed devices. The spec itself states "(Future: not yet implemented.
  Currently all matching devices are claimed; user must ensure exclusive
  access via system configuration.)". `env.rs` enumeration does not attempt a
  probe-and-skip path. Acceptance Scenario US1#4 and the corresponding edge
  case are therefore unmet. Known/documented gap, not a surprise.

- ⚠️ **Stale crate-path references** (minor): the workspace moved
  `spdk-sys` from `components/` to `lib/`, but supporting spec docs still
  reference the old path.
  - `specs/002-spdk-env-vfio-init/tasks.md:22,24,34,35,36` reference
    `components/spdk-sys/...` (now `lib/spdk-sys/`).
  - `spec.md:6` (historical Input) and `spec.md:99,188` reference
    `../component-framework` / "component-framework" — the crate name is
    unchanged (still resolves as a workspace dep), but the relative
    filesystem path `../component-framework` is now `../../lib/component-framework`.
  - Location: `components/spdk-env/specs/002-spdk-env-vfio-init/tasks.md:22`
  - Severity: minor (docs only; no impact on the code path).

## Unspecced Features

None found. `checks.rs`, `device.rs` structs (`PciAddress`, `PciId`,
`VfioDevice`) all map to FR-004/FR-005/FR-006 and Key Entities.

## Recommendations

- Implement FR-015 (probe-and-skip in-use devices with per-device warning) or
  keep the "Future" annotation and move it out of the mandatory FR list until
  scheduled.
- Update `tasks.md` (and the FR-021 script-path phrasing) to `lib/spdk-sys/`;
  optionally note in spec 002 that `component-framework` now lives at
  `lib/component-framework`.
