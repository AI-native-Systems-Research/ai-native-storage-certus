# Drift Resolution Proposals — dispatcher

Generated: 2026-10-14 (analysis run 2026-10-18) · Branch: `fix/bundle-dispatcher-ems` (HEAD `4f8b5554b`, base `origin/unstable` `acd240255`) · Mode: **batch**, no questions asked
Based on: `drift-report.md` / `drift-report.json` of the same run (finding ids D-xx, P-xx and U-xx refer to that report)

**Nothing has been applied.** Every proposal is `approved: false`. No spec, source or other file was edited. Spec paths below are relative to `components/dispatcher/`. `001` = `specs/001-dispatcher-cache-interface/spec.md`, `002` = `specs/002-served-by-tier-attribution/spec.md`. Every BACKFILL "old text" is copied verbatim from the spec, and each one was confirmed to occur exactly once.

## Summary

| Resolution Type | Count | Ids |
|-----------------|-------|-----|
| Backfill (Code → Spec) | 11 | B-01 … B-11 |
| Align (Spec → Code) | 8 | A-01 … A-08 (A-07 also offers a BACKFILL alternative) |
| Human Decision | 0 as a separate type. The choice inside A-07 is flagged there |
| New Specs | 4 | N-01 … N-04 |
| Supersede | 0 | — |
| Remove from Spec | 0 (the stale text is replaced inside B-06 / B-11, not removed as requirements) |
| **Total** | **23** | |

Priority: **A-01, A-02, A-03 are major.** A-01 reports failed copies as hits. A-02 is the race this branch's formal-verification repair closed for `lookup` but left open on `batch_lookup`. A-03 frees a pinned slot. Apply B-01 … B-05 in the same change as this branch, so the specs record the five behaviour-changing repairs before the digest is re-stamped.

---

## BACKFILL (Code → Spec)

### B-01 · 001/FR-036 (+ FR-051) — cold `lookup` keeps its read pin through promotion and recovers a lost insert race
- **Type**: BACKFILL · **Finding**: D-01 · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:313`): "For SSD promotion paths the copy completes synchronously and a null stream is returned." `001:328` (FR-051) limits race recovery to "a `batch_lookup` cold promotion".
- **Code does**: the `lookup_async` BlockDevice arm holds the `dm.lookup` read pin across `promote_and_serve` and releases it afterwards (`src/lib.rs:3072-3076`, `:3099`). `AlreadyExists` → `serve_concurrently_promoted` (`src/lib.rs:3086-3097`). Commit `1d24ff27f`. Test `concurrent_remove_cannot_delete_entry_during_cold_promotion` (`src/repair_test_cold_pin.rs:508`).
- **Rationale**: a formally found and repaired race, with a red-first test. The spec states nothing about the pin's lifetime, which is the property the fix exists to guarantee.
- **Edit 1 — old** (`001:313`):
  > For SSD promotion paths the copy completes synchronously and a null stream is returned.
- **Edit 1 — new**:
  > For SSD promotion paths the copy completes synchronously and a null stream is returned. *(Sync 2026-10-14 — code authoritative, commit `1d24ff27f`:)* On a BlockDevice hit, `lookup_async` MUST hold the read pin its `dm.lookup` took until `promote_and_serve` has returned, and release it only then. That pin is what stops a concurrent `remove` or SSD eviction from deleting the entry, or freeing the freshly inserted memory-tier slot, while the SSD read, the GPU copy and the in-place `promote_block_to_memory_tier` are still running. *(Previously the pin was released before promotion started.)* If `promote_and_serve` fails with `AlreadyExists` (a concurrent cold lookup of the same key won the `mt.insert` race), `lookup_async` MUST treat it as a hit and serve the winner's slot via `serve_concurrently_promoted` (FR-051), not fail the load.
- **Edit 2 — old** (`001:328`):
  > When a `batch_lookup` cold promotion loses the `IMemoryTier::insert` race
- **Edit 2 — new**:
  > When a `batch_lookup` or single-key `lookup`/`lookup_async` cold promotion loses the `IMemoryTier::insert` race

### B-02 · 001/FR-032 + edge case — SSD evictor removes the dispatch-map entry before it touches the memory tier
- **Type**: BACKFILL · **Finding**: D-02 · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:309`): "Entries with active read or write references MUST be skipped gracefully (dm.remove fails without panic)." `001:236`: "No memory-tier operation is needed (the entry was already evicted from DRAM)."
- **Code does**: `dm.remove(key)` first; on failure `continue`; only then `mt.remove(key)`, emit `Removed`, free the extent (`src/background.rs:349-374`). Commits `1d24ff27f` / `d450c7cc1`. Test `ssd_evictor_keeps_slot_when_remove_fails` (`src/repair_test_ssd_evictor_release.rs:290`).
- **Rationale**: the edge case's premise ("already evicted from DRAM") does not hold for a key a cold lookup promotes between the evictor's check and its removal. That is exactly the case the repair handles.
- **Edit 1 — old** (`001:309`):
  > Entries with active read or write references MUST be skipped gracefully (dm.remove fails without panic).
