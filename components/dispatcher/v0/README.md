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
                      <-- [IGpuServices receptacle]
                      <-- [ISPDKEnv receptacle]
```

Block devices and extent managers are created internally during `initialize()` based
on the `DispatcherConfig` PCI addresses.

## Building

```bash
cargo build -p dispatcher
cargo test -p dispatcher
cargo test -p dispatcher --features hardware-test --test integration -- --test-threads=1
cargo clippy -p dispatcher -- -D warnings
cargo doc -p dispatcher --no-deps
cargo bench -p dispatcher
```

## Tests

### Unit Tests

Standard mock-based tests covering all `IDispatcher` methods, error paths, and
concurrency. No hardware required.

```bash
cargo test -p dispatcher
```

### Lazy Migration Tests

`tests/lazy_migration.rs` — verifies the background writer migrates staging entries
to block-device state and that lookups/checks still succeed post-migration. Uses
mock infrastructure (no hardware).

### Hardware Integration Tests

`tests/integration.rs` — exercises the full stack with real NVMe devices via SPDK.
Gated behind the `hardware-test` feature flag.

**Test cases:**

- `hw_idispatcher_full_integration` — comprehensive test of every `IDispatcher` method
  (populate, lookup, check, remove, shutdown) including error cases, batch operations,
  concurrent access, various buffer sizes, and lazy migration verification
- `hw_multi_device_initialization` — multi-NVMe-device setup (runs if 2+ devices available)
- `hw_data_integrity` — verifies byte-for-byte data integrity through the cache:
  deterministic patterns, multi-block, non-aligned sizes, large buffers (MDTS
  segmentation), edge patterns (all-zeros/ones), cross-key contamination checks,
  and concurrent populate/verify

**Prerequisites:**

- SPDK built at `deps/spdk-build/`
- NVMe devices bound to VFIO (`dpdk-devbind.py`)
- Hugepages configured (at least 2 GiB recommended)
- IOMMU enabled in kernel boot params
- `memlock` set to unlimited (`ulimit -l unlimited`)

**Run with:**

```bash
cargo test -p dispatcher --features hardware-test --test integration -- --test-threads=1
```

**Important:** The `--test-threads=1` flag is required. SPDK is a process-wide
singleton and NVMe controllers cannot be re-probed (shared) after detach within the same
process. Running tests in parallel will cause `AlreadyInitialized` errors.

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
