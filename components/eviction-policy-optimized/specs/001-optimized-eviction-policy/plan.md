# Implementation Plan: Optimized (TinyLFU) Eviction Policy

**Branch**: `001-optimized-eviction-policy` | **Date**: 2026-09-23 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

An eviction-policy component that combines an O(1) index-based LRU list with a
per-pool Count-Min Sketch frequency estimator to implement TinyLFU admission
control. On `track`, the sketch decides whether a key enters the list at the
protected (MRU) end or the eviction-candidate (LRU) end. Otherwise it presents
the same multi-pool `IEvictionPolicy` surface as `eviction-policy-lru` and is a
drop-in replacement for it.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-framework` / `component-macros` — component model macros, lifecycle
- `component-core` — `query_interface!`
- `interfaces` — `IEvictionPolicy` trait and associated types

**Performance Goals**: O(1) single-entry operations; bounded, key-count-
independent sketch memory; per-pool locking to minimize contention.

## Architecture

### Component Layer

```
EvictionPolicyOptimizedComponent
├── provides: IEvictionPolicy
├── receptacles: ILogger
└── fields:
    └── state: RwLock<EvictionState>
        └── pools: Vec<Mutex<Pool>>
            └── Pool { lru: LruList, sketch: CountMinSketch,
                       access_count: u64, max_len: usize }
```

### Data Structures

```
LruList (src/lru_list.rs)      # identical to eviction-policy-lru
├── nodes: Vec<Node>           # index-addressed
├── head / tail: Option<u32>   # front = LRU (victim), back = MRU (protected)
├── free: Vec<u32>             # recycled slots
└── len: usize
Node { key: u64, prev, next: Option<u32>, active: bool }

CountMinSketch (src/lib.rs)    # per-pool frequency estimator
└── counters: [[u8; CMS_COLS]; CMS_ROWS]   # 4 × 1024 saturating u8
    increment(key) : +1 saturating on each row's bucket
    estimate(key)  : min over rows
    halve()        : every counter >>= 1   (aging)
    bucket = (key.wrapping_mul(CMS_PRIMES[row]) >> 54)  # top 10 bits → 1024
```

### `track` Flow (the admission gate)

1. `sketch.increment(key)`.
2. `max_len = max(max_len, lru.len())`; `access_count += 1`.
3. **Aging**: if `max_len > 0 && access_count % (max_len*10) == 0` → `sketch.halve()`.
4. **Admission**:
   - If the list has a head (victim) key:
     - `new_est = estimate(key)`, `victim_est = estimate(head_key)`.
     - `new_est <= victim_est` → `push_front` (LRU head, first to be evicted).
     - else (`new_est > victim_est`) → `push_back` (MRU, protected).
   - Else (empty list) → `push_back`.
5. Return `EvictionHandle(pool, index)`.

### Concurrency Model

Identical to `eviction-policy-lru`: `RwLock<EvictionState>` read-locked for all
per-pool operations and write-locked only for `create_pool`; each pool guarded
by its own `Mutex`. The sketch lives inside the per-pool `Mutex`, so frequency
updates never contend across pools.

### Operation / Complexity Table

| Operation | Lock | Complexity |
|-----------|------|-----------|
| `create_pool()` | write(state) | O(1) amortized |
| `track(pool, key)` | read(state) + lock(pool) | O(1) (+ O(CMS_ROWS) sketch, O(CMS) on aging ticks) |
| `touch(handle)` | read(state) + lock(pool) | O(1) |
| `batch_touch(handles)` | read(state) + lock(pool) per run | O(k), one lock per pool-run |
| `remove(handle)` | read(state) + lock(pool) | O(1) |
| `identify_next_to_evict(pool)` | read(state) + lock(pool) | O(1) |
| `get_eviction_candidates(pool, n)` | read(state) + lock(pool) | O(n) |
| `len(pool)` / `clear_pool(pool)` | read(state) + lock(pool) | O(1) |

### Key Design Decisions

1. **TinyLFU admission over pure recency**: gate new keys on estimated
   frequency so a key is protected (`push_back`) only when it looks more
   frequently seen than the current victim; otherwise it enters at the LRU head
   as the immediate next eviction candidate.
2. **Count-Min Sketch for frequency**: fixed `4×1024` `u8` array gives
   bounded, key-count-independent memory with O(1) estimate; minimum-over-rows
   reduces overestimation from hash collisions.
3. **Periodic halving (aging)**: every `max_len*10` accesses, halving lets the
   estimator forget stale popularity and adapt to a shifting working set while
   keeping counters within `u8` range.
4. **Reuse of the LRU list unchanged**: only the insertion end differs from
   `eviction-policy-lru`, keeping the O(1) touch/remove/pop machinery and its
   free-list recycling shared and well-tested.
5. **Per-pool sketch inside the pool Mutex**: no extra locking; sketches are
   isolated per eviction domain.

## Project Structure

```text
components/eviction-policy-optimized/
├── Cargo.toml
├── src/
│   ├── lib.rs          # Component def, CountMinSketch, IEvictionPolicy impl, tests
│   └── lru_list.rs     # Index-based doubly-linked list, unit tests
└── specs/001-optimized-eviction-policy/
    ├── spec.md
    ├── plan.md
    └── tasks.md
```

## Dependencies (Consumer Graph)

```
eviction-policy-optimized
├── dispatch-map     (1 pool, via full-optimized profile wiring)
├── memory-tier      (16 pools, via full-optimized profile wiring)
└── certus-server-yaml  (CERTUS_PROFILE=full-optimized selects this crate)
```

## Testing

- **Unit tests** (`src/lru_list.rs`): shared list mechanics (push/pop,
  move_to_back, remove head/middle/tail, free-list reuse, len, clear, peek).
- **Integration tests** (`src/lib.rs`): sequential pool IDs, track/evict order
  for first-time keys, touch reordering, remove invalidation, non-destructive
  candidate peek, pool independence, invalid-pool degradation, clear-and-retrack,
  concurrent 4×100 stress.

## Future Considerations

- Make the aging period (`max_len * 10`) configurable rather than a hard-coded
  constant, and consider adding an absolute admission threshold if a pure
  victim-comparison gate proves too permissive under bursty first-touch traffic.
- Add tests that exercise the admission gate directly (a hot key protected
  against a flood of cold keys), which the current suite only covers implicitly.
- Revisit whether `clear_pool` should reset the sketch.
- Consider a doorkeeper/bloom front for the sketch to further cut cold-key cost.
