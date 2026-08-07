# Spec Sync Apply Report

Generated: 2026-07-22
Mode: AUTO-BACKFILL
Spec: components/dispatcher/specs/001-dispatcher-cache-interface/spec.md
Backup: components/dispatcher/.specify/sync/backups/spec.md.bak.20260722T162308
Base drift report: components/dispatcher/.specify/sync/drift-report.{json,md} (generated 2026-07-22, supersedes
2026-07-21 report)

## Pre-check: verifying drift report against current spec.md

Before applying, the current `spec.md` (as of the previous sync commit `bb427f1`, 2026-07-21) was diffed against
the drift report's claims:

- **FR-034** — genuinely stale. `spec.md` still described a single `ClientChannels` cached per drive at
  `initialize()`. Code (`327306b`, 2026-07-22) replaced this with a per-drive `ChannelPool`/`ChannelLease`. **Applied
  as BACKFILL.**
- **FR-024** — the requirement bullet in `spec.md` already matched the pin-safe `evict_one_clean` implementation
  (amended by the prior sync, `bb427f1`, 2026-07-21). The drift report's "Spec says" column quotes stale text that
  does not match the checked-out file. However, several **companion narrative sections** (User Story 7 body +
  acceptance scenarios, an Edge Case bullet, and two Clarifications-session answers from 2026-05-22/2026-05-08)
  still described the old sparse-probe/shard-targeted/blind-LRU model. **Applied as BACKFILL (companion-doc fix
  only; FR-024 bullet itself unchanged).**
- **FR-037** — same situation: the requirement bullet already matched per-device `DEVICE_STREAMS` (amended by
  `bb427f1`). No companion narrative elsewhere referenced the old single-stream model. **No text change needed**
  beyond confirming alignment.
- **Partition-table guard** — genuinely unspecced. Code (`3db1e6c`, 2026-07-21) added an `EXPECTED_PARTITIONS`
  guard in `initialize()`; no FR covered it. **Applied as BACKFILL (new FR-055).**

## Specs Updated

| Requirement / Section | Change Type | Summary |
|---|---|---|
| FR-034 | Amend (backfill, High drift) | Single per-drive `Option<ClientChannels>` cached at init -> per-drive `ChannelPool`/`ChannelLease`: pool starts empty, grows lazily, each concurrent reader (`promote_and_serve`, `serve_cold_staged`, per-drive prefetch threads) checks out an exclusive RAII-leased channel; checkout drains stale completions; `connect_client()` runs outside the pool lock. Documented the completion-theft hang the pool fixes. |
| FR-055 | Add (backfill) | NEW: partition-table compatibility guard — `initialize()` validates ≥3 Certus partitions before indexing `table.partitions[0]`/`[2]`, returning `DispatcherError::IoError` with remediation guidance instead of panicking on a valid-but-non-Certus GPT. Cross-referenced as a refinement of FR-025. |
| User Story 7 (narrative + AS1-3) | Amend (companion-doc fix, no FR change) | Rewrote to describe pin-safe `evict_one_clean` / widening `oldest_keys` scan / demote-or-drop / never-free-pinned, replacing the sparse-probe + blind-LRU-primary description. Removed stale `MAX_ATTEMPTS=512` in favor of configurable `max_attempts` (default 2048). |
| Edge Cases (2 bullets) | Amend (companion-doc fix, no FR change) | `evict_for_space` exhaustion bullet: corrected `MAX_ATTEMPTS=512` -> configurable `max_attempts` (default 2048) and clarified it never blind-frees. Blind-LRU-fallback bullet rewritten to describe demote-or-drop-when-unpinned / skip-when-pinned / `AllocationFailed` when all pinned. |
| Edge Cases (new bullet) | Add (backfill, cross-ref FR-055) | NEW: partition-table guard behavior on a non-Certus GPT. |
| Clarifications — Session 2026-05-22, 2026-05-08 | Amend (superseded pointers, no FR change) | Annotated 3 stale Q&A answers (sparse-probe/shard-targeted eviction description) with `*(superseded 2026-07-20 — see Session 2026-07-22 ...)*` pointers rather than deleting the historical record. |
| Clarifications — new Session 2026-07-22 | Add (backfill) | NEW session with 3 Q&A entries summarizing pin-safe eviction (FR-024), the `ChannelPool` rationale (FR-034), and the partition-table guard (FR-055) as the current, authoritative answers. |

FR-024 and FR-037's requirement-bullet text was left unmodified — both already reflect the current implementation
(applied in the prior sync, `bb427f1`, 2026-07-21).

## SUPERSEDE

None. No spec directory required a superseding banner.

## NEW_SPEC

None. The `ChannelPool` and partition-table guard are refinements of the existing 001 spec's FR-034/FR-025, not
separate features — backfilled as amendments/additions within `spec.md` rather than a new spec directory.

