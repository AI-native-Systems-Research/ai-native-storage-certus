# Spec Sync Proposals

Generated: 2026-07-21
Spec: components/dispatcher/specs/001-dispatcher-cache-interface/spec.md
Base commit: 833e9f36e01f1df8a0e0fc57d5cd223d823d3199 .. HEAD

Code is authoritative and covered by regression tests (`eviction_never_frees_pinned_slot`,
`batch_lookup_recovers_from_concurrent_promotion_race`). Direction is therefore BACKFILL
(spec follows code) for new behaviors, and ALIGN (rewrite an existing FR to match code)
for the two drifts. All proposals are unapproved; check the box to approve.

---

## ALIGN-024 — Pin-safe memory-tier eviction (rewrite FR-024)

- **Direction**: ALIGN (rewrite existing FR-024 to match tested code)
- **Confidence**: High
- **Drift ref**: FR-024 (High)

**Current State** — FR-024 documents a sparse-probe + shard-targeted blind-LRU-primary
algorithm using `evict_lru_for_key(target_key)`, with a blind-LRU fallback that removes the
dispatch-map entry when the BlockDevice transition fails ("data loss accepted"). That path,
and the `target_key` shard-targeting, no longer exist in the code.

**Proposed Resolution** — Replace FR-024 body with:

> **FR-024**: Eviction in v1 is purely capacity-based within the memory-tier pool and MUST
> be pin-safe. When the pool is full, `evict_for_space(dm, mt, needed, _target_key, max_attempts)`
> frees one victim per iteration via `evict_one_clean`, which scans `IMemoryTier::oldest_keys(scan)`
> where the scan widens as pressure persists (`MAX_SCAN=4 × attempts`, capped at 1024) to look
> past pinned/unpersisted entries deeper in the LRU. For each candidate, in order of preference:
> (1) **demote** to BlockDevice via `IDispatchMap::try_evict_to_block` (write-through complete,
> unpinned) followed by `IMemoryTier::remove`; (2) otherwise, if unpinned, **drop** it entirely
> via `dm.remove` + `mt.remove` (write-through incomplete — the block is lost from cache and
> recomputed on next miss). In BOTH branches the dispatch-map transition happens BEFORE the DRAM
> slot is freed, and both dispatch-map operations reject entries with active read/write references,
> so eviction NEVER frees a slot a pinned in-flight load still points at. The prior blind
> `evict_lru`/`evict_lru_for_key(target_key)` fallback (which could reclaim a pinned slot and
> corrupt a concurrent load) has been REMOVED; `target_key` is unused. If every scanned candidate
> is pinned, `evict_for_space` returns `AllocationFailed` rather than blind-free; the caller leaves
> the block uncached (`populate`) or serves it via the staging pool (FR-053). The loop is bounded
> by `max_attempts` (`DispatcherConfig::max_eviction_attempts`, default 2048). `evict_and_insert`'s
> shard-fragmentation relief path likewise force-evicts one pin-safe victim via `evict_one_clean`
> and fails rather than blind-free when none is evictable. Count-based TSC eviction (from v0) is NOT used.

Also update US7 acceptance scenarios 1-3, the related Edge Cases bullets, and the Session
2026-05-22 / 2026-05-08 clarifications that reference blind `evict_lru` and shard-targeted
`evict_lru_for_key(target_key)` to reflect pin-safe eviction (no blind-free, AllocationFailed on
all-pinned).

**Rationale** — The blind-free path was a data-corruption bug under tensor parallelism (freeing a
DRAM slot an in-flight load still pointed at → invalid H2D DMA / crash). The fix is intentional
and regression-tested; the spec's "data loss accepted" fallback is now factually wrong.

- [ ] Approve

---

## ALIGN-037 — Per-device warm stream (update FR-037)

- **Direction**: ALIGN (update existing FR-037; cross-reference FR-052)
- **Confidence**: High
- **Drift ref**: FR-037 (Medium)

**Current State** — FR-037 states a single warm CUDA stream is pre-allocated and stored as an
`AtomicU64`, and that multi-stream is reserved for the future. The code now selects a warm stream
per destination GPU device.

**Proposed Resolution** — Amend FR-037 to add:

> The single `AtomicU64` warm stream (`warm_stream`) is retained as a fallback, but the warm
> memory-tier hot path (`lookup_async`, `batch_lookup`) and the D2H populate path now resolve the
> destination GPU device from the client's IPC pointer and use the per-device warm stream from the
> process-global `DEVICE_STREAMS` map (see FR-052). A CUDA stream is bound to the device that was
> current at creation, so a stream on another GPU makes `cudaMemcpyAsync` fail with "invalid
> argument" under multi-GPU / tensor parallelism; per-device streams avoid this.

**Rationale** — Multi-GPU (tensor-parallel) support forced the single-stream model to become
per-device. Code is authoritative.

- [ ] Approve

---

## BACKFILL-051 — Concurrent-promotion-race recovery

- **Direction**: BACKFILL (new FR)
- **Confidence**: High

**Current State** — Unspecced. `batch_lookup` (FR-039) does not describe what happens when two
concurrent lookups both classify the same key as cold and both try to promote it.

**Proposed Resolution** — Add:

