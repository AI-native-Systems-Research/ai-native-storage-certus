# Sync Apply Report

Applied: 2026-04-17

## Changes Made

### Specs Updated

| Spec | Proposal | Requirement | Change Type | Description |
|------|----------|-------------|-------------|-------------|
| 001 | 1 | FR-003 | Modified | Removed namespace_id from persisted metadata |
| 001 | 2 | FR-006 | Modified | Exclusive lock → read lock for iteration |
| 001 | 3 | FR-007 | Modified | Removed size class range constraint |
| 001 | 4 | FR-012 | Modified | Removed reopen; fresh init only |
| 001 | 5 | Key Entities | Modified | Replaced Superblock with On-Disk Layout |
| 001 | 10 | FR-017 | Added | DMA allocator via set_dma_alloc() |
| 001 | 11 | Multiple | Modified | Removed all namespace_id references |
| 001 | 12 | US7/Scenario5 | Moved | Reopen scenario moved from US7 to US5 |
| 002 | 8 | FR-003 | Removed | --ns-id removed from spec |

### New Specs Created

(none)

### Implementation Tasks Generated

1 task in `.specify/sync/align-tasks.md`:

1. **Wire Logger to ExtentManager in Benchmark App** (small) — Bind logger to extent manager's ILogger receptacle in benchmark app. Proposal 9, spec 002/FR-009.

### Not Applied

| Proposal | Reason |
|----------|--------|
| 6 (FR-008, FR-009, US5) | Rejected — crash recovery kept in spec for future implementation |
| 7 (FR-015) | Approved as keep — stays in spec tied to crash recovery |

### Backups

- `001-extent-management-spec.md.bak` → `.specify/sync/backups/`
- `002-extent-benchmark-spec.md.bak` → `.specify/sync/backups/`

## Next Steps

1. Review updated specs: `specs/001-extent-management/spec.md`, `specs/002-extent-benchmark/spec.md`
2. Implement alignment task in `.specify/sync/align-tasks.md` (wire logger to extent manager in benchmark app)
3. Commit changes: `git add specs/ .specify/sync/ && git commit -m "sync: apply drift resolutions 2026-04-17"`
