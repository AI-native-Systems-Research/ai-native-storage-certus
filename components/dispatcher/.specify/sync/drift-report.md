# Drift Report: Dispatcher Cache Interface (001-dispatcher-cache-interface)

**Generated**: 2026-06-12  
**Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`  
**Implementation**: `components/dispatcher/src/` (lib.rs, pipeline.rs, io_segmenter.rs, background.rs)  
**Interface**: `components/interfaces/src/idispatcher.rs`, `components/interfaces/src/imemory_tier.rs`  
**Prior run**: 2026-05-29

## Summary

| Status | Count |
|--------|-------|
| Aligned | 36 |
| Drifted (behavioral) | 1 |
| Drifted (omission from spec) | 1 |

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

## Previously Resolved Drift

All items from the 2026-05-29 run (DRIFT-A through DRIFT-G in that report) were resolved via BACKFILL proposals applied to the spec. Those changes remain aligned.

---

## Items Correctly Implemented and Correctly Specified

All of the following are aligned between spec and code:

- FR-001 through FR-023 (core interface and operations)
- FR-025 (format_on_init recovery)
- FR-028 through FR-039 (promote, evictor, CUDA registration, batch_lookup)
- User Stories 1-11 (except eviction algorithm details in US7)
