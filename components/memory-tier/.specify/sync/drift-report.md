# Spec-Drift Report: memory-tier

**Generated**: 2026-07-22T21:28:04Z
**Spec analyzed**: `components/memory-tier/specs/001-memory-tier/spec.md` (+ `plan.md`, `tasks.md` used as supporting context)
**Implementation analyzed**: `components/memory-tier/src/lib.rs`, `src/allocator.rs`, `Cargo.toml`, `README.md`, `components/interfaces/src/imemory_tier.rs`, `components/interfaces/src/ieviction_policy.rs`

## Summary

| Metric | Count |
|---|---|
| Specs analyzed | 1 |
| Requirements checked (FR+NFR+SC) | 44 |
| Aligned | 34 |
| Drifted | 5 |
| Not implemented | 5 |
| Unspecced features found | 3 |

**Headline finding**: The spec (and `plan.md`) describe a **16-way sharded** pool architecture (per-shard `FreeList` + slot map, `Mutex<Shard>`, key-modulo-16 shard selection, round-robin `evict_counter`, per-shard eviction pools). **None of this exists in the code.** The implementation (`src/lib.rs`) uses a single unsharded `Pool` (one `FreeList` + one `HashMap<CacheKey, Slot>`) behind one `RwLock<Pool>`, and a single `PoolId` obtained once from the eviction policy at `initialize()`. This is the most significant drift item in the component.

## Per-Spec Findings: `001-memory-tier`

### Aligned (20 FR + 8 NFR + 6 SC = 34)

| ID | Requirement | Evidence |
|---|---|---|
| FR-001 | Contiguous mmap'd region | `lib.rs:194-222` `alloc_mmap` |
| FR-002 | Hugepage preferred, fallback | `lib.rs:194-222` MAP_HUGETLB then plain mmap |
| FR-003 | SPDK `spdk_zmalloc` when SPDK active | `lib.rs:276-302` |
| FR-004 | 4 KiB alignment | `allocator.rs:5,42` `ALIGNMENT=4096`, `next_multiple_of` |
| FR-008 | insert rejects zero size | `lib.rs:328-330` |
| FR-009 | insert rejects duplicate key | `lib.rs:355-357` |
| FR-010 | insert returns PoolFull | `lib.rs:359-362` |
| FR-011 | get() returns ptr/size + LRU touch | `lib.rs:378-407` |
| FR-012 | peek() no LRU update | `lib.rs:409-419` (no `ep.touch` call) |
| FR-015 | remove()/KeyNotFound | `lib.rs:469-500` |
| FR-016 | touch() promotes, no data return | `lib.rs:502-515` |
| FR-017 | batch_touch amortizes lock | `lib.rs:517-552` |
| FR-018 | clear() removes all, returns count | `lib.rs:591-605` |
| FR-019 | NUMA mbind with graceful fallback | `lib.rs:224-253` |
| FR-020 | is_dma_capable() only true for SPDK | `lib.rs:607-610` |
| FR-022 | pool_info() for CUDA registration | `lib.rs:582-589` |
| FR-023 | initialized-flag guard on all ops | every method in `lib.rs` checks `state.initialized` |
| FR-024 | IEvictionPolicy is external receptacle | `lib.rs:141-144` |
| FR-025 | ILogger is optional receptacle | `lib.rs:141-144`, `log_info`/`log_warn` use `if let Ok` |
| FR-026 | Free-list coalesces on dealloc | `allocator.rs:59-83` |
| NFR-001 | O(1) LRU via delegated policy | architecture (`IEvictionPolicy`) |
| NFR-003 | No syscalls on data path post-init | `insert`/`get`/etc. contain no syscalls |
| NFR-004 | Send+Sync | `lib.rs:94-95` `unsafe impl Send/Sync` |
| NFR-005 | BTreeMap first-fit | `allocator.rs:10,44` |
| NFR-006 | Freed on Drop (munmap/spdk_free) | `lib.rs:115-135` |
| NFR-007 | Default pool size 256 MiB | `lib.rs:33` `DEFAULT_POOL_SIZE` (see note under Unspecced — constant is unused by any call site) |
| NFR-009 | SPDK feature optional | `Cargo.toml:9` `spdk = ["dep:spdk-sys"]`, `#[cfg(feature="spdk")]` guards |
| NFR-010 | DMA-suitable pointers | 4 KiB alignment + contiguous region |
| SC-1 | Unit tests pass | `cargo test -p memory-tier --lib` → 21/21 passed |
| SC-2 | No leaks (Drop) | `lib.rs:115-135` |
| SC-4 | 4 KiB alignment invariant | `allocator.rs` |
| SC-5 | Eviction frees + allows reinsert | `remove_and_reuse`, `evict_lru_returns_some` tests |
| SC-6 | NUMA binds or falls back | `lib.rs:224-253` |
| SC-7 | SPDK path DMA-capable | `lib.rs:276-302`, `is_dma_capable` |

### Drifted (3 FR + 1 NFR + 1 SC = 5)

