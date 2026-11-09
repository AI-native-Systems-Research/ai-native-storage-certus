# Quickstart: Dispatch Map Component

**Synced 2026-07-22**: previous revision described a `set_dma_alloc`/`create_staging`
DMA-buffer API that no longer exists. Updated to match the current memory-tier API
and the mandatory `IEvictionPolicy` receptacle. See `.specify/sync/drift-report.md`.

## Build

```bash
cargo build -p dispatch-map
```

## Test

```bash
cargo test -p dispatch-map
cargo test -p dispatch-map -- --test-threads 1  # CI mode
```

## Usage

```rust,ignore
use dispatch_map::{DispatchMapComponent, DispatchMapState};
use interfaces::{IDispatchMap, IExtentManager, ILogger, IEvictionPolicy};
use component_core::{query_interface, iunknown::IUnknown};

// 1. Create the component
let component = DispatchMapComponent::new(DispatchMapState::new());

// 2. Bind receptacles — IEvictionPolicy is mandatory; ILogger and
//    IExtentManager are optional (IExtentManager enables recovery).
component.connect_receptacle_raw("eviction_policy", &*eviction_policy_component).unwrap();
component.connect_receptacle_raw("logger", &*logger_component).unwrap();
component.connect_receptacle_raw("extent_manager", &*extent_manager_component).unwrap();

// 3. Initialize (recovers extents from IExtentManager if bound; otherwise
//    succeeds with an empty map). Must be called explicitly.
let dm = query_interface!(component, IDispatchMap).unwrap();
dm.initialize().unwrap();

// 4. Create a memory-tier entry for new data (write_ref=1 on success)
let ptr: *mut u8 = /* externally-allocated DRAM pointer, e.g. from a memory-tier pool */;
dm.create_memory_tier_entry(42, ptr, 16384).unwrap();
// ... write data to ptr ...
dm.release_write(42).unwrap();

// 5. Write through to the block device: record the SSD offset, then
//    perform the explicit MemoryTier -> BlockDevice transition once
//    write-through completes.
dm.take_write(42).unwrap();
dm.convert_to_storage(42, 8192).unwrap();   // sets ssd_offset only
dm.release_write(42).unwrap();
dm.try_evict_to_block(42).unwrap();         // atomic evictability check + transition

// 6. Read back (now a BlockDevice entry)
let result = dm.lookup(42).unwrap();
// result is LookupResult::BlockDevice { offset: 8192 }
// read_ref is automatically incremented
// ... perform I/O using offset ...
dm.release_read(42).unwrap();

// 7. On a subsequent read miss for the same cold extent, the caller may
//    promote it back into the memory tier in place (preserving any
//    reference already held, e.g. by an in-flight load):
let new_ptr: *mut u8 = /* freshly-allocated DRAM pointer */;
dm.promote_block_to_memory_tier(42, new_ptr, 16384).unwrap();

// 8. Clean up (requires no active references)
dm.remove(42).unwrap();
```

## Typical Write Flow

```
create_memory_tier_entry(key, ptr, size)  → MemoryTier, write_ref=1
write data to buffer                      → caller does I/O
release_write(key)                        → write_ref=0
take_write(key)                           → write_ref=1 (for convert)
convert_to_storage(key, offset)           → sets ssd_offset (still MemoryTier)
release_write(key)                        → write_ref=0
try_evict_to_block(key)                   → BlockDevice (atomic; requires zero refs)
```

## Typical Read Flow

```
lookup(key)                  → blocks if writer active, then read_ref++
read data from ptr/offset    → caller does I/O
release_read(key)            → read_ref--
```

## Typical Promotion Flow (cold-block read miss)

```
lookup(key)                              → BlockDevice { offset }, read_ref++ (pinned)
allocate DRAM buffer, stage data from SSD → caller does I/O
promote_block_to_memory_tier(key, ptr, size) → MemoryTier in place; read_ref unchanged
release_read(key)                        → read_ref--
```

## File Layout

```
src/
├── lib.rs       # Component definition, IDispatchMap impl
├── entry.rs     # DispatchEntry, Location enum
└── state.rs     # DispatchMapState (Mutex + Condvar)
tests/
└── integration.rs
benches/
└── dispatch_map_benchmark.rs
```