- **Edit 1 — new**:
  > Entries with active read or write references MUST be skipped gracefully (dm.remove fails without panic). *(Sync 2026-10-14 — code authoritative, commits `1d24ff27f`/`d450c7cc1`:)* The evictor MUST remove the dispatch-map entry **before** it frees any memory-tier slot for the key, and MUST NOT touch the memory tier when `dm.remove` fails. Between the evictor's `BlockDevice` check and its removal, a cold lookup may promote the key into a fresh slot and pin it; freeing the slot first released it from under that in-flight promotion. Only after `dm.remove` succeeds does the evictor call `IMemoryTier::remove` (a no-op when the key is not resident), emit `Removed`, and free the extent.
- **Edit 2 — old** (`001:236`):
  > When the SSD evictor removes an entry, it frees the extent via the extent manager and removes the dispatch-map entry. No memory-tier operation is needed (the entry was already evicted from DRAM).
- **Edit 2 — new**:
  > When the SSD evictor removes an entry, it first removes the dispatch-map entry. If the entry is pinned, that removal is refused and the candidate is skipped. Then it calls `IMemoryTier::remove` for the key. This is normally a no-op, because a BlockDevice candidate is not resident, but it is needed for the race in which a cold lookup promoted the key after the evictor's check. Finally it frees the extent via the extent manager (FR-032).

### B-03 · 001/FR-003 + FR-059 — a populate that fails after reserving releases its slot
- **Type**: BACKFILL · **Finding**: D-03 · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:280`): the happy path and the drop-on-full case, ending "…rather than returning `AllocationFailed`. See FR-060.)*". No post-reserve failure behaviour. `001:354`: "Entries that fail allocation or DMA do not prevent other entries from succeeding."
- **Code does**: `release_memory(key)` on gpu-services-unbound / `copy_gpu_to_memory_async` error / `stream_synchronize` error (`src/lib.rs:3215-3246`). `batch_populate` likewise (`src/lib.rs:3316-3319`, `:3346-3354`). Commit `9defa9e1a`. Tests `src/repair_test_populate_slot.rs:485,504,534`.
- **Rationale**: an unregistered slot can be reclaimed by no evictor, so a leak is permanent and blocks retries with `AlreadyExists`. This is a repaired, tested invariant.
- **Edit 1 — old** (`001:280`):
  > rather than returning `AllocationFailed`. See FR-060.)*
- **Edit 1 — new**:
  > rather than returning `AllocationFailed`. See FR-060.)* *(Sync 2026-10-14 — code authoritative, commit `9defa9e1a`:)* Once `reserve_memory` has succeeded, every failure before the entry is registered MUST release the reserved slot (`release_memory`) before returning the error. Such failures are: `gpu_services` not bound, a `copy_gpu_to_memory_async` error, or a store-stream `stream_synchronize` error. Until registration the slot has no dispatch-map entry, so no evictor can ever reclaim it. Leaking it loses DRAM and makes every retry of the key fail with `AlreadyExists`.
- **Edit 2 — old** (`001:354`):
  > Entries that fail allocation or DMA do not prevent other entries from succeeding.
- **Edit 2 — new**:
  > Entries that fail allocation or DMA do not prevent other entries from succeeding. An entry whose D2H copy fails, or whose batch `stream_synchronize` fails, has its reserved slot released (`release_memory`) before its error is reported, as in `populate` (FR-003).

### B-04 · 001/FR-056 `copy_gpu_to_memory_completed` — keeps the live slot on `AlreadyExists`
- **Type**: BACKFILL · **Finding**: D-04 · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:338`): "`copy_gpu_to_memory_completed(key, size)` finalizes the entry after the async copies complete, transitioning it to a resident `MemoryTier` entry (design invariant P10)."
- **Code does**: on `DispatchMapError::AlreadyExists` it re-looks-up the key. It frees the slot only if the key is gone or resolves elsewhere. It keeps the slot if the entry points at it or the lookup errors. It always returns `AlreadyExists` (`src/lib.rs:3560-3580`). Commit `ad3654207`. Test `completed_on_registered_key_keeps_the_live_slot` (`src/repair_test_copy_completed.rs:428`).
- **Rationale**: before the fix, a repeated or racing completion freed a slot the live entry still pointed at, which is a use-after-free for every later reader.
- **Old** (`001:338`):
  > `copy_gpu_to_memory_completed(key, size)` finalizes the entry after the async copies complete, transitioning it to a resident `MemoryTier` entry (design invariant P10).
