# Dispatcher v0

Dispatcher component for the Certus storage system. Orchestrates cache operations (populate, lookup, check, remove) using GPU-to-SSD data flows via DMA staging buffers. This is the simpler version without a DRAM memory-tier -- data goes directly from GPU to a staging buffer and then to SSD.

## Summary

- Direct GPU-to-SSD caching via DMA staging buffers (no memory-tier)
- Background writer asynchronously persists staging data to SSD
- MDTS-aware I/O segmentation for NVMe transfers
- Watermark-based eviction of staging buffers
- Dispatch map with read/write reference locking for concurrency

## Structure

```
src/
  lib.rs           Component definition (DispatcherComponentV0), IDispatcher impl
  background.rs    BackgroundWriter for async staging-to-SSD writes
  io_segmenter.rs  MDTS-aware I/O splitting (128 KiB default chunks)

tests/
  integration.rs      Hardware integration tests (behind `hardware-test` feature)
  lazy_migration.rs   Mock-based tests for background write migration

benches/
  dispatcher_benchmark.rs   Mock-based dispatcher benchmarks
```

### Component Wiring

```
DispatcherComponentV0 --> [IDispatcher provider]
                      <-- [ILogger receptacle]
                      <-- [IDispatchMap receptacle]
                      <-- [IGpuServices receptacle]
                      <-- [ISPDKEnv receptacle]
```

### Data Flow

```
populate: GPU --DMA--> Staging Buffer --async--> SSD (via extent manager)
lookup:   SSD/Staging --DMA--> GPU
```

## Build and Test

```bash
# Build
cargo build -p dispatcher

# Unit and lazy migration tests (no hardware required)
cargo test -p dispatcher

# Hardware integration tests (requires SPDK, NVMe, hugepages)
cargo test -p dispatcher --features hardware-test --test integration -- --test-threads=1

# Lint and docs
cargo clippy -p dispatcher -- -D warnings
cargo doc -p dispatcher --no-deps

# Benchmarks
cargo bench -p dispatcher
```

Hardware tests require SPDK built at `deps/spdk-build/`, NVMe devices bound to VFIO, hugepages, IOMMU enabled, and `memlock` set to unlimited. Use `--test-threads=1` because SPDK is a process-wide singleton.
