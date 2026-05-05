---
title: "Status & Guard Rails"
updated: 2026-05-05
---

# Status & Guard Rails

## Critical Bugs (DO NOT ship without fixing)

### 1. dispatcher `check()` leaks read references

**File**: `components/dispatcher/v0/src/lib.rs:748`

`check()` calls `dm.lookup(key)` which increments `read_ref` in the dispatch-map, but never calls `release_read()`. Every `check()` call permanently pins the entry, making it un-evictable.

**Impact**: Eviction can never free checked entries. Under real workloads, the cache fills permanently.

**Fix options**:
- Call `dm.release_read(key)` after the lookup in `check()`
- Add a non-ref-counting existence check to `IDispatchMap` (e.g. `contains(key) -> bool`)

### 2. dispatcher `run_eviction_cycle` TOCTOU — eviction is dead code

**File**: `components/dispatcher/v0/src/lib.rs:296`

The eviction cycle calls `dm.lookup(key)` (increments `read_ref`), then immediately calls `dm.take_write(key)` (which waits for `read_ref == 0`). The write lock always times out after 100ms, so no entries are ever evicted.

**Why tests pass**: The mock `MockDispatchMap::take_write` doesn't check `read_refs`, only `write_ref`. The real `DispatchMapComponentV0` does.

**Impact**: `prepare_store` eviction is completely non-functional against the real dispatch-map. The cache will fill and `prepare_store` will fail with `AlreadyExists` once at capacity.

**Fix options**:
- Release the read ref before attempting the write lock
- Add a `block_offset(key) -> Option<u64>` query that doesn't take refs
- Restructure: `take_write` first, then inspect location while holding write

---

## Spec-vs-Code Drift

### dispatch-map contract (`specs/001-dispatch-map/contracts/idispatch_map.md`)

| Spec says | Code actually has | Impact |
|---|---|---|
| `convert_to_storage(key, offset, block_device_id: u16)` | `convert_to_storage(key, offset)` | `block_device_id` dropped — dispatcher uses key % drives for routing instead |
| `lookup(key, timeout: Duration)` | `lookup(key)` | Timeout hardcoded as `DEFAULT_TIMEOUT` (100ms) internally |
| `take_read(key, timeout)` / `take_write(key, timeout)` | `take_read(key)` / `take_write(key)` | Same — timeout internal |
| `LookupResult::Staging { ptr, len }` | `LookupResult::Staging { buffer: Arc<DmaBuffer> }` | Better — Arc ownership is safer |
| `LookupResult::BlockDevice { offset, device_id }` | `LookupResult::BlockDevice { offset }` | `device_id` dropped, dispatcher infers from key |
| No `oldest_keys` | `oldest_keys(n: usize) -> Vec<CacheKey>` | Added for eviction — never spec'd |

### dispatcher contract (`specs/001-dispatcher-cache-interface/contracts/idispatcher.md`)

| Spec says | Code actually has | Impact |
|---|---|---|
| 6 methods: initialize, shutdown, lookup, check, remove, populate | 9 methods: + `prepare_store`, `commit_store`, `cancel_store` | Three methods never spec'd |
| `DispatcherConfig { metadata_pci_addr, data_pci_addrs }` | Also: `block_device_version`, `extent_manager_version`, `max_cache_entries`, `eviction_threshold` | Config extended without spec update |
| Receptacles: logger, block_device_admin, dispatch_map | Receptacles: logger, dispatch_map, gpu_services, spdk_env | `block_device_admin` removed; `gpu_services` + `spdk_env` added |

---

## Functions Needed for Full vLLM OffloadingSpec Contract

Per `certus-connector/README.md`, the following are required but not yet implemented:

### On dispatch-map (IDispatchMap additions needed)

| Function | Purpose | Status |
|----------|---------|--------|
| `touch(key)` | Update LRU without taking a ref | Not implemented (lookup already updates TSC, but takes read_ref) |
| `pin(key)` | Increment ref_cnt for eviction protection | Semantic alias for `take_read` — may need dedicated method |
| `unpin(key)` | Decrement ref_cnt | Semantic alias for `release_read` |
| `evict_lru(count, protected) -> Option<Vec<CacheKey>>` | Atomic eviction: return N victims or None | Not implemented — `oldest_keys` is non-atomic, doesn't skip protected |
| `mark_ready(key)` | Block not loadable until complete_store(success) | Not implemented — no readiness flag exists |
| Non-ref-counting existence check | For dispatcher's `check()` and eviction | Not implemented — needed to fix Bug #1 and #2 |

### On certus-connector CertusEngine (engine.rs wiring)

| Method | Status |
|--------|--------|
| `batch_check(keys)` → `dispatcher.check()` | Done |
| `prepare_store(keys)` → dispatcher + eviction | In progress (eviction broken) |
| `complete_store(keys, ok)` → commit/cancel | Partially done |
| `touch(keys)` → dispatch-map LRU update | Not wired |
| `prepare_load(keys)` → dispatch-map pin | Not wired |
| `complete_load(keys)` → dispatch-map unpin | Not wired (no-op) |
| `shutdown()` → dispatcher + gpu shutdown | Done |

---

## Safe to Commit (no dependency on broken eviction)

- `components/extent-manager/` — standalone, fully tested
- `components/block-device-spdk-nvme/` — standalone, fully tested
- `components/gpu-services/` — standalone
- `components/spdk-env/` — standalone
- `components/spdk-sys/` — standalone
- `components/logger/` — standalone
- `components/dispatch-map/` — all methods work correctly in isolation
- `certus-connector/certus_connector/*.py` — Python adapter layer (no Rust dependency bugs)
- `components/component-framework/` — stable foundation

## Unsafe to Commit (depends on broken eviction path)

- Any code assuming `prepare_store` eviction works under load
- Any code assuming `check()` is side-effect-free
- Any integration test that exercises the full populate → evict → re-populate cycle against real `DispatchMapComponentV0`
- certus-connector `engine.rs` `prepare_store` wiring (will silently fail eviction)