- **New**:
  > `copy_gpu_to_memory_completed(key, size)` finalizes the entry after the async copies complete, transitioning it to a resident `MemoryTier` entry (design invariant P10). If the dispatch map already holds an entry for `key` (`AlreadyExists`, from a repeated or racing completion), it MUST return `AlreadyExists`. It MUST free the memory-tier slot only when the existing entry provably does not reference it, meaning the key has since disappeared or resolves to a different location. When the existing entry points at this very slot, or its state cannot be determined, the slot MUST be kept: a leaked slot is recoverable, but a freed live one is a use-after-free for every reader. *(Sync 2026-10-14 — code authoritative, commit `ad3654207`; previously the slot was freed unconditionally.)*

### B-05 · 001/FR-042 + FR-050 — every eviction path reports dropped events; FR-050's `Removed` clause is stale
- **Type**: BACKFILL · **Finding**: D-05 (+ U-05) · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:319`): "Eviction events are emitted (best-effort, non-blocking send) whenever an entry is evicted from the memory-tier by `evict_for_space`." `001:327`: "The evictor MUST emit `EvictionEvent { key, reason: Demoted }` (or `Removed` on transition failure) via the eviction notification channel (FR-042) for each demoted entry."
- **Code does**: `EvictionSender { tx, dropped }` with `emit()`, which either queues the event or counts it (`src/background.rs:225-241`). It is used by the foreground path (`src/lib.rs:579-584`), the MT evictor `Demoted` (`src/background.rs:564-569`) and the SSD evictor `Removed` (`src/background.rs:360-365`). The MT evictor emits nothing on transition failure (`src/background.rs:556-558`). Commit `0d3fcde43`. Test `src/repair_test_eviction_events_dropped.rs:170`.
- **Rationale**: the spec's emitter list and its `Removed` attribution no longer match what the code emits or counts. *(The remaining production gap in SSD-evictor wiring is A-06, not this backfill.)*
- **Edit 1 — old** (`001:319`):
  > Eviction events are emitted (best-effort, non-blocking send) whenever an entry is evicted from the memory-tier by `evict_for_space`.
- **Edit 1 — new**:
  > Eviction events are emitted (best-effort, non-blocking send) whenever an entry is evicted from the memory-tier by `evict_for_space` (`Demoted`), demoted by the background memory-tier evictor (`Demoted`, FR-050), or removed by the background SSD evictor (`Removed`). *(Sync 2026-10-14 — code authoritative, commit `0d3fcde43`:)* Every emitter sends through one `EvictionSender`, which pairs the channel sender with the dispatcher's dropped-event counter. An event that does not fit is therefore counted in `eviction_dropped_count()` whichever path produced it. Before this commit only foreground drops were counted, and background drops were lost silently.
- **Edit 2 — old** (`001:327`):
  > The evictor MUST emit `EvictionEvent { key, reason: Demoted }` (or `Removed` on transition failure) via the eviction notification channel (FR-042) for each demoted entry.
- **Edit 2 — new**:
  > The evictor MUST emit `EvictionEvent { key, reason: Demoted }` via the eviction notification channel (FR-042) for each demoted entry. On transition failure the entry is skipped (FR-049) and no event is emitted; `Removed` is emitted only by the background SSD evictor (FR-042). *(Sync 2026-10-14 — the earlier "(or `Removed` on transition failure)" did not match the code.)*

### B-06 · 001/FR-019, US-9 AS1, Q&A, Key Entities — the `pipelined_ssd_to_gpu` ring-buffer fallback no longer exists
- **Type**: BACKFILL · **Finding**: D-08 · **Confidence**: HIGH · `approved: false`
- **Spec says**: `001:164` "The fallback path (ring-buffer `pipelined_ssd_to_gpu`) is used when the memory-tier pool is not CUDA-pinned/SPDK-registered." `001:265` describes it with 8 DmaBuffers. `001:296` and `001:378` call `pipelined_ssd_to_gpu_zero_copy` the "fallback".
- **Code does**: no `pipelined_ssd_to_gpu` exists. The public pipeline functions are listed at `src/pipeline.rs:235,457,771,1166,1279`. `promote_and_serve` always uses `pipelined_multi_object_zero_copy` (`src/lib.rs:875-886`). `pipelined_ssd_to_gpu_zero_copy` is called only by `serve_cold_staged` (`src/lib.rs:998-1011`).
- **Rationale**: the spec describes code that was removed.
- **Edit 1 — old** (`001:164`):
  > The fallback path (ring-buffer `pipelined_ssd_to_gpu`) is used when the memory-tier pool is not CUDA-pinned/SPDK-registered.
- **Edit 1 — new**:
  > There is no separate fallback for a memory-tier pool that is not CUDA-pinned/SPDK-registered. A registration failure is only logged at `initialize()` (FR-034), and the zero-copy pipelines require the registration. *(Sync 2026-10-14 — the ring-buffer `pipelined_ssd_to_gpu` fallback no longer exists in `src/pipeline.rs`.)*
- **Edit 2 — old** (`001:296`):
  > zero-copy pipelines (`pipelined_ssd_to_gpu_zero_copy` for the fallback single-entry path, `pipelined_multi_object_zero_copy` for the primary single-entry and batch paths)
- **Edit 2 — new**:
  > zero-copy pipelines (`pipelined_multi_object_zero_copy` for the single-entry promotion path, `scatter_gather_multi_drive_zero_copy` for the batch path, and `pipelined_ssd_to_gpu_zero_copy` for the cold-load staging serve)
- **Edit 3 — old** (`001:296`):
  > The single-entry fallback `pipelined_ssd_to_gpu_zero_copy` retains per-segment H2D with `max_queue_depth=16`.
- **Edit 3 — new**:
  > `pipelined_ssd_to_gpu_zero_copy` (per-segment `dma_copy_to_device_async` H2D, `max_queue_depth=16`) is no longer a promotion fallback; it is the read primitive of the cold-load staging serve (`serve_cold_staged`, FR-053).
- **Edit 4 — old** (`001:265`):
  > The single-entry fallback (`pipelined_ssd_to_gpu_zero_copy`) retains per-segment H2D via `dma_copy_to_device_async` on alternating streams with FIFO completion ordering. The ring-buffer fallback (`pipelined_ssd_to_gpu`) uses intermediate buffers (8 CUDA-pinned DmaBuffers) and processes in batches: submit all reads for a batch → wait for all completions → memcpy each to memory-tier slot + issue GPU copies → next batch.
- **Edit 4 — new**:
  > `pipelined_ssd_to_gpu_zero_copy` retains per-segment H2D via `dma_copy_to_device_async` on alternating streams with FIFO completion ordering; it now serves only the cold-load staging path (FR-053). *(Sync 2026-10-14: the ring-buffer `pipelined_ssd_to_gpu` fallback this answer previously described no longer exists.)*
- **Edit 5 — old** (`001:378`):
  > (3) `pipelined_ssd_to_gpu_zero_copy` (fallback — single object, per-segment H2D via `dma_copy_to_device_async`).
- **Edit 5 — new**:
  > (3) `pipelined_ssd_to_gpu_zero_copy` (single object, per-segment H2D via `dma_copy_to_device_async`; used by the cold-load staging serve, FR-053).

### B-07 · 001/FR-037 — per-device streams are not destroyed at shutdown
- **Type**: BACKFILL · **Finding**: D-09 · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:314`): "All per-device streams MUST be destroyed during `shutdown()`."
- **Code does**: `shutdown` destroys `warm_stream` and the `PipelineRing` only (`src/lib.rs:2151-2161`). The `DEVICE_STREAMS` entries "live until the process exits" (`src/lib.rs:453-455`, `:467-469`).
- **Rationale**: this is a documented, deliberate design (one process-wide dispatcher, and CUDA streams are process-wide). If a lifetime rule is wanted instead, raise it as an ALIGN.
- **Old** (`001:314`):
  > All per-device streams MUST be destroyed during `shutdown()`.
