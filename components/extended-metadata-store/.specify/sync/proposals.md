# Spec-Sync Proposals (Phase B): extended-metadata-store

**Generated**: 2026-08-20
**Source**: `.specify/sync/drift-report.{json,md}`
**Policy**: `.specify/sync/PHASE_B_POLICY.md`

One entry per drifted requirement, per unspecced feature. Directions per the
shared Phase B policy: BACKFILL (spec -> matches working code), ALIGN (code
violates a correct spec -> task, no source edit), RESOLVED (already fixed on the
main thread), BACKFILL-UNSPECCED (document working feature).

| ID | Spec | Requirement | Direction | Applied |
|----|------|-------------|-----------|---------|
| P1 | 001 | FR-05 (`force_flush` durability) | BACKFILL | yes |
| P2 | 001 | NFR-07 (`MockBlockDevice::read_write_stats`) | RESOLVED | yes |
| P3 | 001 | SC-1/2/3/6/7 (workspace membership) | RESOLVED | yes |
| P4 | 002 | FR-011 (interface-only usage) | ALIGN (ALIGN-EMS-001) | yes |
| P5 | 002 | FR-007 (surface `CapacityExhausted`) | ALIGN (ALIGN-EMS-002) | yes |
| P6 | 001 | UNSPECCED `Superblock::region_capacity_bytes()` | BACKFILL-UNSPECCED (FR-18) | yes |
| P7 | 001 | UNSPECCED `create_test_component_from_state()` | BACKFILL-UNSPECCED (NFR-11) | yes |
| P8 | 001 | UNSPECCED `CapacityExhausted` never constructed | BACKFILL-UNSPECCED (Known Gaps + ALIGN-EMS-002) | yes |

---

## P1 — FR-05 `force_flush()` durability — BACKFILL

- **Rationale**: The spec said the durability fix was "drafted on branch
  `sync/spec-drift-sweep-20260807`". The trigger-based fix is now present and
  merged in the working tree (`src/lib.rs:201-215`; `attach_flush_trigger` at
  `:111`; `FlushTrigger` alias at `:68`). Working, intended code + stale spec =
  spec-lag -> BACKFILL to Implemented.
- **Before**: FR-05 status = "Fix drafted (branch …)"; Known Gaps described the
  fix as drafted and "not yet verified under testing/spdk" for two reasons
  (crate outside workspace; MockBlockDevice missing `read_write_stats`).
- **After**: FR-05 status = **Implemented**, with the trigger mechanism described
  in the requirement text. Known Gaps FR-05 entry rewritten to **RESOLVED**
  (both former blockers now fixed — see P2/P3). US6 note, Feature-Gating note,
  and the Success-Criteria workspace note updated to remove the stale "no-op"
  / "deferred" language.

## P2 — NFR-07 `MockBlockDevice::read_write_stats` — RESOLVED

- **Rationale**: Already fixed on the main thread. `impl IBlockDevice for
  MockBlockDevice` now provides `read_write_stats(&self) -> ReadWriteStats`
  returning `ReadWriteStats::default()` (`src/test_support.rs:223`), so the
  `testing`/`spdk` build compiles. NFR-07's requirement text ("MockBlockDevice
  with fault injection", Implemented) is correct — no backfill, no task. Stale
  "blocker" references removed as part of recording RESOLVED.

## P3 — SC-1/2/3/6/7 workspace membership — RESOLVED

- **Rationale**: Already fixed on the main thread. Root `Cargo.toml` lists
  `components/extended-metadata-store` under `[workspace] members` (line 23) and
  `[workspace.dependencies]` (line 105), so CI builds and tests the crate. The
  Success Criteria are correct and now exercisable — no backfill, no task. The
  stale "cannot currently be exercised … not a workspace member" note is updated
  to RESOLVED.

## P4 — FR-011 interface-only usage — ALIGN (ALIGN-EMS-001)

- **Rationale**: The spec requirement (use the `IExtendedMetadataStore` interface,
  not internal APIs) is correct and agreed. The test genuinely violates it:
  store creation and durability go through inherent/internal APIs
  (`initialize_from_client`, `snapshot_entries`, `flush::flush_to_disk`,
  `mark_flushed`, `load_entries`). FR-05 now being implemented makes the
  durability step movable onto `force_flush()`. Correct spec + non-conforming
  code -> ALIGN task; no source edited, requirement not relaxed.

## P5 — FR-007 surface capacity to caller — ALIGN (ALIGN-EMS-002)

- **Rationale**: The spec intent (capacity surfaced to the caller as
  `CapacityExhausted`; `test_capacity_exhaustion` expects it) is correct and
  **not aspirational** — the interface already defines the `CapacityExhausted`
  variant for exactly this. The code only enforces `ValueTooLarge` at `put()` and
  a flush-time `String` error in `flush::flush_to_disk`, never constructing
  `CapacityExhausted`, so the test passes trivially. Preferred **ALIGN** over
  HUMAN_DECISION because the designed-but-unused variant is evidence of intent.
  ALIGN task filed to construct `CapacityExhausted` on an interface method; no
  source edited, spec not relaxed to flush-time-only (option b rejected).

## P6 — UNSPECCED `Superblock::region_capacity_bytes()` — BACKFILL-UNSPECCED (FR-18)

- **Rationale**: Genuine working public accessor on the always-compiled on-disk
  format module (`src/on_disk.rs:142`), returning `region_a_size * sector_size`.
  Documented as **FR-18** (Implemented) rather than removed.

## P7 — UNSPECCED `create_test_component_from_state()` — BACKFILL-UNSPECCED (NFR-11)

- **Rationale**: Genuine `testing`-gated test-infrastructure helper
  (`src/test_support.rs:272`) reconstructing a component over reboot-preserved
  `MockState`. Documented as **NFR-11** (Implemented) rather than removed.

## P8 — UNSPECCED `CapacityExhausted` never constructed — BACKFILL-UNSPECCED (Known Gaps)

- **Rationale**: Unlike P6/P7, this is **not** a working feature — the variant is
  defined (`interfaces/src/iextended_metadata_store.rs:12`) but never constructed.
  Per policy "don't invent", it is documented in the 001 spec Known Gaps as a
  defined-but-unconstructed interface variant whose construction is the subject of
  the FR-007 ALIGN task (ALIGN-EMS-002), rather than claimed as implemented.
