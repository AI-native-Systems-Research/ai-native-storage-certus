# Spec-Sync Apply Report (Phase B): extended-metadata-store

**Applied**: 2026-08-20
**Source**: `.specify/sync/drift-report.{json,md}`
**Policy**: `.specify/sync/PHASE_B_POLICY.md`
**Backups**: `.specify/sync/backups/specs/001-extended-metadata-store/spec.md.bak`,
`.specify/sync/backups/specs/002-ssd-integration-test/spec.md.bak`
(prior timestamped backups remain under `.specify/sync/backups/2026*`)

## Summary counts

| Outcome | Count |
|---------|-------|
| BACKFILL applied | 1 (FR-05) |
| UNSPECCED backfilled | 3 (FR-18, NFR-11, `CapacityExhausted` Known-Gaps note) |
| ALIGN tasks generated | 2 (ALIGN-EMS-001, ALIGN-EMS-002) |
| RESOLVED | 2 (NFR-07, SC-1/2/3/6/7) |
| HUMAN_DECISION | 0 |
| not_implemented | 0 |

## Specs Updated

### 001-extended-metadata-store/spec.md

| Requirement | Change type | Change |
|-------------|-------------|--------|
| FR-05 | BACKFILL | Status "Fix drafted (branch …)" -> **Implemented**; requirement text now describes the `attach_flush_trigger` / `FlushTrigger` mechanism (`src/lib.rs:201-215`). |
| FR-18 | BACKFILL-UNSPECCED | New FR documenting `Superblock::region_capacity_bytes()` (`src/on_disk.rs:142`). |
| NFR-11 | BACKFILL-UNSPECCED | New NFR documenting `create_test_component_from_state()` (`src/test_support.rs:272`). |
| Known Gaps (FR-05) | BACKFILL | Rewritten to **RESOLVED** — fix present; both former verification blockers (NFR-07 mock, workspace membership) closed. |
| Known Gaps (`CapacityExhausted`) | BACKFILL-UNSPECCED | New entry: variant defined but never constructed; construction tracked by ALIGN-EMS-002. |
| Known Gaps (retained API) | BACKFILL | "dead public API" note reframed: `region_capacity_bytes` -> FR-18, helper -> NFR-11, `StorageError` now constructed by FR-05. |
| US6 note | BACKFILL | Updated: `force_flush()` contract now met when a trigger is installed. |
| Feature-Gating note | BACKFILL | Removed "unconditional no-op under testing/spdk" claim; describes trigger-based durable flush. |
| Success Criteria note | RESOLVED | Workspace-membership note updated to RESOLVED (crate now a workspace member; mock compiles). |
| Header / Last-Synced | metadata | New 2026-08-20 Phase B sync note; prior note retained inline. |

### 002-ssd-integration-test/spec.md

| Requirement | Change type | Change |
|-------------|-------------|--------|
| FR-007 (persistence) | ALIGN note | Annotated: FR-05 implemented; test still uses internal flush path; move-to-interface tracked by ALIGN-EMS-001. Requirement stands. |
| FR-011 | ALIGN note | Annotated: interface-only for ops; creation/durability still inherent; **code-side gap** tracked by ALIGN-EMS-001 (no spec relaxation). |
| Capacity note | ALIGN note | Reversed prior "documents option b (flush-time) as actual behavior"; now option a (surface `CapacityExhausted`) is the target via ALIGN-EMS-002. |
| US3 scenario note | ALIGN note | FR-05 implemented; test still drives internal wiring; re-point via ALIGN-EMS-001. |
| US5 scenario 2 note | ALIGN note | Capacity gap reframed as code-side ALIGN-EMS-002, not accepted behavior. |
| Header / Last-Synced | metadata | New 2026-08-20 Phase B sync note; prior note retained inline. |

> No requirement text in 002 was relaxed to match buggy code; the ALIGN items keep
> the spec as the correct target and file tasks instead.

## Align Tasks Generated

| Task | Spec / Req | Severity | Files to modify (by implementer) |
|------|------------|----------|----------------------------------|
| ALIGN-EMS-001 | 002 / FR-011 (+FR-007 persistence, US3) | moderate | `tests/integration_ssd.rs` |
| ALIGN-EMS-002 | 002 / FR-007 (capacity, US5-2, capacity note) | moderate | `src/lib.rs` (and/or `src/flush.rs`) |

## Unspecced Backfilled

| Feature | Location | Backfilled as |
|---------|----------|---------------|
| `Superblock::region_capacity_bytes()` | `src/on_disk.rs:142` | 001 spec FR-18 |
| `create_test_component_from_state()` | `src/test_support.rs:272` | 001 spec NFR-11 |
| `ExtendedMetadataStoreError::CapacityExhausted` (never constructed) | `../interfaces/src/iextended_metadata_store.rs:12` | 001 spec Known Gaps note; construction tracked by ALIGN-EMS-002 |

## Resolved (already fixed on the main thread — verified present)

| Item | Verification | Location |
|------|--------------|----------|
| NFR-07: `MockBlockDevice::read_write_stats` | `fn read_write_stats(&self) -> ReadWriteStats { ReadWriteStats::default() }` present | `src/test_support.rs:223` |
| SC-1/2/3/6/7: workspace membership | crate in `[workspace] members` and `[workspace.dependencies]` | root `Cargo.toml:23,105` |

## Not modified

- No `.rs` source was edited; `cargo` was not run (per policy).
- Only files under `components/extended-metadata-store/.specify/sync/` and
  `components/extended-metadata-store/specs/*/spec.md` were changed.