- **New**:
  > The fallback `warm_stream` and the `PipelineRing` streams MUST be destroyed during `shutdown()`. The per-device `DEVICE_STREAMS` entries are not destroyed by `shutdown()`. They are process-global, created lazily per device and deliberately kept until process exit, because one dispatcher serves all local GPUs and CUDA streams are process-wide resources. *(Sync 2026-10-14 — code authoritative, `src/lib.rs:453-455`, `2151-2161`.)*

### B-08 · 001 edge case — `remove` order is dispatch map first, then memory tier
- **Type**: BACKFILL · **Finding**: D-10 · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:231`): "When remove is called for a key, the memory-tier slot is freed first, then the dispatch-map entry is removed, then the SSD extent (if any) is freed."
- **Code does**: `dm.remove(key)` (refused while pinned → `KeyNotFound`), then `mt.remove(key)`, then `remove_extent` (`src/lib.rs:3152-3168`). Unchanged since before this branch.
- **Rationale**: the spec states the unsafe order, which contradicts FR-024/FR-049 and the order this branch now applies to the SSD evictor.
- **Old** (`001:231`):
  > When remove is called for a key, the memory-tier slot is freed first, then the dispatch-map entry is removed, then the SSD extent (if any) is freed.
- **New**:
  > When remove is called for a key, the dispatch-map entry is removed first. That stops new lookups from obtaining the memory-tier pointer, and it is refused while the entry is pinned or write-locked. Then the memory-tier slot is freed, then the SSD extent (if any). A remove that is refused because the entry is pinned returns `KeyNotFound` and frees nothing. *(Sync 2026-10-14 — code authoritative; matches FR-024/FR-049 ordering.)*

### B-09 · 001/FR-003 + Assumptions — populate does not use `dma_copy_to_host`
- **Type**: BACKFILL · **Finding**: D-12 · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:280`): "…via `IGpuServices::dma_copy_to_host`, register…". `001:410`: "GPU-to-host DMA transfers use `IGpuServices::dma_copy_to_host` (single-entry populate direction)…".
- **Code does**: `populate` → `copy_gpu_to_memory_async` (per-region `memcpy_d2h_async` on `store`) + `stream_synchronize` (`src/lib.rs:3232-3246`, `:3519-3537`). There is no non-test `dma_copy_to_host` call.
- **Rationale**: this is an inconsistency inside the spec itself: FR-037 and FR-056 already describe the real path.
- **Edit 1 — old** (`001:280`):
  > initiate DMA copy from the client's GPU memory into the memory-tier slot via `IGpuServices::dma_copy_to_host`, register
