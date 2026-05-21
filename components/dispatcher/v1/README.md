# Dispatcher v1

Dispatcher component for the Certus storage system with DRAM memory-tier caching. Orchestrates cache operations (populate, lookup, check, remove) using a GPU-to-DRAM-to-SSD data flow with LRU-managed memory pool, write-through persistence, pipelined SSD-to-GPU promotion, and SSD eviction.

## Summary

- **Zero-copy pipeline**: NVMe reads directly into CUDA-pinned + SPDK-registered memory-tier slots, then async H2D DMA to GPU — no intermediate CPU memcpy. Pipeline depth of 16 concurrent NVMe reads. Falls back to ring-buffer path for unregistered memory.
- **Memory-tier caching**: Populate writes GPU data to a DRAM pool (via `IMemoryTier`) instead of a temporary staging buffer. Pool is registered at init for zero-copy DMA.
- **Pipelined SSD-to-GPU promotion**: Lookups from SSD promote entries back into the memory-tier via `pipeline::pipelined_ssd_to_gpu_zero_copy`
- **Async warm-path DMA**: Memory-tier lookups use `dma_copy_to_device_async` + `stream_synchronize` from CUDA-pinned memory (~10 GB/s vs ~2.4 GB/s sync)
- **Background write-through**: Async worker thread persists memory-tier entries to SSD without blocking the caller
- **SSD eviction**: Background evictor reclaims SSD extents when capacity thresholds are reached

## Structure

```
src/
  lib.rs           Component definition (DispatcherComponentV0), IDispatcher impl
  pipeline.rs      Zero-copy + ring-buffer pipelined SSD→DRAM→GPU reader (dual CUDA streams)
  background.rs    BackgroundWriter (memory-tier to SSD) and BackgroundEvictor
  io_segmenter.rs  MDTS-aware I/O splitting (128 KiB default chunks)

tests/
  integration.rs      Hardware integration tests (behind `hardware-test` feature)
  lazy_migration.rs   Mock-based tests for background write-through migration

benches/
  dispatcher_benchmark.rs      Mock-based dispatcher benchmarks
  ssd_evictor_benchmark.rs     SSD eviction benchmarks
  pipeline_hw_benchmark.rs     Hardware pipeline benchmarks (requires `hardware-test`)
  dispatcher_hw_benchmark.rs   Hardware dispatcher benchmarks (requires `hardware-test`)
```

### Component Wiring

```
DispatcherComponentV0 --> [IDispatcher provider]
                      <-- [ILogger receptacle]
                      <-- [IDispatchMap receptacle]
                      <-- [IGpuServices receptacle]
                      <-- [ISPDKEnv receptacle]
                      <-- [IMemoryTier receptacle]
```

### Data Flow

```
populate: GPU --DMA--> Memory-Tier Slot --write-through--> SSD (via extent manager)
warm:     Memory-Tier --async H2D DMA--> GPU (CUDA-pinned, ~10 GB/s)
cold:     SSD --NVMe DMA--> Memory-Tier --async H2D DMA--> GPU (zero-copy, ~3.3 GB/s)
```

## Build and Test

```bash
# Build
cargo build -p dispatcher-v1

# Unit and lazy migration tests (no hardware required)
cargo test -p dispatcher-v1

# Hardware integration tests (requires SPDK, NVMe, hugepages)
cargo test -p dispatcher-v1 --features hardware-test --test integration -- --test-threads=1

# Lint and docs
cargo clippy -p dispatcher-v1 -- -D warnings
cargo doc -p dispatcher-v1 --no-deps

# Benchmarks
cargo bench -p dispatcher-v1
cargo bench -p dispatcher-v1 --features hardware-test --bench pipeline_hw_benchmark
cargo bench -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark
```

Hardware tests require SPDK built at `deps/spdk-build/`, NVMe devices bound to VFIO, hugepages, IOMMU enabled, and `memlock` set to unlimited. Use `--test-threads=1` because SPDK is a process-wide singleton.
