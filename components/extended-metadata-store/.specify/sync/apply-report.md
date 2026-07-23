# Spec-Sync Apply Report: extended-metadata-store

**Mode**: AUTO-BACKFILL
**Applied**: 2026-07-22
**Source**: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T21:30:28Z)
**Backups**: `.specify/sync/backups/20260722T232035Z/{spec,plan,tasks}.md`

## Scope

Only Markdown under `components/extended-metadata-store/specs/**` and
`.specify/sync/**` was modified. No source code (`src/**`, `tests/**`) or
`Cargo.toml` was touched, per HARD RULES.

## Backfilled (code -> spec)

1. **spec.md — SC-1 test count**: "All 8 unit tests" -> "All 9 unit tests"
   (`src/lib.rs` has 9 `#[test]` fns; `dirty_count_increments` was
   uncounted).
2. **plan.md — `on_disk.rs` test inventory**: "5 tests" -> "6 tests";
   corruption detection split into `superblock_corrupt_rejected` and
   `entry_record_corrupt_rejected`.
3. **plan.md — `integration_ssd.rs` test inventory**: "12 tests" -> "14
   tests".
4. **NFR-05 internal conflict (spec.md vs plan.md vs tasks.md T057)
   resolved in docs**: NFR-05 wording in spec.md reworded to distinguish
   gated persistence I/O modules (`block_io`/`flush`/`recovery`/
   `test_support`, `#[cfg(feature = "testing")]`) from the always-compiled
   `on_disk.rs` (pure format definitions, no I/O). plan.md's Module
   Dependency Graph annotated with the same rationale. tasks.md T057 marked
   resolved, pointing back here. This is a documentation-only resolution —
   `on_disk.rs`'s always-compiled status in the code is unchanged and is now
   correctly described as intentional rather than left as an open
   contradiction.
5. **Stale companion docs**: plan.md's "Running Tests" section and
   spec.md's Success Criteria section each gained a note that
   `cargo test -p extended-metadata-store` (and clippy/doc) currently fail
   because the crate is outside the workspace — pointing to align-tasks.md
   ALIGN-001 rather than silently leaving the commands looking functional.
6. **spec.md FR-05 / User Story 6 / Feature Gating section**: status
   annotated "Partially Implemented" with an explicit "Known Gaps" note
   describing the `force_flush()` no-op behavior and pointing to
   align-tasks.md ALIGN-002. The requirement text itself (intended
   behavior) was left unchanged, since the gap is a code defect to be
   fixed, not a behavior to backfill as "correct."
7. **plan.md Data Flow: Put + Flush**: annotated that `force_flush()` is
   not actually wired into the flush path; the working path today is
   `FlushManager::trigger_flush()` / `flush::flush_to_disk()` via the FR-17
   API.

## New Requirement Added (from Unspecced Code)

- **FR-17** added to spec.md, documenting the external persistence-wiring
  API (`initialize_from_client`, `snapshot_entries`, `mark_flushed`,
  `load_entries`, `dirty_count()`, `flush_seq()` on
  `ExtendedMetadataStoreComponent`) — the actual mechanism by which callers
  achieve recovery-on-startup and flush-on-demand today, previously only
  described informally via plan.md's Data Flow diagrams. Added to the
  Interface Contracts section as an "Inherent API" alongside the
  `IExtendedMetadataStore` trait. This is a genuine part of the same
  feature (not a separate spec) — NEW_SPEC was not used.

## NOT Backfilled — Sent to align-tasks.md (Code Defects)

Per HARD RULES, these are NOT resolved by rewriting spec text to match
current behavior. Both require an actual code change and are logged in
`.specify/sync/align-tasks.md`:

| Task ID | Severity | Summary |
|---|---|---|
| ALIGN-001 | MAJOR | `components/extended-metadata-store` is absent from root `Cargo.toml` `[workspace] members` — crate is unbuilt/untested by `cargo build`/`cargo test --all`/CI. |
| ALIGN-002 | MAJOR | `IExtendedMetadataStore::force_flush()` is an unconditional no-op in all build configurations (`src/lib.rs:177-184`), contradicting FR-05 and the interface doc comment. |

## SUPERSEDE

None. No spec was superseded — this is the only spec (`001-extended-metadata-store`) for this component and it remains current.

## Unresolved / Deferred

None deferred — all 5 drifted items, the 1 unspecced feature, and the 1
spec-internal conflict from drift-report.json were either backfilled into
docs or routed to align-tasks.md as code defects.

## Verification Note

Because of ALIGN-001, none of `cargo test -p extended-metadata-store`,
`cargo clippy -p extended-metadata-store -- -D warnings`, or
`cargo doc -p extended-metadata-store --no-deps` could be run to confirm
doc/test consistency as part of this pass. Once ALIGN-001 is resolved,
re-run `speckit.sync.analyze` for this component to confirm the backfilled
test counts (9 / 6 / 14) and NFR-05 wording stay accurate.
