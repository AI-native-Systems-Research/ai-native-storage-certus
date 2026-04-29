# Dispatcher v0

Dispatcher component for the Certus storage system. Orchestrates cache operations
(populate, lookup, check, remove) using GPU-to-SSD data flows via DMA staging buffers.

## Interface

Provides the `IDispatcher` interface with methods:

- `initialize(config)` — Create and initialize N data block devices and extent managers
- `shutdown()` — Complete in-flight background writes and release resources
- `populate(key, ipc_handle)` — Cache GPU data: staging buffer allocation, DMA copy, async SSD write
- `lookup(key, ipc_handle)` — Retrieve cached data: DMA copy from staging or SSD to GPU
- `check(key)` — Check cache entry presence without data transfer
- `remove(key)` — Evict cache entry, freeing staging buffer and/or SSD extent

## Component Wiring

```
DispatcherComponentV0 --> [IDispatcher provider]
                      <-- [ILogger receptacle]
                      <-- [IDispatchMap receptacle]
```

Block devices and extent managers are created internally during `initialize()` based
on the `DispatcherConfig` PCI addresses.

## Building

```bash
cargo build -p dispatcher
cargo test -p dispatcher
cargo clippy -p dispatcher -- -D warnings
cargo doc -p dispatcher --no-deps
cargo bench -p dispatcher
```

## Architecture

### Data Flow

```
populate: GPU --DMA--> Staging Buffer --async--> SSD (via extent manager)
lookup:   SSD/Staging --DMA--> GPU
```

### Internal Modules

- `io_segmenter` — MDTS-aware I/O splitting (128 KiB default)
- `background` — Async staging-to-SSD write worker thread

### Concurrency

The dispatcher relies on the dispatch map's built-in read/write reference locking:
- Multiple concurrent lookups on different keys proceed in parallel
- Lookup blocks if a populate write is active on the same key
- Remove blocks until any in-flight background write completes
- Fixed 100ms timeout for blocking operations
