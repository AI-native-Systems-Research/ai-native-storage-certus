# Sync Apply Report — dispatcher-p2p

Applied: 2026-11-11T21:14:07Z · Branch `copilot/fix-dispatcher-lba-issues` · HEAD `adacb8ce1` · Mode: interactive · Backup: `backups/20261111T211120Z/`

Approved interactively by the user in /component-sync-specs --interactive, 2026-11-11. Every proposal is `approved: true`. Before each replacement, the old text was checked to occur exactly once in its file. Every check passed.

## Specs Updated

| Proposal | Requirement | Change Type |
|---|---|---|
| P-P2P1 | FR-037 (new) | Added |

Files: `specs/001-gpudirect-cold-path/spec.md`. No `.rs` source was modified.

## Implementation Tasks Generated

- **A-P2P-LBA** (in `align-tasks.md`): add a mock-drive test for the p2p read and write paths that asserts the base LBA is applied.

## Not Applied

None.

## Follow-up 2026-11-11 — A-P2P-LBA resolved

- `src/lib.rs` (tests only): added `LbaDisk` and `BaseEm` mocks and the tests `write_through_uses_data_partition_base_but_records_relative_offset` and `promote_to_memory_tier_reads_at_data_partition_base`. `MockMemoryTier` gained a `dma_capable` flag.
- `specs/001-gpudirect-cold-path/spec.md` FR-037: the "not covered by a test" bullet now names the tests (backup of the pre-follow-up spec is the committed `3228adabd` version).
- `align-tasks.md`: A-P2P-LBA marked ✅ RESOLVED.