## ALIGN / DEFECT / AMBIGUOUS (deferred to align-tasks.md)

None. The drift report contains no ALIGN/DEFECT/AMBIGUOUS-classified items for this component — only three
BACKFILL-eligible drift items (FR-034/024/037, all confirmed intentional code evolutions per report and user
instruction) and unspecced-feature rows, all of which were resolved above or were already covered by the prior
sync (FR-051..FR-054). `components/dispatcher/.specify/sync/align-tasks.md` was not created (nothing to append).

## Observation (not actioned)

The drift report's `drift-report.json`/`.md` "spec says" quotes for FR-024 and FR-037 do not match the text
actually present in `spec.md` at generation time (they describe the pre-`bb427f1` wording). This looks like a
stale-snapshot artifact in report generation rather than real spec drift; flagging for awareness, no spec or code
change made on the basis of it beyond the companion-doc fixes above, which were independently verified against the
live `spec.md` content.

## Inter-Spec Conflicts

None. Single spec directory for this component.

## Notes

- Active requirement count after apply: FR-001..FR-055 (5 REMOVED: FR-020/021/022/026/027) and SC-001..SC-015
  (1 REMOVED: SC-008).
- Only Markdown under `components/dispatcher/specs/**` and `.specify/sync/**` was modified. No source code
  (`src/**`) was touched.
- Backup of `spec.md` prior to this apply: `components/dispatcher/.specify/sync/backups/spec.md.bak.20260722T162308`.

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`)

Mode: **fully-interactive** (per-component approval via the drift-sweep workflow).
Drift source: `.specify/sync/drift-report.{json,md}` (generated 2026-08-07).
Pre-edit backups: `.specify/sync/backups/20260807T160256Z/{spec.md, idispatcher.rs}` (from git HEAD).
Nothing committed to `unstable` — all changes staged on the feature branch.

## User Decisions Driving This Pass

- **API drift (FR-001, FR-039, FR-042, config fields, primitives)** = **Backfill all to spec**.
- **Phantom Creusot proofs in `idispatcher.rs`** = **Soften doc to match reality**.

## Changes Made

### Specs Updated (BACKFILL — applied directly)

| Requirement | Change |
|-------------|--------|
| Header | Added "Last Synced 2026-08-07" note summarizing this sweep. |
| FR-001 | Expanded the method inventory to the full shipped `IDispatcher` surface (added `reserve_memory`, `copy_gpu_to_memory_async`, `copy_gpu_to_memory_completed`, `release_memory`, `pin`, `unpin`, `flush_to_ssd`, `read_write_stats`), grouped by role; noted `create_eviction_channel`/`eviction_dropped_count` are inherent methods on the concrete component, not trait methods. |
| FR-039 | Signature backfilled to `batch_lookup(entries: &[(CacheKey, Vec<IpcHandle>)])` with a multi-region-scatter note. |
| FR-042 | Signature backfilled to `create_eviction_channel(capacity: usize)`. |
| FR-033 | Extended with `metadata_partition_size` (u64, 128 MiB), `extended_metadata_partition_size` (u64, 128 MiB), and `backfill_delay_ms` (u64, 10 — noted p2p-only, inert for local caching). |
| **FR-056 (new)** | Documents the GPU-staged memory-lifecycle primitives (`reserve_memory`/`copy_gpu_to_memory_async`/`copy_gpu_to_memory_completed`/`release_memory`/`pin`/`unpin`) and the durability/introspection methods (`flush_to_ssd`, `read_write_stats`). |

### Code Doc Corrected on Branch (ALIGN — doc-only, no behavior change)

| File | Change |
|------|--------|
| `components/interfaces/src/idispatcher.rs` | Softened the "Verified Properties" block comment: reframed P1–P10 as **informal design invariants** (exercised by tests, not machine-checked), explicitly recorded that the previously-advertised Creusot proof tree at `components/dispatcher/verif/` ("24 verification conditions discharged by SMT solvers") **does not exist** and the claim was removed. Changed all 16 per-method `# Verified: Pn` doc headings to `# Design invariants (informal, not machine-checked): Pn`. Also completed the P9/P10 list entries that were previously cited only in method docs. |

## Verification

- `cargo build -p interfaces` — **clean** (doc-comment edits only).
- Dispatcher crate (`dispatcher-v1`) requires SPDK + hardware and was not built in this pass; the interface edits are comment-only and cannot change codegen.

## Not Applied / Deferred

| Item | Reason |
|------|--------|
| Restoring real Creusot proofs | Out of scope; noted in the softened doc as possible future work. Per the soften-doc decision, docs were corrected rather than proofs restored. |

## Next Steps

1. Review the softened `idispatcher.rs` doc comment and the six spec edits on the branch.
2. Commit on `sync/spec-drift-sweep-20260807` (do NOT commit to `unstable`).
