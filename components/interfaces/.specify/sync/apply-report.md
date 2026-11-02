# Sync Apply Report — interfaces

> **Apply 2026-10-31T05:08:52Z (branch `specsync-a`).** Backup: `.specify/sync/backups/20261031T050852Z/` (`specs/001-interfaces/{spec,plan}.md` and the previous `sync/*`). Applied P-1 (FR-011 `memcpy_batch_async` entry), P-3 (Implementation Notes citation), the `Last Synced 2026-10-30` banner in `spec.md`, and P-2 (`plan.md` method counts and a recount note). Not applied: P-4 (ALIGN, not approved; task in `align-tasks.md`). No `src/` file edited. Stamp not written (the final pass stamps the merged tree). JSON: `apply-report.json` key `apply_2026_10_30`.


Applied: 2026-10-21T22:58:04Z (analysis run 2026-10-21) · Branch: `fix/dispatcher-remove-clear-mt` (HEAD `efd502331`) · Mode: batch, no questions
Approved by: Cornel, 2026-10-21 ("Approved by Cornel 2026-10-21 (groups 1, 3, 4)")
Backup: `.specify/sync/backups/20261021T225804Z/`. It holds `specs/001-interfaces/spec.md` and the previous `sync/{align-tasks.md, apply-report.md, apply-report.json, drift-report.md}`.

## Changes Made

### Specs Updated

Every BACKFILL edit uses the exact old→new text from `proposals.md` / `proposals.json`. Each `old` string was matched exactly once in `specs/001-interfaces/spec.md` (the edit tool rejects an ambiguous match). Insertions were placed at the anchors the proposals name.

| Proposal | Spec | Requirement | Change Type |
|---|---|---|---|
| 1 | 001 `spec.md` | FR-017: `Command` 12→13 variants (+`FlushSync`, with sync note) | Modified |
| 1 | 001 `spec.md` | FR-017: `Completion` 11→12 variants (+`FlushDone`) | Modified |
| 1 | 001 `spec.md` | FR-017: `ReadWriteStats` IO-size histograms, `IO_SIZE_BUCKETS`, `size_bucket`/`bucket_lower_bound`, `merge_from` | Modified |
| 2 | 001 `spec.md` | FR-018: `DispatcherConfig` 18→19 fields (+`store_backpressure_ms`) | Modified |
| 2 | 001 `spec.md` | FR-018: `TierEventStats` bullet after `LookupResult` | Added |
| 3 | 001 `spec.md` | FR-008: `copy_gpu_to_memory_async` takes `regions: &[IpcHandle]` (gather semantics) | Modified |
| 3 | 001 `spec.md` | FR-008: `batch_populate` method after `populate` | Added |
| 3 | 001 `spec.md` | FR-008: `tier_event_stats` and `schedule_write_through` methods after `read_write_stats` | Added |
| 3 | 001 `spec.md` | User Story 2 acceptance criterion 1: 16→22-method inventory | Modified |
| 4 | 001 `spec.md` | FR-008: `clear_memory_tier` result contract (pinned entries kept, `Timeout`, count; repair `cccfb328e`) | Modified |
| 4 | 001 `spec.md` | FR-008: `flush_to_ssd` result contract (`IoError` on a failed write-through; repair `9bd231d85`) | Modified |
| 8 | 001 `spec.md` | FR-023: `LookupConfig` 10→12 fields (+`caller_wait`, `connection_teardown_timeout`) | Modified |
| 9 | 001 `spec.md` | FR-021: `FormatParams` 10→9 fields, named | Modified |
| 10 | 001 `spec.md` | Overview: "two" → "three" Cargo features | Modified |
| 10 | 001 `spec.md` | Overview: `IMemoryTier` removed from the `spdk`-gated list, marked ungated | Modified |
| 10 | 001 `spec.md` | FR-009 heading: dropped "(feature: spdk)" | Modified |
| 11 | 001 `spec.md` | NFR-002: `gpu` feature gates nothing; `IGpuServices` `DmaBuffer` methods gated on `spdk` | Modified |
| 12 | 001 `spec.md` | Implementation Notes: verified-properties claim replaced (removed in `a93b5620f`; only `IGpuServices` retains one) | Modified |
| 13 | 001 `spec.md` | Backfill banner: FR-014/FR-025 "Deferred" note marked resolved 2026-08-20 | Modified |
| — | 001 `spec.md` | `> **Last Synced 2026-10-21**` banner line summarising this apply (skill step 2.5, "update Last Modified metadata") | Added |

