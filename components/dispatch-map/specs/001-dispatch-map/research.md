# Research: Dispatch Map Component

**Date**: 2026-04-27
**Feature**: Dispatch Map v0

## R1: Synchronization Primitive for Blocking with Timeout

**Decision**: `std::sync::Mutex` + `std::sync::Condvar` with `wait_timeout`.

**Rationale**: The dispatch map requires blocking operations (take_read, take_write, lookup) that wait for reference count conditions with a configurable timeout. `Condvar::wait_timeout` provides exactly this: a thread-safe blocking primitive that releases the mutex while waiting and allows timeout-based wakeup. This is part of `std` (no external dependency), well-tested, and straightforward to reason about.

**Alternatives considered**:
- `parking_lot::Mutex` + `Condvar`: Faster uncontended acquire, but adds an external dependency. Constitution VII mandates minimal dependencies. Can be swapped in later if std Mutex contention becomes measurable.
- `DashMap` + per-entry `AtomicU32` + `thread::park_timeout`: Lock-free per-entry access, but implementing blocking-with-timeout on atomic conditions requires custom parking logic (essentially reimplementing Condvar). Significantly more complex for no proven benefit in v0.
- `RwLock<HashMap>` + per-entry atomics: Allows concurrent reads of the map, but we still need blocking on ref count conditions, which requires Condvar and thus Mutex anyway. RwLock adds complexity without benefit.

## R2: Entry Compactness (≤32 bytes)

**Decision**: Store DMA buffer pointer + length in the `Location::Staging` variant; own the `DmaBuffer` object in a separate side map.

**Rationale**: `DmaBuffer` is 40+ bytes (ptr, len, free_fn, numa_node, BTreeMap metadata). Storing it inline would exceed the 32-byte target. By storing only the raw pointer and length in the entry (16 bytes for the Location enum), and keeping the owned `DmaBuffer` in `HashMap<CacheKey, DmaBuffer>`, the entry stays compact. The Mutex ensures the side map and entry map stay consistent.

**Alternatives considered**:
- `Arc<DmaBuffer>` in the entry: 8 bytes for the Arc pointer, but requires Arc overhead (refcount allocation) and complicates ownership (who drops the DmaBuffer?). The side map approach is simpler since the DmaBuffer lifecycle exactly matches the entry lifecycle.
- Inline DmaBuffer in entry: Exceeds 32-byte target. Rejected.

## R3: IDispatchMap Feature Gating

**Decision**: Gate `IDispatchMap` behind `#[cfg(feature = "spdk")]` in the interfaces crate.

**Rationale**: The interface references `DmaBuffer`, `DmaAllocFn`, and uses types from `spdk_types`. Following the established pattern of `IExtentManager` and `IBlockDevice` (both spdk-gated), the dispatch map interface must also be gated. The dispatch-map crate already depends on `interfaces` with `features = ["spdk"]`.

**Alternatives considered**:
- Ungated interface with abstract buffer type: Would require a generic or trait-object buffer type to avoid SPDK dependency. Adds abstraction complexity for no current consumer benefit. All dispatch map users are in the SPDK path.

## R4: Error Type Location

**Decision**: Define `DispatchMapError` in `interfaces/src/idispatch_map.rs`, alongside the trait.

**Rationale**: Follows the pattern of `ExtentManagerError` (defined in `iextent_manager.rs`), `NvmeBlockError` (defined in `iblock_device.rs`), and `SpdkEnvError` (defined in `spdk_types.rs`). Error types that appear in interface method signatures must be in the interfaces crate so consumers can match on them without depending on the implementation crate.

**Alternatives considered**:
- Error type in dispatch-map crate: Breaks interface-only exposure (Constitution II). Consumers would need to depend on the implementation crate to handle errors.

## R5: Recovery Mechanism

**Decision**: `initialize()` method that calls `IExtentManager::for_each_extent` to populate the map.

**Rationale**: The extent manager already provides `for_each_extent(&self, cb: &mut dyn FnMut(&Extent))` which iterates all committed extents. Each `Extent` contains `key`, `size`, and `offset`. The dispatch map can directly populate entries as `BlockDevice` locations from this data. This mirrors how the extent manager is used elsewhere in the system. The `extent_manager_id` for recovered entries can be derived from the bound receptacle identity.

**Alternatives considered**:
- Lazy recovery (populate on first lookup): Simpler but means the first lookup for any key would need to check the extent manager, adding latency to the hot path. Bulk recovery at init is predictable and front-loads the cost.
- Separate recovery interface: Unnecessary abstraction. The initialize method is the natural place for one-time setup work.

## R6: DMA Allocation Source

**Decision**: `set_dma_alloc(DmaAllocFn)` method on IDispatchMap, called during component setup.

**Rationale**: Mirrors the `IExtentManager::set_dma_alloc` pattern exactly. The `DmaAllocFn` is a `Arc<dyn Fn(usize, usize, Option<i32>) -> Result<DmaBuffer, String> + Send + Sync>` that the SPDK environment provides. The dispatch map needs it only for `create_staging` (DMA buffer allocation). Storing it as an `Option<DmaAllocFn>` field, set before first use, is the established pattern.

**Alternatives considered**:
- Pass allocator per-call: Clutters every `create_staging` signature. The allocator doesn't change.
- ISPDKEnv receptacle: Over-couples the dispatch map to SPDK environment management. The DmaAllocFn abstraction provides the minimal interface needed.
