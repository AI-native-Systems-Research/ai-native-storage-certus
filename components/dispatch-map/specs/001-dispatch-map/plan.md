# Implementation Plan: Dispatch Map Component

**Branch**: `dispatch-map` | **Date**: 2026-04-27 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/001-dispatch-map/spec.md`

## Summary

Implement a thread-safe dispatch map component that maps extent keys (`CacheKey = u64`) to location metadata (staging DMA buffer or block-device offset) with readers-writer reference counting and configurable timeout blocking. The component uses `define_component!`, provides `IDispatchMap`, and consumes `ILogger` and `IExtentManager` receptacles. On initialization it recovers committed extents from the extent manager.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**: component-framework, component-core, component-macros, interfaces (with `spdk` feature)
**Storage**: In-memory `HashMap`; persistence delegated to `IExtentManager`
**Testing**: `cargo test -p dispatch-map`, Criterion benchmarks
**Target Platform**: Linux only (RHEL/Fedora)
**Project Type**: Library / component crate
**Performance Goals**: Non-blocking lookup when no writer active (SC-005); per-entry metadata ≤32 bytes (SC-004)
**Constraints**: Thread-safe, re-entrant; all operations return `Result` (no panics); SPDK DMA buffer compatible
**Scale/Scope**: Single component crate; single-namespace per instance (v0)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Component-Framework Conformance | PASS | Uses `define_component!`, `define_interface!`, typed receptacles for ILogger and IExtentManager |
| II. Interface-Only Exposure | PASS | All public API via `IDispatchMap` in interfaces crate; no pub items outside interface boundary |
| III. Code Quality and Correctness | PASS | Will enforce clippy -D warnings, fmt, doc --no-deps zero warnings, SAFETY comments for any unsafe |
| IV. Comprehensive Testing | PASS | Unit tests, doc tests, integration tests for wiring + recovery + concurrency; deterministic under --test-threads 1 |
| V. Performance Validation | PASS | Criterion benchmarks for lookup, ref operations, entry size assertion |
| VI. Documentation Standards | PASS | Doc comments with examples on all public items; module-level //! docs |
| VII. Maintainability | PASS | YAGNI: no eviction policy, no multi-namespace, no fairness guarantees in v0 |

No violations. No complexity tracking needed.

## Project Structure

### Documentation (this feature)

```text
specs/001-dispatch-map/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── idispatch_map.md
└── tasks.md             # Phase 2 output (via /speckit.tasks)
```

### Source Code

```text
components/interfaces/src/
├── idispatch_map.rs     # IDispatchMap trait + DispatchMapError + LookupResult (rewrite, spdk-gated)
└── lib.rs               # Updated re-exports

components/dispatch-map/v0/
├── Cargo.toml           # Add criterion dev-dependency
├── src/
│   ├── lib.rs           # Component definition, IDispatchMap impl, module declarations
│   ├── entry.rs         # DispatchEntry, Location enum, CacheKey type alias
│   ├── error.rs         # (lives in interfaces crate as DispatchMapError)
│   └── state.rs         # DispatchMapState: Mutex<HashMap> + Condvar, blocking logic
├── tests/
│   └── integration.rs   # Component wiring, recovery, concurrency, timeout tests
└── benches/
    └── dispatch_map_benchmark.rs  # Criterion: lookup, ref ops, entry size
```

**Structure Decision**: Single component crate following existing patterns (block-device-spdk-nvme, extent-manager). Interface types and error enum defined in the shared `interfaces` crate. Implementation entirely within `dispatch-map/v0/src/`.

## Architecture

### Synchronization Strategy

`Mutex<HashMap<CacheKey, DispatchEntry>>` + `Condvar` for blocking with timeout.

**Rationale**: Simple and correct for v0. The Mutex protects the entire map; per-entry reference counts are plain `u32` fields (no atomics needed since access is mutex-guarded). The Condvar handles `take_read`/`take_write` blocking: waiters loop on `condvar.wait_timeout()` checking ref count conditions. `release_read`/`release_write` call `condvar.notify_all()` to wake blocked waiters.

**Why not lock-free / DashMap**: Lock-free per-entry ref counting with blocking requires parking infrastructure and is significantly more complex. The Mutex + Condvar approach is proven, debuggable, and sufficient for v0. Can be optimized later if benchmarks show contention.

**Re-entrancy**: The API is designed so no method calls another IDispatchMap method while holding the lock, preventing self-deadlock. The Mutex is held only for short critical sections (hash lookup + ref count check/modify).

### Entry Layout (targeting SC-004: ≤32 bytes)

```
Location enum:         16 bytes  (Staging: *mut c_void + usize; BlockDevice: u64 + u16 + pad)
extent_manager_id:      2 bytes  (u16)
size_blocks:            4 bytes  (u32, 4KiB units)
read_ref:               4 bytes  (u32)
write_ref:              4 bytes  (u32, always 0 or 1)
                       ─────────
Total:                 30 bytes + padding ≈ 32 bytes
```

DMA buffers are owned in a side map (`HashMap<CacheKey, DmaBuffer>`) so the entry stores only a raw pointer + length. The read reference count guarantees the buffer is not freed while in use. When `convert_to_storage` is called, the DmaBuffer is removed from the side map and dropped.

### IDispatchMap Interface (interfaces crate)

The current stub `IDispatchMap` will be rewritten behind `#[cfg(feature = "spdk")]` (matching `IExtentManager` pattern) since it references `DmaBuffer` and `DmaAllocFn`. New types:

- `DispatchMapError` — error enum (KeyNotFound, ActiveReferences, Timeout, AllocationFailed, InvalidSize, NotInitialized, AlreadyExists)
- `LookupResult` — enum (NotExist, MismatchSize, Staging { ptr, len }, BlockDevice { offset, device_id })
- `CacheKey` — type alias for `u64`

### Recovery Flow

`initialize()` calls `self.extent_manager.for_each_extent(|extent| { ... })` to populate the map with BlockDevice entries for all committed extents. Each extent's key, offset, size, and extent_manager_id are recorded. After recovery, the map is ready for lookups.

### DMA Allocation

The component requires a `DmaAllocFn` (set via `set_dma_alloc`) to allocate staging buffers in `create_staging`. This mirrors the `IExtentManager::set_dma_alloc` pattern.

## Phasing

### Phase 1: Interface & Types (interfaces crate)

1. Define `DispatchMapError`, `LookupResult`, `CacheKey` types in `idispatch_map.rs`
2. Rewrite `IDispatchMap` trait with full method signatures (spdk-gated)
3. Update `lib.rs` re-exports

### Phase 2: Core Data Structures (dispatch-map crate)

1. Create `entry.rs` with `DispatchEntry`, `Location` enum
2. Create `state.rs` with `DispatchMapState` (Mutex + Condvar + HashMap)
3. Implement blocking logic: `wait_for_condition(key, predicate, timeout)` helper

### Phase 3: IDispatchMap Implementation

1. Implement all IDispatchMap methods in `lib.rs`
2. Wire component with `define_component!` (update from stub)
3. Implement `initialize()` recovery from IExtentManager
4. Add ILogger calls throughout

### Phase 4: Testing

1. Unit tests for entry/state modules
2. Doc tests on all public API items
3. Integration tests: wiring, recovery, concurrency, timeout, error paths
4. Verify single-threaded compatibility (`--test-threads 1`)

### Phase 5: Benchmarks & Polish

1. Criterion benchmarks: lookup latency, ref op throughput, entry size assertion
2. Clippy + fmt + doc clean pass
3. Update README.md if needed

## Complexity Tracking

No constitution violations to justify.
