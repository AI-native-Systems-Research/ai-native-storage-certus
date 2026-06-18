# Drift Report: Dispatcher Cache Interface (001-dispatcher-cache-interface)

**Generated**: 2026-06-16  
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`  
**Implementation**: `components/dispatcher/src/` (lib.rs, pipeline.rs, io_segmenter.rs, background.rs)  
**Interface**: `components/interfaces/src/idispatcher.rs`, `components/interfaces/src/imemory_tier.rs`  
**Prior run**: 2026-06-15

## Summary

| Status | Count |
|--------|-------|
| Aligned | 34 |
| Drifted (behavioral) | 3 |
| Drifted (omission from spec) | 2 |
| Resolved since last run | 1 (DRIFT-G) |
| New drift | 1 (DRIFT-H) |

## Drift Items

### DRIFT-A: `evict_for_space` blind-LRU path now uses shard-targeted eviction (Severity: Significant)

**Spec refs**: FR-024, User Story 7 (scenario 1, 3), Prior Clarity (line 217, 229, 231)  
**Severity**: Significant — the spec's stated eviction algorithm no longer matches the code

**Spec says** (FR-024): "the primary path on most iterations calls `IMemoryTier::evict_lru()` directly (O(1) under the MT lock)"

**Code does** (lib.rs `evict_for_space`):
- Function signature now accepts `target_key: CacheKey` as a fourth parameter
- The blind-LRU path calls `IMemoryTier::evict_lru_for_key(target_key)` instead of `evict_lru()`
- All callers (populate, promote_and_serve, batch_lookup) pass the key being inserted/promoted

**Why the code changed**: The memory-tier uses 16 shards (key % 16). `insert(key)` allocates from a specific shard determined by the key. The old `evict_lru()` round-robins across all shards, frequently freeing space in the wrong shard. Under pool-overflow workloads (e.g., populate exceeding pool capacity), the eviction loop exhausted MAX_ATTEMPTS=512 and returned "pool full after eviction" despite the pool having global free space — just not in the correct shard. The fix ensures eviction targets the same shard where the new key will be allocated, guaranteeing the freed space is usable by the subsequent `insert()`.

**Required spec update**: 
- FR-024: Replace `IMemoryTier::evict_lru()` with `IMemoryTier::evict_lru_for_key(target_key)` in the blind-LRU description
- Add explanation that shard-targeted eviction prevents cross-shard fragmentation under pressure
- Update `evict_for_space` signature description to include `target_key` parameter

---

### DRIFT-B: `IMemoryTier` interface has new `evict_lru_for_key` method (Severity: Moderate)

**Spec refs**: Prior Clarity Session 2026-05-08 (line 229)  
**Severity**: Moderate — new interface method not documented

**Spec says**: "The `IMemoryTier` interface provides `insert()`, `get()`, `peek()`, `remove()`, `evict_lru()`, `oldest_keys()`, `touch()`, `capacity()`, and `used()`."

**Code does**: The `IMemoryTier` trait (in `interfaces/src/imemory_tier.rs`) now also provides:
```rust
fn evict_lru_for_key(&self, key: CacheKey) -> Option<CacheKey>;
```
Evicts the LRU entry from the same shard as `key`, ensuring the freed space is allocatable by a subsequent `insert(key, ...)`. Returns the evicted key, or `None` if the target shard is empty.

**Required spec update**: Add `evict_lru_for_key()` to the IMemoryTier interface listing in Prior Clarity, and add a new clarification Q&A explaining the shard-targeting rationale.

---

---

### DRIFT-C: `max_eviction_attempts` now configurable (Severity: Minor)

**Spec refs**: FR-024, FR-033, User Story 7 (scenario 5)  
**Severity**: Minor — behavioral improvement, backward-compatible

**Spec says** (FR-024): "The loop is bounded by MAX_ATTEMPTS=512"  
**Spec says** (US7-5): "evict_for_space iterates MAX_ATTEMPTS=512 times"

**Code does**: `max_eviction_attempts` is a configurable field in `DispatcherConfig` (default 2048). Stored as `AtomicUsize` on the component, loaded at each call site.

**Required spec update**:
- FR-024: Replace hardcoded 512 with "configurable via `DispatcherConfig::max_eviction_attempts` (default 2048)"
- FR-033: Add `max_eviction_attempts: usize` to the DispatcherConfig field list
- US7 scenario 5: Update 512 → `max_eviction_attempts` (default 2048)

---

### DRIFT-D: Multi-stream warm pool replaces single warm_stream (Severity: Moderate)

**Spec refs**: FR-037, FR-039  
**Severity**: Moderate — architectural change for throughput

**Spec says** (FR-037): "pre-allocate a dedicated CUDA stream (`warm_stream`) ... stored as an `AtomicU64` and loaded without a mutex"

**Code does**: Allocates **4 CUDA streams** stored as `RwLock<Vec<u64>>`. `batch_lookup` distributes H2D copies round-robin across streams. Single `lookup` uses `streams[0]`.

**Required spec update**:
- FR-037: Replace single stream with "pool of N warm CUDA streams (default 4)" stored in `RwLock<Vec<u64>>`. Describe round-robin distribution in batch_lookup.

---

### DRIFT-E: Ring-buffer fallback pipeline removed (Severity: Moderate)

**Spec refs**: FR-019, FR-034, Key Entities (Pipelined Reader)  
**Severity**: Moderate — removes fallback path

**Spec says** (FR-019): "A ring-buffer fallback path (`pipelined_ssd_to_gpu`) exists for when the memory-tier pool is not registered for DMA"  
**Spec says** (FR-034): "Registration failure ... MUST NOT be fatal (the system falls back to the ring-buffer pipeline path)"

**Code does**: `pipelined_ssd_to_gpu` function and `PipelineRing::buffers` field have been deleted. Only zero-copy paths remain. `PipelineRing` now holds only streams + chunk_size.

**Required spec update**:
- FR-019: Remove ring-buffer fallback language
- FR-034: Registration failure should now be considered a warning (no fallback available)
- Key Entities "Pipelined Reader": Remove fallback description

---

### DRIFT-F: Deferred batch synchronization in batch_lookup hot path (Severity: Minor)

**Spec refs**: FR-039  
**Severity**: Minor — performance optimization, semantically equivalent

**Spec says** (FR-039): "serves MemoryTier and Staging hits inline (same as single-entry `lookup`)"

**Code does**: Issues all `memcpy_h2d_async` calls across warm streams without synchronizing, then calls `stream_synchronize` once per used stream after all copies are issued. Read locks and LRU touches are deferred until after sync.

**Required spec update**:
- FR-039: Add note that hot-path entries use deferred batch synchronization for throughput

---

### ~~DRIFT-G~~: RESOLVED — `promote_to_memory_tier` now specced as FR-040/FR-041

Previously unspecced. Now fully covered by FR-040 and FR-041 added to spec. No further action needed.

---

### DRIFT-H: `touch()` now also refreshes Memory Tier LRU (Severity: Moderate)

**Spec refs**: FR-023, User Story 8 (acceptance scenario 1), SC-010  
**Severity**: Moderate — spec explicitly prohibits memory-tier operations but implementation now includes them

**Spec says** (FR-023): "The `touch(key)` method MUST update the entry's eviction timestamp in the dispatch map without performing any DMA transfer or acquiring any reference."  
**Spec says** (US8 scenario 1): "No DMA or memory-tier operations occur."  
**Spec says** (SC-010): "The touch operation refreshes an entry's dispatch-map timestamp without performing DMA."

**Code does** (`lib.rs:2066-2080`):
```rust
fn touch(&self, key: CacheKey) -> Result<(), DispatcherError> {
    // ... dm.touch(key) ...
    if let Ok(mt) = self.memory_tier.get() {
        mt.touch(key);
    }
    Ok(())
}
```

**Why the code changed**: The eviction algorithm (FR-024) uses `mt.evict_lru_for_key(target_key)` as the primary eviction mechanism. This relies on the Memory Tier's internal LRU ordering, NOT the dispatch-map timestamp. Without `mt.touch(key)`, a `touch()`ed entry retains its old LRU position in the memory-tier and can be evicted from DRAM despite being recently accessed. The spec's goal ("preventing it from being selected as a victim" — User Story 8) is only achievable if both data structures are refreshed. Note: `mt.touch(key)` is metadata-only (updates an LRU timestamp); it performs no DMA, no memory copy, and acquires no dispatch-map reference.

**Required spec update**:
- FR-023: "The `touch(key)` method MUST update the entry's eviction timestamp in the dispatch map AND refresh the memory-tier LRU position (if the entry is memory-tier resident) without performing any DMA transfer or acquiring any dispatch-map reference."
- US8 scenario 1: "the entry's timestamp is refreshed in the dispatch map and its memory-tier LRU position is updated (if resident in DRAM). No DMA operations occur."
- SC-010: "The touch operation refreshes an entry's dispatch-map timestamp and memory-tier LRU position without performing DMA."

---

## Previously Resolved Drift

All items from the 2026-05-29 and 2026-06-12 runs (DRIFT-A through DRIFT-F in the prior report) remain as documented above — spec updates are pending.

---

## Items Correctly Implemented and Correctly Specified

All of the following are aligned between spec and code:

- FR-001 through FR-018 (core interface and operations)
- FR-020 through FR-022 (prepare/commit/cancel)
- FR-025 (format_on_init recovery)
- FR-028 through FR-036, FR-038 through FR-041 (promote, evictor, CUDA registration, clear, promote_to_memory_tier, DRAM-only pipelines)
- User Stories 1-6, 9-11 (except eviction details in US7, touch details in US8)