- **Edit 1 — new**:
  > copy the client's GPU memory into the memory-tier slot via `copy_gpu_to_memory_async` (per-region `IGpuServices::memcpy_d2h_async` on the device's `store` stream, FR-037/FR-056) followed by `stream_synchronize`, register
- **Edit 2 — old** (`001:410`):
  > GPU-to-host DMA transfers use `IGpuServices::dma_copy_to_host` (single-entry populate direction), or per-region `memcpy_d2h_async` on the caller-supplied stream for the reserve/`copy_gpu_to_memory_async` path (FR-056); both
- **Edit 2 — new**:
  > GPU-to-host DMA transfers use per-region `memcpy_d2h_async` via `copy_gpu_to_memory_async` (FR-056) for both the single-entry and the batch store path; both

### B-10 · 002/FR-008 — there is no "inline fallback" cold sub-path any more
- **Type**: BACKFILL · **Finding**: D-14 · **Confidence**: HIGH · `approved: false`
- **Spec says** (`002:475-476`): "the cold SSD promotion (all of its sub-paths — pooled, inline fallback, no-drives, and staging)".
- **Code does**: sub-paths are scatter-gather, no-drives, staging (`src/lib.rs:2486-2743`). The inline fallback was removed (001 FR-039(3), `src/lib.rs:2469-2476`).
- **Rationale**: the spec lists a sub-path that no longer exists. Each remaining sub-path still inherits `Ssd` from classification (`src/lib.rs:2440`).
- **Old** (`002:475-476`, the line break is part of the text):
  > (all of its sub-paths — pooled,
  >   inline fallback, no-drives, and staging)
- **New**:
  > (all of its sub-paths — scatter-gather,
  >   no-drives, and staging; the inline single-key fallback was removed, spec 001 FR-039(3))

