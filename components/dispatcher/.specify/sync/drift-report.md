# Spec Drift Report

Generated: 2026-07-22
Project: dispatcher (components/dispatcher)
Spec: specs/001-dispatcher-cache-interface/spec.md
Base commit: 833e9f36e01f1df8a0e0fc57d5cd223d823d3199 .. bb9569dde029cc7cd98306e88f7904b8cd4cdbee (HEAD)
Sources analyzed: src/lib.rs, src/pipeline.rs, src/cold_pool.rs, src/background.rs, src/io_segmenter.rs, src/metrics.rs, Cargo.toml, README.md, CLAUDE.md

This report supersedes the 2026-07-21 report. Since that report, two commits
landed: `3db1e6c` (partition-table compatibility guard on init) and `327306b`
("per-drive channel pool to stop cold-read completion theft"). The latter is
the primary driver of new drift below: it replaced the single cached
`ClientChannels` per drive (`DataDrive.cached_channels: Option<ClientChannels>`)
with a `ChannelPool` that lazily grows a per-drive pool of leasable channels,
specifically to fix a hang where two concurrent readers on the same drive
(`batch_lookup` and the prefetch path) shared one SPSC completion channel and
stole each other's completions.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 50 (active FR-* and SC-*; excluding 5 REMOVED FRs and 1 REMOVED SC) |
| Aligned | 47 |
| Drifted | 3 (FR-024, FR-034, FR-037) |
| Not Implemented | 0 |
| Unspecced Code | 5 |
| Inter-Spec Conflicts | 0 |

## Per-Spec Findings — 001-dispatcher-cache-interface

### Aligned

All active requirements not listed under "Drifted" remain aligned with the
code (FR-001..FR-023, FR-025, FR-028..FR-033, FR-035, FR-036,
FR-038..FR-050 and SC-001..SC-014). Neither of the two new commits touches
eviction (`evict_for_space`), per-device CUDA stream selection, or any other
requirement's logic — their scope is limited to channel-lifecycle management
(327306b) and an init-time partition-count guard (3db1e6c).

### Drifted

