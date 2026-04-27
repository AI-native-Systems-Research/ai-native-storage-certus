# Quickstart: Dispatch Map Component

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
use dispatch_map::{DispatchMapComponentV0, DispatchMapState};
use interfaces::{IDispatchMap, IExtentManager, ILogger};
use component_core::{query_interface, iunknown::IUnknown};
use std::time::Duration;

// 1. Create the component
let component = DispatchMapComponentV0::new(DispatchMapState::new());

// 2. Bind receptacles
component.connect_receptacle_raw("logger", &*logger_component).unwrap();
component.connect_receptacle_raw("extent_manager", &*extent_manager_component).unwrap();

// 3. Set DMA allocator and initialize (recovers extents)
let dm = query_interface!(component, IDispatchMap).unwrap();
dm.set_dma_alloc(dma_alloc_fn);
dm.initialize().unwrap();

// 4. Stage new data
let ptr = dm.create_staging(42, 4).unwrap();  // 4 x 4KiB blocks
// ... write data to ptr ...
dm.release_write(42).unwrap();  // release implicit write ref

// 5. Commit to storage
dm.take_write(42, Duration::from_secs(5)).unwrap();
dm.convert_to_storage(42, 8192, 1).unwrap();
dm.release_write(42).unwrap();

// 6. Read back
let result = dm.lookup(42, Duration::from_secs(5)).unwrap();
// result is BlockDevice { offset: 8192, device_id: 1 }
// read_ref automatically incremented
// ... perform I/O using offset ...
dm.release_read(42).unwrap();

// 7. Clean up
dm.remove(42).unwrap();
```

## Typical Write Flow

```
create_staging(key, size)   → DMA buffer ptr + write_ref=1
write data to buffer        → caller does I/O
release_write(key)          → write_ref=0
take_write(key, timeout)    → write_ref=1 (for convert)
convert_to_storage(key, offset, device_id)  → BlockDevice, DMA freed
```

## Typical Read Flow

```
lookup(key, timeout)        → blocks if writer active, then read_ref++
read data from ptr/offset   → caller does I/O
release_read(key)           → read_ref--
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