### B-11 · 001/User Story 11 AS4 — "the thread" no longer exists
- **Type**: BACKFILL · **Finding**: ⧗ note in the drift report (US-11 AS4) · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:201`): "**When** the thread encounters the error".
- **Code does**: there are no cold-path threads. Per-job errors come out of the single scatter-gather loop and are isolated per entry (`src/lib.rs:2669-2718`).
- **Old** (`001:201`):
  > **When** the thread encounters the error
- **New**:
  > **When** the scatter-gather loop encounters the error

---

## ALIGN (Spec → Code)

### A-01 · 002/US1 AS3 + FR-009 — a failed warm batched sync must turn the warm keys into `ERROR`
- **Type**: ALIGN · **Finding**: D-13 · **Severity**: major · **Confidence**: HIGH · `approved: false`
- **Spec says** (`002:303-305`): "**Given** a batch in which the batched GPU synchronization fails, **When** `batch_lookup` returns, **Then** every key whose copy was in flight is attributed `ERROR` and none is reported as a hit." `002:430-434`: "Attribution MUST be rewritten in lockstep".
- **Code does**: `if let Err(e) = gpu.stream_synchronize(s) { self.log_info(...) }`. The warm results stay `Ok`, and `tier` stays `Dram` (`src/lib.rs:2750-2755`). Those keys are therefore counted as hits in `promotions_to_gpu` and `lookup_hits_dram` and sent as wire byte 1. The behaviour is the same on base.
- **Proposed change**: record the warm-hit indices as their copies are submitted (`src/lib.rs:2420-2431`). On a sync failure, set each of them to `results[i] = Some(Err(IoError("batch stream_synchronize failed: …")))`, mirroring the remote-delivery block (`src/lib.rs:2907-2917`). Leave `tier[i]` as it is: the final mapping (`:2982`) turns the `Err` into `Error`. Add a test that uses `PinProbe::failing_sync` on warm keys and asserts `ServedBy::Error` and `!result.is_ok()`.
- **Rationale**: the spec is explicit, and the code already does the right thing on the remote path. Reporting undelivered data as served is the worst kind of wrong for a hit-rate instrument.

### A-02 · 001/FR-051 (+ B-01) — `batch_lookup` cold path must keep its read pin until promotion completes
- **Type**: ALIGN · **Finding**: P-03 · **Severity**: major · **Confidence**: MEDIUM · `approved: false`
- **Spec says** (`001:328`): "The caller's load pin keeps the entry from being evicted while waiting." The new FR-036 text from B-01 makes keeping the pin through promotion an explicit guarantee for `lookup`.
- **Code does**: `let _ = dm.release_read(key);` right after classification (`src/lib.rs:2433-2434`). Then `evict_and_insert` → scatter-gather → `promote_block_to_memory_tier` runs with no pin held (`src/lib.rs:2562-2717`). Staging and no-drives are the same. `promote_to_memory_tier` has the same pattern (`src/lib.rs:3726-3729`).
- **Proposed change**: adopt each cold key's classification pin into a `pins::PinnedKeys` guard, the way warm hits do (`src/lib.rs:2356`, `:2428`). Drop that guard after the cold block and the staging post-pass, before the recovery pass, because `serve_concurrently_promoted` takes and releases its own pins. Port `repair_test_cold_pin.rs` to `batch_lookup`. *Alternative, if callers always pin first:* make "the caller holds a pin (`IDispatcher::pin`) for every key passed to `batch_lookup`" a documented precondition in 001 FR-039 and in the interfaces trait docs, and keep the code as it is.
- **Rationale**: this is the race the branch's verified repair closed for the single-key path, and the shm-queue `LOOKUP` path (`lib/shmq-dispatcher/src/translate.rs:756`) still has it. Confidence is MEDIUM only because a client that follows Check→Pin is already protected by its own pin, so the fix-or-document choice is left open.

### A-03 · 001/FR-038 `clear_memory_tier` — must not free a pinned entry's slot
- **Type**: ALIGN (with a wording update) · **Finding**: D-06 · **Severity**: major · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:315`): "This method evicts ALL entries from the memory-tier pool by calling `IMemoryTier::evict_next()` in a loop until empty. For each evicted key, it transitions the dispatch-map entry to BlockDevice state via `convert_memory_tier_to_block`. If the transition fails (write-through not complete), the entry is removed from the dispatch map entirely."
- **Code does**: it loops over `oldest_keys(64)` and calls `try_evict_to_block`. On *any* failure, including `ActiveReferences`, it runs `mt.remove(key)` unconditionally and then `dm.remove(key)`. The real map refuses that removal while the entry is pinned (`components/dispatch-map/src/lib.rs:341-343`), which leaves a `MemoryTier` entry pointing at a freed slot (`src/lib.rs:3875-3890`).
- **Proposed change**: match on the error. On `ActiveReferences`, skip the key and leave it resident. Otherwise call `dm.remove` first, and `mt.remove` only if it succeeds (the D-02 and D-10 order). Cap the outer loop so that a tier full of pinned keys cannot spin forever. In the same change, correct FR-038's mechanism wording (`evict_next` → `oldest_keys`; `convert_memory_tier_to_block` → `try_evict_to_block`) and add "pinned entries are skipped and remain resident".
- **Rationale**: this reproduces the dangling-pointer pattern that this branch's repairs removed elsewhere, and it is reachable over shm-queue `CLEAR_MEMORY_TIER` (`lib/shmq-dispatcher/src/translate.rs:891-894`).