| Req | Spec says | Code now does | Severity |
|-----|-----------|---------------|----------|
| FR-034 | "The dispatcher SHOULD also cache block-device `ClientChannels` per data drive at init time to avoid per-operation connection overhead." (i.e. one `Option<ClientChannels>` cached at `initialize()` and reused by every reader.) | `DataDrive.cached_channels: Option<ClientChannels>` has been replaced by `DataDrive.channel_pool: ChannelPool` (`src/lib.rs:110,122-163`). The pool starts **empty** at init (`ChannelPool::new` at `src/lib.rs:1525` does not connect any channel eagerly — the "at init time" caching described by the spec no longer happens) and instead grows on demand: each concurrent reader calls `channel_pool.checkout()` (`src/lib.rs:641-642` in `promote_and_serve`, `:752-754` in `serve_cold_staged`, `:2861-2876` in `promote_to_memory_tier`'s per-drive prefetch threads) to obtain an exclusive, RAII-leased `ClientChannels`; the channel is connected lazily on first checkout and returned to the pool on `Drop`. This was a deliberate bug fix: a single shared channel let two concurrent readers on the same drive (`batch_lookup` and the prefetch path) dequeue each other's NVMe completions ("completion theft"), causing a permanent, non-timing-out hang. The spec's "cache at init time, single instance" model is no longer what the code does or should do. | High |
| FR-024 | `evict_for_space` uses sparse-probe + **shard-targeted blind LRU primary** (`evict_lru_for_key(target_key)`); when no clean candidate is found it **blind-frees** the LRU victim and removes the dispatch-map entry if the BlockDevice transition fails ("data loss accepted"). | `evict_for_space` now frees one **pin-safe** victim per iteration via `evict_one_clean`, scanning a **widening** `oldest_keys(4×attempts, cap 1024)` window. Each candidate is demoted (`try_evict_to_block`) or, if unpinned, dropped (`dm.remove`); pinned candidates are skipped. The blind `evict_lru`/`evict_lru_for_key(target_key)` fallback is **removed** — `target_key` is now unused (`_target_key`). If every scanned candidate is pinned it returns `AllocationFailed` rather than free a slot an in-flight load points at. `evict_and_insert`'s fragmentation-relief path likewise uses `evict_one_clean` instead of blind `evict_lru_for_key`. | High |
| FR-037 | The dispatcher pre-allocates **a single warm CUDA stream** stored as an `AtomicU64` (`warm_stream`); "a single CUDA stream is used for GPU operations (lock-free access via atomic load). Multi-stream round-robin is reserved for future scaling." | The warm/pipeline paths now select **per-GPU-device** streams from a process-global `DEVICE_STREAMS` map (`device_streams_for`): one warm stream + one pipeline pair per device, created lazily on the target device. The device is resolved per request from the IPC destination pointer (`device_of_ptr`/`set_batch_device`) and made current before issuing the copy; the shared `warm_stream` AtomicU64 is now a fallback only. | Medium |

### Not Implemented
None.

## Unspecced Code

| Feature | Location | Suggested FR |
|---------|----------|--------------|
| Per-drive `ChannelPool` / `ChannelLease` for concurrent cold-path readers: replaces the single per-drive cached channel with a lazily-grown pool of exclusive, RAII-leased `ClientChannels`. Checkout drains stale completions so a recycled channel never matches an old op's tag against a new op's segments. `connect_client` runs outside the pool lock. | `lib.rs:110` (`DataDrive.channel_pool`), `lib.rs:122-163` (`ChannelPool`, `ChannelLease`), checkout call sites at `lib.rs:641-642` (`promote_and_serve`), `lib.rs:752-754` (`serve_cold_staged`), `lib.rs:2861-2876` (`promote_to_memory_tier` per-drive prefetch threads) | Rewrite **FR-034**'s second sentence (see Recommendations) |
| Init-time partition-table compatibility guard: validates that a data drive's GPT has the expected 3 Certus partitions (metadata, extended metadata, data) before indexing `table.partitions[0]`/`[2]`, returning a descriptive `DispatcherError::IoError` with remediation guidance instead of panicking with an out-of-bounds index on a disk with a valid-but-non-Certus (e.g. empty/zeroed) GPT. | `lib.rs` `initialize()`, `EXPECTED_PARTITIONS` check (~line 1373-1391, commit `3db1e6c`) | New **FR-055** (edge case of FR-025) |
| Concurrent-promotion-race recovery: a `batch_lookup` cold promotion that loses the `mt.insert` race (`MemoryTierError::AlreadyExists`) is treated as a hit — mapped to `DispatcherError::AlreadyExists`, then a bounded-wait recovery pass serves the winner's resident slot to the GPU. | `lib.rs` `serve_concurrently_promoted`, `serve_memory_tier_to_gpu`, batch_lookup recovery post-pass; AlreadyExists error mapping | Already covered — see FR-051 |
| Per-GPU-device CUDA stream routing for multi-GPU / tensor-parallel: `DEVICE_STREAMS` map, `device_streams_for`, `set_batch_device`; `cold_pool::ColdReadRequest.gpu_device` + worker `set_device`. | `lib.rs` `DeviceStreams`/`DEVICE_STREAMS`/`device_streams_for`/`set_batch_device`; `cold_pool.rs` `ColdReadRequest.gpu_device` | Already covered — see FR-052 |
| Cold-load staging pool: bounded pool of pinned, pre-registered host DRAM buffers (`StagingPool`/`StagingLease`) used to serve cold reads uncached (`SSD→staging→GPU`) when the memory tier is saturated, instead of failing with `AllocationFailed`. | `pipeline.rs` `StagingPool`/`StagingLease`, `PipelineRing::new` staging arg; `lib.rs` `serve_cold_staged`, `promote_and_serve` staging fallback, batch_lookup staging post-pass | Already covered — see FR-053 |
| Cold-read drain-to-completion / no-early-break: pipelined cold paths drain until `completed == submitted` and use a `stop_submitting` flag on error instead of breaking early. | `pipeline.rs` `pipelined_ssd_to_gpu_zero_copy`, `pipelined_multi_object_zero_copy` | Already covered — see FR-054 |

Note: the last three rows (FR-051/052/053/054) were already added as spec text in the prior sync
(`bb427f1`); they are listed here only for completeness/traceability and are not new drift.
The two genuinely new unspecced items introduced since the last sync are the **ChannelPool**
and the **partition-table guard**.

## Inter-Spec Conflicts

None. (Single spec directory for this component.)

## Recommendations

1. **FR-034 (drift, High)** — Rewrite the second sentence to describe the `ChannelPool`:
   replace "The dispatcher SHOULD also cache block-device `ClientChannels` per data drive at
   init time to avoid per-operation connection overhead" with a description of the lazily-grown,
   RAII-leased per-drive channel pool and *why* a single shared channel is unsafe (completion
   theft between concurrent readers on the same drive). This is the most user-visible drift since
   it is a documented behavioral guarantee ("cached at init") that the code deliberately no longer
   provides, for a hang-prevention reason a spec reader should know about.
2. **FR-024 (drift, High)** — Rewrite to match pin-safe eviction (unchanged since last report).
3. **FR-037 (drift, Medium)** — Update to describe per-device streams (unchanged since last report).
4. **FR-055 (backfill)** — Add a requirement documenting the partition-table compatibility guard
   added in `3db1e6c`, as a refinement/edge case of FR-025 (recovery/format_on_init behavior).
5. No action needed for FR-051..FR-054 — already synced in `bb427f1`.
