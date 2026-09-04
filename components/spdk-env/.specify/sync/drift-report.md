---
spec_sync_component: spdk-env
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-04T16:10:38Z
spec_sync_git_commit: e7e1bc10
spec_sync_inputs_sha256: ba0268416df0d95407ce161f890379d8a46a4d6bff9b8650773a4e3d50afac44
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: spdk-env

**This sweep (2026-09-03, D4 resolved 2026-09-04)** independently re-verified
spec `002-spdk-env-vfio-init` (19 FR + 5 SC) against
`src/{lib,env,checks,device,dma,error}.rs`, `examples/spdk-env-example.rs`, and
the mirror interface (`components/interfaces/src/{ispdk_env,spdk_types}.rs`).
Four documentation BACKFILLs were applied (D1/D2/D5 on 2026-09-03; D4 on
2026-09-04). **No actionable drift remains — `drift_status: clean`.** No code
bugs (no ALIGN) were found; D4 was a device-scope inconsistency resolved by a
product decision to keep the NVMe-only scope and re-scope the spec to match
(BACKFILL), not by masking a code bug.

Spec `001-spdk-vfio-env` is a **superseded, unfilled `spec-template.md`
scaffold** (its own Supersession Notice at lines 1-7 says so and points to
`002`); its placeholder FR/SC are excluded from analysis.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 2 (1 live, 1 superseded scaffold) |
| Requirements Checked | 24 (19 FR + 5 SC, spec 002) |
| Aligned | 18 FR + 5 SC (SC-001 aligned after D4 BACKFILL) |
| Drifted → resolved this sweep | 4 doc BACKFILLs (D1 FR-005, D2 FR-015 note, D5 SC-005, D4 device scope) |
| Drifted → **UNRESOLVED** | 0 |
| Not Implemented (spec-acknowledged) | 1 (FR-015 skip-and-warn, explicitly "Future") |
| Unspecced (public surface, low) | see notes |

## D4 resolution (2026-09-04) — device scope re-scoped to NVMe (BACKFILL)

**D4** was a genuine spec-vs-code inconsistency about *promised capability*:
SC-001, User Story 1, and the 2026-04-07 Clarification promised "all
SPDK-supported VFIO device types (NVMe, virtio-blk, etc.)", while FR-006 **and
the code** (`enumerate_devices` → `spdk_pci_enumerate` against
`spdk_pci_get_driver("nvme")` only, `env.rs:163-181`) implement NVMe-only. It was
surfaced (not masked) as `drift` pending a product/engineering decision. That
decision — **keep the NVMe-only scope; broadening to other VFIO device types is a
future enhancement, not current behavior** — was taken, so this is resolved by
**BACKFILL** (re-scope the doc to match the correct, deliberate FR-006
implementation), never by an ALIGN that would have masked a bug. Edits, all
carrying a dated backfill note:
- Clarification (`spec.md:12`): answer changed to "NVMe devices bound to VFIO".
- User Story 1 narrative (`spec.md:22`): "discover all VFIO-attached devices" /
  "probes for all SPDK-supported device types (NVMe, virtio-blk, etc.)" →
  "discover the NVMe devices bound to VFIO" / "enumerates NVMe devices bound to
  VFIO (per FR-006)".
- Acceptance Scenario 2 (`spec.md:31`): dropped "for all SPDK-supported device
  types"; scoped to "each enumerated NVMe device".
- User Story 3 Scenario 1 (`spec.md:65`): "all VFIO-bound devices" → "all NVMe
  devices bound to VFIO".
- SC-001 (`spec.md:174`): "VFIO-bound devices" → "NVMe devices bound to VFIO".

The neutral "VFIO-attached device" phrasing in FR-002 and the `VfioDevice` entity
was left as-is — an NVMe device *is* a VFIO-attached device, so those are not
contradictory. If broad device support is later implemented, this BACKFILL should
be revisited (re-widen the scope + extend `enumerate_devices`).

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

None. D4 (device scope) was resolved on 2026-09-04 by BACKFILL — see "D4
resolution" above.

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
| SC-001 discover NVMe VFIO devices (post-BACKFILL, NVMe-only) | `env.rs:115-185`; enumerates `spdk_pci_get_driver("nvme")` only |
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

1. **D4 resolved (align-tasks Task 1).** The device scope was re-scoped to
   NVMe-only by BACKFILL (see above). If broad VFIO device support (virtio-blk,
   etc.) is later desired, re-widen SC-001/User Story 1/Clarifications **and**
   extend `enumerate_devices` together — do not let the doc lead the code again.
2. **align-tasks Task 3 (latent).** `do_init`'s error path clears the singleton
   flag but never calls `spdk_env_fini()`; harmless today because
   `enumerate_devices` always returns `Ok`, but a real leak if enumeration is
   made fallible (e.g. a future device-scope extension or FR-015). Code change,
   out of scope for a doc sync.

## Verification feasibility

`spdk-env` is not a workspace default member; it depends on `interfaces`
(`features=["spdk"]`) + `spdk-sys`, both requiring the pre-built SPDK at
`deps/spdk-build/` (present here). This sweep changed only spec Markdown (no
`src/`), so the previously-green `cargo build -p spdk-env` / `clippy -p spdk-env
--all-targets -- -D warnings` / `test -p spdk-env` (57 unit tests via `tempfile`
mocks) state is unaffected. `init()`/`spdk_env_init` runtime paths need
VFIO+hugepages hardware.
