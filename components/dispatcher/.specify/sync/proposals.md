# Drift Resolution Proposals — dispatcher

Generated: 2026-10-31T06:30:00Z · Branch: `specsync-b` (HEAD `0a71990e`, base `a0292be6`) · Based on: `drift-report.json` of the same run.

BACKFILL proposals are approved by the brief's rule (spec text updated to match reviewed, tested code on this branch); B-33 was approved by the user via the lead. H-1 is a judgment call and is not approved.

| Id | Requirement | Before | After | Approved |
|---|---|---|---|---|
| B-25 | 001 FR-039 step (2) | a multi-region cold entry's serve_memory_tier_to_gpu sync on the warm stream also waits for queued warm copies | cold entries do not use the warm stream (in-loop pipe-stream delivery, FR-061); only a staged multi-region entry syncs on warm | yes |
| B-26 | 001 FR-019 | scatter-gather makes one final dual pipe-stream sync | + multi-region object gets one piece batch; final sync is checked and fails every issued object | yes |
| B-27 | 001 Clarification Q&A (pipelined reader) | one final dual pipe-stream sync | one final, checked dual sync; multi-region piece copies batched in the loop | yes |
| B-28 | 001 Key Entities: Pipelined Reader | one final dual pipe-stream sync covers the batch | + checked; multi-region job via ColdReadJob::regions / queue_slot_to_regions | yes |
| B-29 | 001 Implementation Notes | cold pipeline uses coalesced per-object memcpy_h2d_async | + multi-region object: one batched piece copy on the pipe stream | yes |
| B-30 | 001 US-11 acceptance scenario 4 | error reported for that entry only | + exception: failed final sync fails every issued entry | yes |
| B-31 | 001 FR-053 | (multi-region staged delivery unstated) | single-region fused SSD->GPU; multi-region reads into staging then serve_memory_tier_to_gpu scatter on warm, synced, before the lease is returned; not part of in-loop delivery | yes |
| B-32 | 001 FR-006 | (zero-size-region rule unstated) | copies clamped to the slot; zero-size region gets no bytes but does not stop the scatter; only an exhausted slot does | yes |
| B-33 | 001 data-model.md (CacheKey, DispatcherConfig, BackgroundWriteJob, PendingWrite, Cache Entry Lifecycle, Dispatcher Lifecycle, Relationships) | v0 model: prepare_store/commit_store/cancel_store, [Staging] state, metadata block device, removed config fields | states/transitions/receptacles as the code has them (approved by the user via the lead) | yes |
| B-34 | 002 spec.md / contracts/served-by.md / research.md shm-queue citations; FR-019 | translate.rs:573, wire.rs:98-110, translate.rs:749-760 | translate.rs:1152, wire.rs:194-205, translate.rs:1149-1159; FR-019 note: LOOKUP_BLOCKS/LOOKUP_CHUNKS return the same byte | yes |
| H-1 | 001 contracts/idispatcher.md (whole file) | v0 IDispatcher contract: prepare_store/commit_store/cancel_store, dma_copy_to_host/to_device, 5-field DispatcherConfig, no memory tier, no batch/async/store-lifecycle methods | option A: rewrite against components/interfaces/src/idispatcher.rs; option B: mark the file historical and point at the interface source and FR-001 | no |

Open items carried over unchanged in `align-tasks.md`: A-1…A-5, A-01, A-02, A-04, A-05, A-06, A-08, A-10, N-01.

Previous proposals: `backups/20261031T054807Z/proposals.md.prev`.