| Requirement | Spec text | Actual | Location | Severity |
|---|---|---|---|---|
| FR-013 | "evict_lru() cycles through shards via atomic counter (round-robin)" | No `evict_counter`, no shards. `evict_lru()` simply calls `ep.pop_oldest(state.pool_id)` on the single global pool. | `src/lib.rs:431-463` | High |
| FR-014 | "evict_lru_for_key() evicts from the same shard as the target key" | The `key` argument is ignored (`_key`); the function is a pure alias for `evict_lru()`. Targeted-shard eviction is impossible because there is only one pool. | `src/lib.rs:465-467` | High |
| FR-021 | "oldest_keys(n) peeks at N oldest keys across shards" (spec's Implementation Notes further claims per-shard sampling `(n / NUM_SHARDS).max(1)`) | Single call to `ep.peek_oldest(state.pool_id, n)` — no per-shard sampling logic exists at all. | `src/lib.rs:421-429` | Medium |
| NFR-008 | "Component version is 0.2.0" | Three different version strings exist and none is 0.2.0: `Cargo.toml` package version = `"0.1.0"`; `define_component!` macro `version:` field = `"0.3.0"`. | `Cargo.toml:3`, `src/lib.rs:139` | Medium |
| SC-3 | "Concurrent access from 16+ threads does not deadlock or corrupt state" (implicitly relies on the 16-shard design for parallelism) | Correctness likely still holds (single `RwLock<Pool>` + `Mutex`-free path serializes all writers), but the claimed 16-way concurrency mechanism does not exist — all writers on all keys contend for one lock, not one of 16. | `src/lib.rs:75-89` (`Pool`, `MemoryTierState`) | Medium |

### Not Implemented (3 FR + 1 NFR + 1 SC = 5)

| Requirement | Spec text | Status |
|---|---|---|
| FR-005 | "Pool is divided into 16 independent shards" | No `Shard` struct, no `NUM_SHARDS` constant, no per-shard anything anywhere in `src/`. |
| FR-006 | "Shard selection uses key modulo 16" | No `shard_for_key` function exists in the codebase. |
| FR-007 | "Each shard has its own Mutex-protected allocator and slot map" | Single `RwLock<Pool>` wraps one `FreeList` + one `HashMap`; no `Mutex<Shard>` anywhere. |
| NFR-002 | "Per-shard locking minimizes contention (16-way parallelism)" | Same as above — one lock, not 16. |
| SC-8 | "10 formal properties verified with Creusot (21 verification conditions)" | No `verif/`, `creusot/`, or any Creusot-related directory/file exists under `components/memory-tier/`. The referenced properties P4 (shard-bounded), P5 (shard-deterministic), and P10 (evict-round-robin) describe behavior that does not exist in code, so they could not have been verified against the current implementation even if proof artifacts existed elsewhere. |

**Related interface-doc drift (not a spec.md FR, but worth flagging)**: `components/interfaces/src/imemory_tier.rs` doc comments still assert `# Verified: P4 (shard-bounded)`, `P5 (shard-deterministic)`, `P10 (evict-round-robin)` on `get`, `peek`, `touch`, `batch_touch`, `contains`, `evict_lru`, `evict_lru_for_key` (lines 107-166) and a comment block at lines 53-68 restates the same 10 Creusot properties as "formally proved." These claims are stale relative to the actual (unsharded) implementation.

## Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---|---|---|---|
| `telemetry` Cargo feature + `MemoryTierTelemetry`/`TelemetrySnapshot`/`telemetry()`/`reset_telemetry()`/`telemetry_snapshot()` (eviction, write/read lock-contention counters) | `Cargo.toml:10`; `src/lib.rs:19-20,35-67,164-176,341-351,386-396,439-449,479-489,530-540,612-632`; `interfaces/src/imemory_tier.rs:7-16,202-206` | ~90 | Add a new User Story / FR block: "Operational telemetry (eviction count, lock contention counters) exposed via optional `telemetry` feature and `telemetry_snapshot()`." |
| `free_capacity()` inherent method (capacity − used) | `src/lib.rs:178-186` | 9 | Add FR: "free_capacity() returns capacity() − used() for proactive-eviction triggers" (overlaps SC-5's "operator... query capacity/usage" intent but is a distinct, unspecced accessor). |
| `DEFAULT_POOL_SIZE` constant declared but never consumed by any call site (initialize() always takes an explicit `pool_size` argument) | `src/lib.rs:33` | 1 | Either wire it in as an actual default (e.g., a `new_default()`-style constructor) or remove/note as reserved-for-future in plan.md, matching how `NotEvictable` is already documented there. |

## Conflicts

None found — no contradictory statements between separate spec documents (`spec.md`, `plan.md`, `tasks.md` are internally consistent with each other on the sharded design; the conflict is entirely spec-vs-code, captured above).

## Recommendations

1. **Resolve the sharding discrepancy first** (highest priority). Either:
   - (a) Implement the 16-shard design as originally specified (shard-per-Mutex, key-modulo-16 selection, round-robin eviction counter, per-shard `IEvictionPolicy` pools) — this was evidently the original intent per `plan.md`'s architecture diagram, or
   - (b) Update `spec.md`, `plan.md`, `tasks.md`, and the `IMemoryTier` interface doc comments (P4/P5/P10 claims in `imemory_tier.rs`) to describe the current single-pool design and drop/relabel FR-005, FR-006, FR-007, NFR-002, and the shard-related Creusot property claims.
2. Fix `evict_lru_for_key()` (`src/lib.rs:465-467`) so it is not a dead alias for `evict_lru()` — either give it real key-scoped semantics (requires (1a)) or rename/deprecate it and update FR-014 accordingly if a global-only eviction API is the intended final design.
3. Reconcile the three conflicting version numbers: spec's NFR-008 ("0.2.0"), `Cargo.toml` (`"0.1.0"`), and the `define_component!` macro (`"0.3.0"`). Pick one source of truth.
4. Either produce the Creusot proof artifacts claimed by SC-8 and the interface doc comments, or remove/soften those claims until the underlying (currently nonexistent) shard invariants are implemented.
5. Backfill FR/User-Story coverage for the `telemetry` feature and `free_capacity()`, or remove them if they are considered experimental/dead code.
6. Minor: `README.md` describes an `LruList`/`lru.rs` module ("index-based doubly-linked list") that does not exist in `src/` — eviction is fully delegated to the external `IEvictionPolicy` receptacle instead. Update `README.md`'s Architecture and Source Layout sections to match.
