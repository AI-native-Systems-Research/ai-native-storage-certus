# Dispatcher

Dispatcher component for the Certus storage system. Orchestrates GPU-to-SSD
cache operations (populate, lookup, check, remove) through a DRAM memory-tier
with LRU eviction and write-through persistence to NVMe SSDs. The component
implements the `IDispatcher` interface and coordinates N data block devices with
N extent managers for striped persistent storage.

Data enters via GPU IPC handles, lands in a CUDA-pinned DRAM pool managed by
`IMemoryTier`, and is asynchronously written through to SSD by a background
worker. Cache lookups that hit the memory-tier are served via async H2D DMA
(~10 GB/s from pinned memory). Cold lookups promote SSD-resident entries back
into the memory-tier using a zero-copy pipelined reader that overlaps NVMe reads
with GPU DMA transfers across dual CUDA streams.

## Architecture

### Component Wiring

```
DispatcherComponent --> [IDispatcher provider]
                    <-- [ILogger receptacle]
                    <-- [IDispatchMap receptacle]
                    <-- [IGpuServices receptacle]
                    <-- [ISPDKEnv receptacle]
                    <-- [IMemoryTier receptacle]
```

**Lifecycle**: `new_default()` -> bind receptacles -> `initialize(config)` -> use `IDispatcher` methods -> `shutdown()`.

Block devices and extent managers are created internally during `initialize()` based on the `DispatcherConfig` PCI addresses. If the `ISPDKEnv` receptacle is not connected, the component operates in staging-only mode (for unit testing without hardware).

### Data Flow

```
populate: GPU --DMA--> Memory-Tier Slot --background write-through--> SSD
warm:     Memory-Tier --async H2D DMA--> GPU (CUDA-pinned, ~10 GB/s)
cold:     SSD --NVMe DMA--> Memory-Tier --async H2D DMA--> GPU (zero-copy pipeline)
```

### Internal Modules

| Module | Purpose |
|--------|---------|
| `lib.rs` | Component definition (`DispatcherComponent`), `IDispatcher` implementation |
| `pipeline.rs` | Zero-copy and ring-buffer pipelined SSD-to-DRAM-to-GPU reader with dual CUDA streams |
| `background.rs` | `BackgroundWriter` (async memory-tier to SSD persistence) and `BackgroundEvictor` (SSD capacity reclamation) |
| `io_segmenter.rs` | MDTS-aware I/O splitting into device-safe transfer segments (128 KiB default) |

### Key Design Points

- **Zero-copy pipeline**: NVMe reads directly into CUDA-pinned + SPDK-registered memory-tier slots, then issues async H2D DMA with no intermediate CPU memcpy. Pipeline depth of 16 concurrent NVMe reads.
- **Ring-buffer fallback**: For unregistered memory, uses a pre-allocated ring of 8 CUDA-pinned DMA buffers with per-chunk CPU memcpy.
- **SSD eviction**: Background evictor monitors extent-manager utilization and reclaims space by removing the oldest dispatch-map entries when a configurable threshold is exceeded.

## Build

This crate requires SPDK dependencies and is not a default workspace member.

```bash
cargo build -p dispatcher
```

## Test

```bash
# Unit and lazy-migration tests (no hardware required, uses mocks)
cargo test -p dispatcher

# Hardware integration tests (requires SPDK, NVMe devices, hugepages, IOMMU)
cargo test -p dispatcher --features hardware-test --test integration -- --test-threads=1
```

Hardware tests require SPDK built at `deps/spdk-build/`, NVMe devices bound to VFIO, hugepages allocated, IOMMU enabled, and `memlock` set to unlimited. Use `--test-threads=1` because SPDK is a process-wide singleton.

## Benchmarks

```bash
# All benchmarks (mock-based, no hardware)
cargo bench -p dispatcher

# Hardware benchmarks (require SPDK + NVMe)
cargo bench -p dispatcher --features hardware-test --bench pipeline_hw_benchmark
cargo bench -p dispatcher --features hardware-test --bench dispatcher_hw_benchmark
```

Available benchmark suites:

| Benchmark | Feature gate | Description |
|-----------|-------------|-------------|
| `dispatcher_benchmark` | none | Mock-based dispatcher operation benchmarks |
| `ssd_evictor_benchmark` | none | SSD eviction logic benchmarks |
| `pipeline_hw_benchmark` | `hardware-test` | Pipelined SSD-to-GPU transfer on real hardware |
| `dispatcher_hw_benchmark` | `hardware-test` | Full dispatcher operations on real hardware |
