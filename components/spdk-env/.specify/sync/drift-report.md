---
spec_sync_component: spdk-env
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-04T02:05:38Z
spec_sync_git_commit: 7219c346
spec_sync_inputs_sha256: 8fc7d79b7e010c3adf924e3842e0fe6d1f21b9e272329bef38190a40f94fe802
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: spdk-env

**This sweep (2026-09-03)** independently re-verified spec `002-spdk-env-vfio-init`
(19 FR + 5 SC) against `src/{lib,env,checks,device,dma,error}.rs`,
`examples/spdk-env-example.rs`, and the mirror interface
(`components/interfaces/src/{ispdk_env,spdk_types}.rs`). Three documentation
BACKFILLs were applied (D1/D2/D5). **One actionable drift remains unresolved
(D4) — hence `drift_status: drift`.** No code bugs (no ALIGN) were found in this
sweep; D4 is a product/engineering scope decision, not a defect.

Spec `001-spdk-vfio-env` is a **superseded, unfilled `spec-template.md`
scaffold** (its own Supersession Notice at lines 1-7 says so and points to
`002`); its placeholder FR/SC are excluded from analysis.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 2 (1 live, 1 superseded scaffold) |
| Requirements Checked | 24 (19 FR + 5 SC, spec 002) |
| Aligned | 18 FR + 4 SC |
| Drifted → resolved this sweep | 3 doc BACKFILLs (D1 FR-005, D2 FR-015 note, D5 SC-005) |
| Drifted → **UNRESOLVED** | 1 (D4 — SC-001/User Story 1 vs FR-006, HUMAN_DECISION) |
| Not Implemented (spec-acknowledged) | 1 (FR-015 skip-and-warn, explicitly "Future") |
| Unspecced (public surface, low) | see notes |

## Why `drift` (not `clean`)

**D4** is a genuine, unresolved spec-vs-code inconsistency about *promised
capability*, tracked as **Task 1 in `.specify/sync/align-tasks.md`** and there
classified as a real functional gap, not mere doc lag. Resolving it requires
either (a) a code change to enumerate all SPDK-supported VFIO device types, or
(b) a deliberate product decision to keep NVMe-only scope and re-scope
SC-001/User Story 1/Clarifications to match FR-006. Neither is a text-only
mechanical fix, and the choice belongs to a human (the `--interactive` propose
step). Per the sync mandate, this stamp is **not** set to `clean` to pass the
gate; the drift is left visible and explained until the scope decision is made.
The HARD RULE is respected: NVMe-only is a deliberate FR-006 implementation, so
this is **not** being backfilled to mask a code bug — it is left open for a
human decision.

## Resolved this sweep (BACKFILL — doc lag against correct code)