> **FR-051**: When a `batch_lookup` cold promotion loses the `IMemoryTier::insert` race to a
> concurrent lookup for the same key (receiving `MemoryTierError::AlreadyExists`), the dispatcher
> MUST treat this as a hit rather than a failure. The error is mapped to `DispatcherError::AlreadyExists`
> and, in a post-classification recovery pass (`serve_concurrently_promoted`), the dispatcher
> bounded-waits (up to 5s, 50µs backoff) for the winning promoter to transition the dispatch-map
> entry to `MemoryTier`, then serves the resident slot to the GPU via `serve_memory_tier_to_gpu`
> (per-device warm stream, FR-052). If the entry is still `BlockDevice` at timeout it returns
> `KeyNotFound`; `MismatchSize` returns `InvalidParameter`. The caller's load pin keeps the entry
> from being evicted while waiting. This prevents spurious `AlreadyExists` load failures when
> sibling tensor-parallel ranks request the same content-hash key concurrently.

**Rationale** — Regression-tested (`batch_lookup_recovers_from_concurrent_promotion_race`);
prevents an engine crash under TP. Add SC-015: "Concurrent `batch_lookup` promotions of the same
cold key both succeed — the loser is served the winner's promoted data rather than failing."

- [ ] Approve

---

## BACKFILL-052 — Per-GPU-device CUDA stream routing (multi-GPU)

- **Direction**: BACKFILL (new FR; paired with ALIGN-037)
- **Confidence**: High

**Current State** — Unspecced. The spec assumes a single GPU/stream.

**Proposed Resolution** — Add:

> **FR-052**: Cold (SSD→GPU) and warm (memory-tier→GPU) DMA transfers MUST use CUDA streams bound
> to the destination GPU's device. Because `cudaMemcpyAsync` rejects a peer pointer on a different
> device than its stream ("invalid argument") under multi-GPU / tensor parallelism, the dispatcher
> maintains per-device streams in a process-global `DEVICE_STREAMS` map — one warm stream plus one
> pipeline stream pair per device, created lazily on the target device via `device_streams_for`. It
> resolves each request's device from its IPC destination pointer (`IGpuServices::device_of_ptr` /
> `set_batch_device`), makes that device current on the issuing thread, and selects the streams bound
> to it. `batch_lookup` resolves one device for the whole batch (all entries in an RPC come from a
> single rank); `cold_pool::ColdReadRequest` carries a `gpu_device: i32` field so the pool worker calls
> `IGpuServices::set_device` and uses device-bound streams. When the device is unknown (`-1`) the paths
> fall back to the shared pipeline-ring / `warm_stream` streams.

**Rationale** — Required for tensor-parallel deployments spanning multiple local GPUs. Code is
authoritative. Depends on `IGpuServices::set_device` / `device_of_ptr` (mocked in tests).

- [ ] Approve

---

## BACKFILL-053 — Cold-load staging pool

- **Direction**: BACKFILL (new FR; also updates FR-033)
- **Confidence**: High

**Current State** — Unspecced. Previously a cold load that could not obtain a memory-tier slot
failed with `AllocationFailed` (crashing the vLLM client under a burst of concurrent cold reads).

**Proposed Resolution** — Add:

> **FR-053**: The dispatcher MUST provide a bounded cold-load staging pool (`StagingPool`) of
> `cold_staging_slots` pre-registered, CUDA-pinned + SPDK-registered host DRAM buffers of
> `cold_staging_buf_bytes` each, leased via an RAII `StagingLease` that returns the buffer on drop.
> When a cold (BlockDevice) load cannot obtain a memory-tier slot because the tier is saturated
> (`evict_for_space` / `evict_and_insert` returns `AllocationFailed` and at least one data drive is
> configured), the dispatcher MUST serve the read through a staging buffer (`SSD → staging → GPU` via
> `serve_cold_staged`) and leave the dispatch-map entry as `BlockDevice` (served uncached, NOT promoted)
> rather than failing the load. In `batch_lookup` such entries are deferred to a post-pass that runs
> after the pooled tier jobs and serves them one lease at a time while holding no other lock, so the
> blocking `checkout` is deadlock-free. This bounds staging memory so a burst of concurrent cold reads
> cannot exhaust the memory tier and cause fatal `AllocationFailed`. Setting `cold_staging_slots` to 0
> disables staging.

> **FR-033 (amend)**: `DispatcherConfig` MUST additionally include `cold_staging_slots` (usize,
> default 64) and `cold_staging_buf_bytes` (usize, default 4 MiB = 4 × 1024 × 1024).

**Rationale** — Prevents fatal load failures under memory-tier saturation. Mirrors the interfaces-crate
backfill (`DispatcherConfig` fields FR-027 there). Code is authoritative.

- [ ] Approve

---

## BACKFILL-054 — Cold-read drain-to-completion / no early break

- **Direction**: BACKFILL (new FR; refines FR-019)
- **Confidence**: High

**Current State** — Unspecced robustness. FR-019 describes the sliding-window pipeline but not its
error/completion-draining semantics. Previously the pipelines broke early on the first error.

**Proposed Resolution** — Add:

> **FR-054**: The pipelined cold-read paths (`pipelined_ssd_to_gpu_zero_copy`,
> `pipelined_multi_object_zero_copy`) MUST drain every submitted NVMe read to completion
> (loop until `completed == submitted`) before returning and MUST NOT break early on the first error.
> On error the path sets a `stop_submitting` flag — submitting no new reads and marking any un-submitted
> work as failed for its object — but continues draining outstanding completions; the first error is
> retained and returned. Breaking early while reads are still in flight would orphan completions in the
> client's SPSC completion ring and deadlock (hang) the next client that reuses the NVMe queue.

**Rationale** — Fixes a client hang caused by orphaned in-flight reads; complements FR-019 and
US11 scenario 4 (per-entry error isolation). Code is authoritative.

- [ ] Approve
