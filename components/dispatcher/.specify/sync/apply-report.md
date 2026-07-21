# Spec Sync Apply Report

Generated: 2026-07-21
Spec: components/dispatcher/specs/001-dispatcher-cache-interface/spec.md
Backup: components/dispatcher/.specify/sync/backups/spec.md.bak
Base commit: 833e9f36e01f1df8a0e0fc57d5cd223d823d3199 .. HEAD

All 6 proposals (ALIGN-024, ALIGN-037, BACKFILL-051, BACKFILL-052, BACKFILL-053,
BACKFILL-054) were approved and applied to spec.md.

## Specs Updated

| Requirement | Change Type | Before -> After (summary) |
|-------------|-------------|---------------------------|
| FR-019 | Refine (backfill link) | Pipeline description unchanged; added a sentence noting the zero-copy paths drain to completion and do not break early on error (cross-reference FR-054). |
| FR-024 | Amend (align, High drift) | Sparse-probe + shard-targeted blind-LRU-primary (`evict_lru_for_key(target_key)`) with blind-LRU fallback removing dispatch-map entry on failure ("data loss accepted") -> pin-safe `evict_one_clean` per iteration over a widening `oldest_keys(4×attempts, cap 1024)` scan; demote (`try_evict_to_block`) or drop (`dm.remove`) only unpinned entries; transition before DRAM free; blind fallback REMOVED (`target_key` unused); returns `AllocationFailed` when all candidates pinned (caller leaves uncached or uses staging FR-053); `evict_and_insert` fragmentation relief also pin-safe. |
| FR-033 | Amend (config) | Added `cold_staging_slots` (usize, default 64) and `cold_staging_buf_bytes` (usize, default 4 MiB) to the enumerated `DispatcherConfig` fields; noted `cold_staging_slots = 0` disables the staging pool. |
| FR-037 | Amend (align, Medium drift) | "Single warm CUDA stream (AtomicU64); multi-stream reserved for future" -> AtomicU64 `warm_stream` is now a fallback; warm hot path + D2H populate resolve the destination GPU device from the IPC pointer and use the per-device warm stream from `DEVICE_STREAMS` (cross-reference FR-052). |
| FR-051 | Add (backfill) | NEW: concurrent-promotion-race recovery — a `batch_lookup` promotion losing the `mt.insert` race (`AlreadyExists`) is served warm from the winner's resident slot after a bounded wait, instead of failing. |
| FR-052 | Add (backfill) | NEW: per-GPU-device CUDA stream routing (`DEVICE_STREAMS`, `device_streams_for`, `set_batch_device`, `ColdReadRequest.gpu_device`) for multi-GPU / tensor-parallel loads. |
| FR-053 | Add (backfill) | NEW: bounded cold-load staging pool (`StagingPool`/`StagingLease`) serves cold reads uncached (`SSD→staging→GPU`) when the tier is saturated instead of failing `AllocationFailed`; config fields via FR-033. |
| FR-054 | Add (backfill, refines FR-019) | NEW: cold-read drain-to-completion — pipelines drain until `completed == submitted` using a `stop_submitting` flag on error, never breaking early (prevents orphaned completions that deadlocked reused NVMe queues). |
| SC-015 | Add (backfill) | NEW success criterion: concurrent `batch_lookup` promotions of the same cold key both succeed (loser served the winner's data) rather than failing with `AlreadyExists`. |

## Inter-Spec Conflicts

None.

## Notes

- Active requirement count after apply: FR-001..FR-054 (5 REMOVED: FR-020/021/022/026/027)
  and SC-001..SC-015 (1 REMOVED: SC-008).
- Companion artifacts (US7 acceptance scenarios, Edge Cases, and the 2026-05-22 / 2026-05-08
  clarifications) still reference the old blind-`evict_lru` / shard-targeted `evict_lru_for_key`
  wording; FR-024 is now authoritative. A follow-up pass may reconcile those narrative sections.