### A-04 · 001/FR-028 + US-9 AS3 — the no-drives `promote_and_serve` must re-register the entry
- **Type**: ALIGN · **Finding**: D-07 · **Severity**: moderate · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:305`): "the pipelined reader MUST re-insert the entry into the memory-tier and re-register it as a MemoryTier entry". `001:166`: "…and re-registers the entry."
- **Code does**: `dm.create_memory_tier_entry` on an existing BlockDevice entry returns `AlreadyExists` and is ignored. `dm.release_write` is then called without a write ref (`src/lib.rs:814-820`). The slot is orphaned and the entry stays BlockDevice.
- **Proposed change**: use `dm.promote_block_to_memory_tier(key, mem_ptr, ipc_handle.size)` as the `batch_lookup` no-drives branch does (`src/lib.rs:2495-2500`), and drop the stray `release_write`. Make `MockDispatchMap::create_memory_tier_entry` (`src/lib.rs:4428-4451`) refuse to overwrite, so a test notices.
- **Rationale**: the requirement is clear, and the sibling code path already implements it correctly.

### A-05 · 001/FR-003 + FR-056 — release on phase-3 failures inside `copy_gpu_to_memory_completed`
- **Type**: ALIGN · **Finding**: P-01 · **Severity**: moderate · **Confidence**: MEDIUM · `approved: false`
- **Spec says**: after B-03, "every failure before the entry is registered MUST release the reserved slot". The repair's own comment says "every failure here must release it" (`src/lib.rs:3216-3218`).
- **Code does**: under `integrity-check`, a `stream_synchronize` or `set_checksum` failure after `create_memory_tier_entry` runs `mt.remove(key)` but leaves the new dispatch-map entry write-locked and pointing at the freed slot (`src/lib.rs:3599-3616`). A `downgrade_reference` failure leaves it write-locked (`src/lib.rs:3619-3620`).
- **Proposed change**: on those paths, undo the registration (`dm.remove`, after `release_write`) **before** `mt.remove`. Add a test under `--features integrity-check` with a failing sync.
- **Rationale**: this completes the invariant the repair introduced. Confidence is MEDIUM because the path is feature-gated and needs a forced device fault.

### A-06 · 001/FR-042 — the SSD evictor must see a channel created after `initialize()`
- **Type**: ALIGN · **Finding**: P-02 · **Severity**: moderate · **Confidence**: HIGH · `approved: false`
- **Spec says** (`001:319`, after B-05): the SSD evictor emits `Removed`, and its drops are counted.
- **Code does**: `BackgroundEvictor::start` receives `self.eviction_tx.lock().unwrap().clone()` at `initialize()` (`src/lib.rs:2059-2071`). Both servers call `create_eviction_channel` after `initialize` (`apps/certus-server-yaml/build.rs:447-471`; `apps/certus-server/src/main.rs:469`, `:529`), so the evictor holds `None` forever.
- **Proposed change**: pass `Arc::clone(&self.eviction_tx)` (an `Arc<Mutex<Option<EvictionSender>>>`), as `MemoryTierEvictor` already does (`src/lib.rs:2101`, `src/background.rs:462`), and lock it per emit. Add a test that creates the channel after `initialize` and asserts that a `Removed` event arrives or is counted.
- **Rationale**: without this, the guarantee in this branch's `EvictionSender` doc ("never lost silently", `src/background.rs:227-228`) is false for the SSD evictor in production.

### A-07 · 001/FR-058 `store_already_resident` — count only refused **reserves**
- **Type**: ALIGN (alternative: BACKFILL) · **Finding**: D-11 · **Severity**: minor · **Confidence**: MEDIUM · `approved: false`
- **Spec says** (`001:352`): "`store_already_resident` (reserves refused because the key was already cached rather than for want of space — interfaces `001-interfaces` FR-018a)".
- **Code does**: the counter is incremented inside `evict_and_insert` (`src/lib.rs:1213-1222`), so the load-path callers `promote_and_serve` (`:773`) and `batch_lookup` cold prep (`:2562-2563`) increment it as well, every time a concurrent promotion loses the race (FR-051).
- **Proposed change**: move `record_store_already_resident()` out of `evict_and_insert` into `reserve_memory`'s `Err(AlreadyExists)` handling.
- **Human decision needed**: if counting load-race losers is wanted, BACKFILL FR-058 instead. In that case replace "reserves refused because the key was already cached" with "`mt.insert` calls refused because the key was already resident, on the store path (a refused reserve) and on the load path (a lost concurrent promotion, FR-051)". The interfaces spec FR-018a would then need the same change.
- **Rationale**: the counter exists to explain declined stores (`align-tasks.md` DISP-RESERVE-REASON). Mixing in load races inflates it in exactly the TP-sibling workload.

### A-08 · 002/FR-027 + SC-009 — add per-value attribution tests
- **Type**: ALIGN (tests only) · **Finding**: D-15 · **Severity**: moderate · **Confidence**: HIGH · `approved: false`
- **Spec says** (`002:554-555`): "Each attribution value MUST have a test that fails if that value is mis-assigned, in both dispatchers."
- **Code does**: value-specific assertions exist only for `Dram` (`src/repair_test_served_by.rs:484`) and `Miss` (`src/lib.rs:5927`).
- **Proposed change**: add one test per value: `Ssd` (a no-drives cold key), `Remote` (`setup_initialized_with_remote` with a peer-held key), `SizeMismatch` (`MockDispatchMap::set_mismatch_key`, `src/lib.rs:4200`) and `Error` (the warm-sync failure from A-01, plus the not-initialized path, `src/lib.rs:2238-2251`). Show each one failing against a deliberately wrong mapping (FR-028).
- **Rationale**: the `is_hit ⇔ is_ok` and partition tests cannot tell the three hit values apart.

---

## NEW_SPEC (unspecced code)

### N-01 · New 001/FR-062 — `integrity-check` verify-before-serve
- **Type**: NEW_SPEC · **Finding**: U-01 · **Confidence**: MEDIUM · `approved: false`
- **Spec says**: nothing in the dispatcher specs. The interfaces spec covers only checksum *storage* (`components/interfaces/specs/001-interfaces/spec.md:31,167-168`).
- **Code does**: CRC-32 is computed after the store D2H lands (`src/lib.rs:3593-3617`) and checked before every warm (`:2408-2417`), remote (`:2861-2869`) and single-key (`:3047-3055`) H2D. A mismatch returns `IoError` (`:1237-1259`).
- **Draft FR-062**: "Under the `integrity-check` feature the dispatcher MUST compute a CRC-32 of each stored block once its D2H copy has completed, and record it on the dispatch-map entry (`set_checksum`). Before every memory-tier→GPU copy (warm hit, remote delivery, single-key lookup) it MUST recompute the checksum and compare. On a mismatch the key MUST fail with `IoError` (attributed `ERROR`, spec 002) instead of serving corrupt data. An entry with no recorded checksum is served unchecked. Cold SSD promotions are not verified."
- **Rationale**: this is a correctness-relevant, feature-gated behaviour with no requirement behind it. 002 FR-029 already presumes the feature exists.

### N-02 · 001/FR-045 note — remote-delivery stage probe
- **Type**: NEW_SPEC (a note on an existing FR) · **Finding**: U-02 · **Confidence**: HIGH · `approved: false`
- **Code does**: when `CERTUS_LOG_REMOTE_DELIVERY` is truthy, it records fetch / lookup / H2D / sync times per batch and logs a summary every 256 batches at `warn` (`src/remote_probe.rs:1-75`, call sites `src/lib.rs:2786-2938`).
- **Draft text**: "*Diagnostics:* setting `CERTUS_LOG_REMOTE_DELIVERY` to `1`/`true`/`yes`/`on` enables a per-batch stage probe of the remote-delivery path. It aggregates over 256 batches and logs at `warn`. It is off by default, and when off it costs one cached flag check per batch."

### N-03 · 001/FR-056 note — opt-in reserve logging
- **Type**: NEW_SPEC (a note on an existing FR) · **Finding**: U-03 · **Confidence**: HIGH · `approved: false`
- **Code does**: `CERTUS_LOG_RESERVE` turns on one `info` line per `reserve_memory` call, with the key, size and session id (`src/lib.rs:3379-3394`).
- **Draft text**: "*Diagnostics:* `CERTUS_LOG_RESERVE` (truthy) logs each `reserve_memory` call with its key, size and `session_id` at `info`. It is off by default because it is noisy under load."

### N-04 · 001/FR-009 + `contracts/errors.md` — the result of `remove` on a pinned entry
- **Type**: NEW_SPEC · **Finding**: U-04 · **Confidence**: MEDIUM · `approved: false`
- **Spec says**: `contracts/errors.md:37` lists `Timeout` for `remove`; FR-009 (`001:286`) gives no pinned-entry behaviour.
- **Code does**: `dm.lookup` errors (timeouts included) and `dm.remove` `ActiveReferences` both become `KeyNotFound` (`src/lib.rs:3140-3155`). `remove` never returns `Timeout`.
- **Draft text (FR-009 addition)**: "If the entry is pinned or write-locked, `remove` frees nothing and returns `KeyNotFound`. A dispatch-map timeout while locating the entry is also reported as `KeyNotFound`." For `contracts/errors.md:37`, drop `remove` from the `Timeout` row. *Alternatively, if callers need to tell these cases apart,* raise an ALIGN that returns `Timeout` / a distinct error instead.
- **Rationale**: callers currently cannot tell "absent" from "busy". Whichever choice is made should be written down.

---

## Not proposed (verified aligned or out of scope)
- 002 FR-009/FR-010: aligned by `a7ccc6e48` (`src/lib.rs:2946-2954`). No spec change is needed. The remaining FR-009 gap is A-01.
- The other ⧗ items (SC-012, SC-014, 002 SC-002/003/006/007/008) need hardware, a cluster or a benchmark, so no proposal is made for them.
- `4a8fe3e75` (`extended-metadata-store`) is outside this component.
