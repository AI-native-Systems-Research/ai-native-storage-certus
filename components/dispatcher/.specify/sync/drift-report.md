# Spec Drift Report

Generated: 2026-07-21
Project: dispatcher (components/dispatcher)
Spec: specs/001-dispatcher-cache-interface/spec.md
Base commit: 833e9f36e01f1df8a0e0fc57d5cd223d823d3199 .. HEAD
Sources analyzed: src/lib.rs, src/pipeline.rs, src/cold_pool.rs

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 50 (active FR-* and SC-*; excluding 5 REMOVED FRs and 1 REMOVED SC) |
| Aligned | 48 |
| Drifted | 2 (FR-024, FR-037) |
| Not Implemented | 0 |
| Unspecced Code | 4 |
| Inter-Spec Conflicts | 0 |

## Per-Spec Findings — 001-dispatcher-cache-interface

### Aligned
All active requirements not listed under "Drifted" remain aligned with the code
(FR-001..FR-023, FR-025, FR-028..FR-036, FR-038..FR-050 and SC-001..SC-014). The
changes on this branch are additive robustness/concurrency guarantees layered on
top of the existing paths; they do not remove or contradict those requirements.

### Drifted

| Req | Spec says | Code now does | Severity |
|-----|-----------|---------------|----------|
| FR-024 | `evict_for_space` uses sparse-probe + **shard-targeted blind LRU primary** (`evict_lru_for_key(target_key)`); when no clean candidate is found it **blind-frees** the LRU victim and removes the dispatch-map entry if the BlockDevice transition fails ("data loss accepted"). | `evict_for_space` now frees one **pin-safe** victim per iteration via `evict_one_clean`, scanning a **widening** `oldest_keys(4×attempts, cap 1024)` window. Each candidate is demoted (`try_evict_to_block`) or, if unpinned, dropped (`dm.remove`); pinned candidates are skipped. The blind `evict_lru`/`evict_lru_for_key(target_key)` fallback is **removed** — `target_key` is now unused (`_target_key`). If every scanned candidate is pinned it returns `AllocationFailed` rather than free a slot an in-flight load points at. `evict_and_insert`'s fragmentation-relief path likewise uses `evict_one_clean` instead of blind `evict_lru_for_key`. | High |
| FR-037 | The dispatcher pre-allocates **a single warm CUDA stream** stored as an `AtomicU64` (`warm_stream`); "a single CUDA stream is used for GPU operations (lock-free access via atomic load). Multi-stream round-robin is reserved for future scaling." | The warm/pipeline paths now select **per-GPU-device** streams from a process-global `DEVICE_STREAMS` map (`device_streams_for`): one warm stream + one pipeline pair per device, created lazily on the target device. The device is resolved per request from the IPC destination pointer (`device_of_ptr`/`set_batch_device`) and made current before issuing the copy; the shared `warm_stream` AtomicU64 is now a fallback only. | Medium |

### Not Implemented
None.

## Unspecced Code

| Feature | Location | Suggested FR |
|---------|----------|--------------|
| Concurrent-promotion-race recovery: a `batch_lookup` cold promotion that loses the `mt.insert` race (`MemoryTierError::AlreadyExists`) is treated as a hit — mapped to `DispatcherError::AlreadyExists`, then a bounded-wait recovery pass serves the winner's resident slot to the GPU. | `lib.rs` `serve_concurrently_promoted`, `serve_memory_tier_to_gpu`, batch_lookup recovery post-pass; AlreadyExists error mapping | New **FR-051** |
| Per-GPU-device CUDA stream routing for multi-GPU / tensor-parallel: `DEVICE_STREAMS` map, `device_streams_for`, `set_batch_device`; `cold_pool::ColdReadRequest.gpu_device` + worker `set_device`. | `lib.rs` `DeviceStreams`/`DEVICE_STREAMS`/`device_streams_for`/`set_batch_device`; `cold_pool.rs` `ColdReadRequest.gpu_device` | New **FR-052** (also update FR-037) |
| Cold-load staging pool: bounded pool of pinned, pre-registered host DRAM buffers (`StagingPool`/`StagingLease`) used to serve cold reads uncached (`SSD→staging→GPU`) when the memory tier is saturated, instead of failing with `AllocationFailed`. Config `cold_staging_slots` (default 64), `cold_staging_buf_bytes` (default 4 MiB). | `pipeline.rs` `StagingPool`/`StagingLease`, `PipelineRing::new` staging arg; `lib.rs` `serve_cold_staged`, `promote_and_serve` staging fallback, batch_lookup staging post-pass | New **FR-053** (also update FR-033 config field list) |
| Cold-read drain-to-completion / no-early-break: pipelined cold paths drain until `completed == submitted` and use a `stop_submitting` flag on error instead of breaking early, preventing orphaned completions that deadlocked reused NVMe queues. | `pipeline.rs` `pipelined_ssd_to_gpu_zero_copy`, `pipelined_multi_object_zero_copy` | New **FR-054** (refines FR-019) |

## Inter-Spec Conflicts

None.

## Recommendations

1. **FR-024 (drift, High)** — Rewrite to match pin-safe eviction. The spec's documented
   "blind LRU / data-loss-accepted" fallback and shard-targeted `evict_lru_for_key(target_key)`
   no longer exist; the code is authoritative and tested (`eviction_never_frees_pinned_slot`).
   See proposal ALIGN-024.
2. **FR-037 (drift, Medium)** — Update to describe per-device streams (cross-reference FR-052);
   the single-`AtomicU64`-warm-stream model is now a fallback. See proposal ALIGN-037.
3. **FR-051..FR-054 (backfill)** — Add the four new requirements above. Code is
   authoritative and covered by regression tests. See proposals BACKFILL-051..054.
4. **FR-033 (config)** — Add `cold_staging_slots` (default 64) and `cold_staging_buf_bytes`
   (default 4 MiB) to the enumerated `DispatcherConfig` fields (folded into BACKFILL-053).