**D1 — FR-005 (`spec.md:103`).** Spec said "check read/write permissions on
/dev/vfio". Code checks the `/dev/vfio` *directory* **read-only**
(`check_path_readable`, `checks.rs:53,77-110`; comment: "we need to list
entries, not write"); only the `/dev/vfio/vfio` container and the numeric
IOMMU-group nodes get the RW check (`checks.rs:56-70`). Requiring write on a
directory you only enumerate would be wrong — code is correct. Reworded FR-005
to "read access on the /dev/vfio directory ... and read/write on /dev/vfio/vfio
and IOMMU group device files."

**D2 — FR-015 note (`spec.md:113`).** The parenthetical said "Currently all
matching devices are **claimed**." The enumeration callback returns `1`
specifically so devices are **NOT** attached/claimed (`env.rs:160-161`,
mandated by FR-006). Reworded to "Devices are enumerated but NOT claimed — the
enumeration callback returns non-zero so no device is attached during discovery,
per FR-006." (This does not touch FR-015's Not-Implemented status — see below.)

**D5 — SC-005 (`spec.md:178`).** The parenthetical listed "permission/hugepage/
VFIO check failures" among `eprintln!` output. Those failures are returned as
`Err(SpdkEnvError::…)` from `checks.rs` and never printed by the component; only
progress lines and enumeration warnings are `eprintln!`'d (`env.rs:43,48,53,173,176`).
The *example* prints the returned error (`examples/spdk-env-example.rs:64`).
Reworded to reflect that pre-flight failures are returned as structured errors,
not printed.

## Unresolved this sweep

**D4 — SC-001 / User Story 1 / Clarifications vs FR-006 (HUMAN_DECISION, medium).**
- SC-001 (`spec.md:174`): "discovers 100% of available … VFIO-bound devices";
  User Story 1 (`spec.md:22`) and the Clarification (`spec.md:12`): "all
  SPDK-supported device types (NVMe, virtio-blk, etc.)".
- FR-006 (`spec.md:104`) narrows to NVMe-only; the code implements exactly that:
  `enumerate_devices` calls `spdk_pci_enumerate` only against
  `spdk_pci_get_driver("nvme")` (`env.rs:163-181`). virtio-blk and other types
  are never enumerated.
- This is an internal spec inconsistency AND a real functional gap. Left
  **unresolved** and untouched in `src/` and in SC-001/User Story 1 pending the
  product decision recorded in `align-tasks.md` Task 1. **This is the reason for
  `drift_status: drift`.**

## No change needed

**D3 — FR-020 (`spec.md:135-137`).** `DmaBuffer::new` "requires the SPDK
environment to already be initialized." The precondition is enforced implicitly
(SPDK returns NULL → `DmaAllocationFailed`, `spdk_types.rs:265-269`) rather than
by an explicit flag check in `new()`. Behavior is acceptable (fails cleanly);
"requires … initialized" reads as a documented precondition. No spec or code
change — observation only.

## Aligned ✓ (spec 002)

| FR/SC | Evidence |
|---|---|
| FR-001 macros | `lib.rs:30,34,58` |
| FR-002 ISPDKEnv + init() | `lib.rs:34-56` |
| FR-003 init on init() not construction | `lib.rs:70-72` → `env.rs:17` |
| FR-004 verify /dev/vfio + vfio-pci module | `checks.rs:16-38`; `env.rs:39` |
| FR-005 perm checks + path in error (post-BACKFILL) | `checks.rs:49-74,96-101,135-140` |
| FR-006 NVMe enumerate, non-attach (cb returns 1) | `env.rs:115-185,160-165` |
| FR-007 eprintln!, no receptacles | `env.rs:43,48,53,173,176`; `lib.rs:58-67` |
| FR-008 non-root operation | `checks.rs:78-149` (uid/gid + mode bits) |
| FR-009 empty list, not error | `env.rs:184` `Ok(devices)` |
| FR-010 runnable example | `examples/spdk-env-example.rs` |
| FR-011 plain procedural, no threads | non-actor `define_component!`; no spawns |
| FR-012 Drop cleanup | `lib.rs:100-107` → `env::do_fini` |
| FR-013 hugepage check + clear error | `checks.rs:154-175`; `env.rs:41` |
| FR-014 singleton AtomicBool; cleared on failure & Drop | `env.rs:11,19-32,199`; `lib.rs:100-107` |
| FR-016 is_initialized() | `lib.rs:54,95-97` |
| FR-017 device_count() no clone | `lib.rs:88-93` (`.len()` on read guard) |
| FR-018 local define_interface! + synced mirror | `lib.rs:34-56`; `interfaces/src/ispdk_env.rs:5-27` |
| FR-019 explicit fini(), idempotent, Drop reuses teardown | `lib.rs:74-79`; `env.rs:188-200`; `lib.rs:100-107` |
| FR-020 DmaBuffer re-export, new/from_raw, Deref/DerefMut, flag-gated Drop | `dma.rs:6`; `spdk_types.rs:238,293,375-401`; `env.rs:102,191-196` |
| FR-021 five operator scripts | `bind_vfio.sh`, `add_kernel_options.sh`, `cfg_user_spdk.sh`, `show_spdk_devices.sh`, `fix_dnf_cache.sh` |
| SC-002 specific issue in first error | `checks.rs:26-34,96-101,135-140,167-171` |
| SC-003 example compiles/runs non-root (structural) | `examples/spdk-env-example.rs:40-61` |
| SC-004 synchronous, no threads | `env.rs:17-70`; no spawns |
| SC-005 eprintln! diagnostics (post-BACKFILL) | `env.rs:43,48,53,173,176` |

## Not Implemented ✗ (spec-acknowledged)

- **FR-015** skip-and-warn for in-use devices — the FR text itself marks it
  "(Future: not yet implemented …)". `enumerate_devices` performs no
  probe-and-skip. Not a surprise; documented. (align-tasks Task 4 tracks this as
  informational.)

## Unspecced public surface (low, noted — not resolved this sweep)

- `checks::check_vfio_available`, `check_vfio_permissions`, `check_hugepages`
  are `pub` (not `pub(crate)`) — `checks.rs:16,44,154`. The spec treats these as
  internal pre-flight steps (FR-004/005/013); their exposure as a public crate
  API is uncovered. Candidate `pub(crate)` tightening or a BACKFILL-UNSPECCED FR.
- `DmaBuffer` accessors (`len`, `as_ptr`, `as_slice`, `numa_node`, `metadata`, …)
  and `DmaAllocFn` live in the `interfaces` crate and are re-exported; FR-020
  specifies `new`/`from_raw`/Deref/DerefMut/Drop but not these. Interfaces-crate
  surface; out of `spdk-env`'s spec scope. Left for a coordinated interfaces pass
  (editing `components/interfaces/` invalidates every stamped component's folded
  hash).

## Recommendations

1. **Resolve D4 (align-tasks Task 1).** Product/engineering decision: extend
   `enumerate_devices` to all SPDK VFIO driver types (code ALIGN), or re-scope
   SC-001/User Story 1/Clarifications to NVMe-only (doc BACKFILL). Then the stamp
   can move to `clean`.
2. **align-tasks Task 3 (latent).** `do_init`'s error path clears the singleton
   flag but never calls `spdk_env_fini()`; harmless today because
   `enumerate_devices` always returns `Ok`, but a real leak if enumeration is
   made fallible (e.g. Task 1's extension or FR-015). Code change, out of scope
   for a doc sync.

## Verification feasibility

`spdk-env` is not a workspace default member; it depends on `interfaces`
(`features=["spdk"]`) + `spdk-sys`, both requiring the pre-built SPDK at
`deps/spdk-build/` (present here). This sweep changed only spec Markdown (no
`src/`), so the previously-green `cargo build -p spdk-env` / `clippy -p spdk-env
--all-targets -- -D warnings` / `test -p spdk-env` (57 unit tests via `tempfile`
mocks) state is unaffected. `init()`/`spdk_env_init` runtime paths need
VFIO+hugepages hardware.
