# Quickstart: Dispatcher Cache Interface

## Build

```bash
cargo build -p dispatcher
```

## Test

```bash
cargo test -p dispatcher                    # Unit + integration tests
cargo test -p dispatcher --doc              # Documentation tests
cargo clippy -p dispatcher -- -D warnings   # Lint
cargo fmt -p dispatcher --check             # Format check
cargo doc -p dispatcher --no-deps           # Documentation
```

## Benchmark

```bash
cargo bench -p dispatcher
```

## Usage

```rust
use component_core::query_interface;
use interfaces::{IDispatcher, ILogger, IDispatchMap, DispatcherConfig, IpcHandle, CacheKey};

// 1. Create the dispatcher component
let dispatcher = DispatcherComponentV0::new();

// 2. Wire receptacles
dispatcher.logger.connect(logger_instance);
dispatcher.dispatch_map.connect(dispatch_map_instance);

// 3. Initialize with device configuration
let disp = query_interface!(dispatcher, IDispatcher).unwrap();
disp.initialize(DispatcherConfig {
    metadata_pci_addr: "0000:01:00.0".parse().unwrap(),
    data_pci_addrs: vec![
        "0000:02:00.0".parse().unwrap(),
        "0000:03:00.0".parse().unwrap(),
    ],
})?;

// 4. Populate a cache entry (GPU → staging → SSD async)
let key: CacheKey = 42;
let handle = IpcHandle { address: gpu_ptr, size: 4096 };
disp.populate(key, handle)?;

// 5. Check if entry exists
assert!(disp.check(key)?);

// 6. Lookup (SSD/staging → GPU DMA transfer)
let dest = IpcHandle { address: dest_gpu_ptr, size: 4096 };
disp.lookup(key, dest)?;

// 7. Remove entry
disp.remove(key)?;

// 8. Shutdown
disp.shutdown()?;
```

## File Layout

```
components/interfaces/src/
  idispatcher.rs         # IDispatcher interface + DispatcherError + IpcHandle + DispatcherConfig

components/dispatcher/v0/
  src/
    lib.rs               # Component definition + IDispatcher implementation + tests
    io_segmenter.rs      # MDTS-aware I/O splitting
    background.rs        # Async staging-to-SSD write worker thread
  benches/
    dispatcher_benchmark.rs   # Criterion benchmarks
```