### Applied by hand before this run (recorded as APPLIED, not re-applied)

Proposals 5 and 6 are approved ALIGN proposals that change only doc comments in `src/`. They were applied by hand in the working tree (`git diff -- components/interfaces/src/idispatcher.rs`). This apply did **not** edit `src/`. It checked that the working-tree doc text matches the proposed text word for word.

| Proposal | File | Change |
|---|---|---|
| 5 (ALIGN, doc) | `src/idispatcher.rs`, `clear_memory_tier` doc | "Empty the memory tier …". Pinned entries keep their slot and are retried. The count covers entries actually demoted or removed. New `# Errors` section: `Timeout`. |
| 6 (ALIGN, doc) | `src/idispatcher.rs`, `batch_lookup` doc | "Returns one [`LookupOutcome`] per input entry, in the same order: the result together with how the key was served (`served_by`)." |

`cargo doc -p interfaces --no-deps --features spdk` was **not** run by this apply. Run it before merging.

### New Specs Created
- None.

### Supersede
- None proposed.

### Implementation Tasks Generated
- No new task for an approved ALIGN proposal. The two approved ALIGN proposals (5, 6) were already applied by hand. `align-tasks.md` records them as **DONE**, with the `cargo doc` check left open.

### Open items recorded (not approved, with an `approval_note`)
- **7** (major): `IDispatcher` `remove` / `clear_memory_tier` / `flush_to_ssd` contract in **`dispatcher-p2p`**. The task belongs to component `dispatcher-p2p`, not `interfaces`. It is recorded in **this component's** `align-tasks.md` because the contract it violates is the `IDispatcher` doc in this crate. The dispatcher apply entered it earlier in this run, and this apply confirmed it. Quoted note: *"TRACK (Cornel 2026-10-21): port the 3 dispatcher repairs to dispatcher-p2p in a separate PR with its own Loom tests and spec sync."*
- **14** (moderate): NFR-004 runnable doc examples → `align-tasks.md`, quoting *"TRACK (Cornel 2026-10-21): runnable doc examples, later doc task."* The BACKFILL alternative (soften NFR-004) was not chosen, so NFR-004 is unchanged.

### Not Applied
| Proposal | Reason |
|---|---|
| 7 | Not approved (TRACK; separate `dispatcher-p2p` PR) |
| 14 | Not approved (TRACK; later doc task). Its BACKFILL alternative was not chosen either. |

### Files modified by this apply
- `specs/001-interfaces/spec.md`
- `.specify/sync/align-tasks.md`, `.specify/sync/apply-report.{md,json}`, `.specify/sync/drift-report.md` (one note below the frontmatter; `spec_sync_*` frontmatter not touched)
- No source files. `proposals.{md,json}` and `drift-report.json` were not changed.

## Next Steps

1. Review the updated spec: `git diff -- components/interfaces/specs`.
2. Run `cargo doc -p interfaces --no-deps --features spdk` (proposals 5/6) and SC-2/3/4/7, which were not re-executed in the analysis.
3. Commit: `git add components/interfaces/specs components/interfaces/.specify/sync components/interfaces/src/idispatcher.rs && git commit -m "sync(interfaces): apply 1-6, 8-13"`.
4. Open the `dispatcher-p2p` repair (interfaces-7) and the doc-examples task (interfaces-14).
