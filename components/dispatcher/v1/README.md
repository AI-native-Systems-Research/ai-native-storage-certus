# Dispatcher v1

Dispatcher component for the Certus storage system with DRAM memory-tier caching.
Orchestrates cache operations (populate, lookup, check, remove) using a GPU-to-DRAM-to-SSD
data flow with LRU-managed memory pool and write-through persistence.

## Interface

Provides the `IDispatcher` interface with methods:

- `initialize(config)` — Create and initialize N data block devices and extent managers
- `shutdown()` — Complete in-flight background writes and release resources
- `populate(key, ipc_handle)` — Cache GPU data: DMA copy to memory-tier slot, async write-through to SSD
- `lookup(key, ipc_handle)` — Retrieve cached data: from memory-tier (fast), SSD with promotion, or legacy staging
- `check(key)` — Check cache entry presence without data transfer
- `remove(key)` — Evict cache entry, freeing memory-tier slot and/or SSD extent

## Component Wiring

```
DispatcherComponentV1 --> [IDispatcher provider]
                      <-- [ILogger receptacle]
                      <-- [IDispatchMap receptacle]
                      <-- [IGpuServices receptacle]
                      <-- [ISPDKEnv receptacle]
                      <-- [IMemoryTier receptacle]
```

Block devices and extent managers are created internally during `initialize()` based
on the `DispatcherConfig` PCI addresses. If the ISPDKEnv receptacle is not connected,
operates in staging-only mode (for unit testing without hardware).

## Building

```bash
cargo build -p dispatcher-v1
cargo test -p dispatcher-v1
cargo test -p dispatcher-v1 --features hardware-test --test integration -- --test-threads=1
cargo clippy -p dispatcher-v1 -- -D warnings
cargo doc -p dispatcher-v1 --no-deps
cargo bench -p dispatcher-v1
```

## Tests

### Unit Tests

Standard mock-based tests covering all `IDispatcher` methods, error paths, and
concurrency. No hardware required.

```bash
cargo test -p dispatcher-v1
```

### Lazy Migration Tests

`tests/lazy_migration.rs` — verifies the background writer migrates memory-tier entries
to block-device state and that lookups/checks still succeed post-migration. Uses
mock infrastructure (no hardware).

### Hardware Integration Tests

`tests/integration.rs` — exercises the full stack with real NVMe devices via SPDK.
Gated behind the `hardware-test` feature flag.

**Prerequisites:**

- SPDK built at `deps/spdk-build/`
- NVMe devices bound to VFIO (`dpdk-devbind.py`)
- Hugepages configured (at least 2 GiB recommended)
- IOMMU enabled in kernel boot params
- `memlock` set to unlimited (`ulimit -l unlimited`)

**Run with:**

```bash
cargo test -p dispatcher-v1 --features hardware-test --test integration -- --test-threads=1
```

**Important:** The `--test-threads=1` flag is required. SPDK is a process-wide
singleton and NVMe controllers cannot be re-probed after detach within the same
process. Running tests in parallel will cause `AlreadyInitialized` errors.

## Architecture

### Data Flow

```
populate: GPU --DMA--> Memory-Tier Slot --write-through--> SSD (via extent manager)
lookup:   Memory-Tier/SSD --DMA--> GPU (with promotion from SSD to memory-tier)
```

### Key Differences from v0

- **Memory-tier caching**: Populate writes to a DRAM pool (via `IMemoryTier`) instead of a staging buffer
- **Capacity-based eviction**: LRU eviction via `IMemoryTier::evict_lru()` when the pool is full
- **Pipelined promotion**: Lookup from SSD promotes entries back to memory-tier via `pipeline::pipelined_ssd_to_gpu`
- **Write-through**: Background writer persists memory-tier entries to SSD asynchronously

### Internal Modules

- `io_segmenter` — MDTS-aware I/O splitting (128 KiB default)
- `background` — Async memory-tier-to-SSD write-through worker thread
- `pipeline` — Pipelined SSD-to-GPU reads with ring-buffer reader

### Concurrency

The dispatcher relies on the dispatch map's built-in read/write reference locking:
- Multiple concurrent lookups on different keys proceed in parallel
- Lookup blocks if a populate write is active on the same key
- Remove blocks until any in-flight background write completes
