# Quickstart: SPDK CPU Memory Allocator Component

**Date**: 2026-04-10
**Feature**: 001-spdk-memory-alloc

## Prerequisites

- Rust 1.75+ toolchain
- SPDK pre-built native libraries at `deps/spdk-build/`
- Hugepages configured on the host system
- VFIO module loaded (for SPDK environment initialization)

## Build

The crate is an SPDK-dependent workspace member (not in `default-members`). Build explicitly:

```bash
cargo build -p memory-allocator-cpu
```

## Usage

```rust
use memory_allocator_cpu::MemoryAllocatorCpu;
use interfaces::{IMemoryManagement, ISPDKEnv};
use spdk_env::SPDKEnvComponent;
use example_logger::LoggerComponent;
use component_framework::prelude::*;
use component_core::iunknown::query;

// 1. Create components
let logger = LoggerComponent::new();
let env = SPDKEnvComponent::new(Default::default(), Default::default());
let allocator = MemoryAllocatorCpu::new_default();

// 2. Wire receptacles
let ilogger = query::<dyn ILogger + Send + Sync>(&*logger).unwrap();
env.logger.connect(ilogger).unwrap();

let ispdk_env = query::<dyn ISPDKEnv + Send + Sync>(&*env).unwrap();
allocator.spdk_env.connect(ispdk_env).unwrap();

// 3. Initialize SPDK environment
env.init().unwrap();

// 4. Allocate DMA memory (4096 bytes, 4096 alignment, any NUMA node)
let buf = allocator.allocate(4096, 4096, None).unwrap();
assert_eq!(buf.len(), 4096);

// 5. Allocate zero-initialized memory on NUMA node 0
let zbuf = allocator.zmalloc(8192, 512, Some(0)).unwrap();
assert!(zbuf.as_slice().iter().all(|&b| b == 0));

// 6. Check stats
let stats = allocator.stats();
assert_eq!(stats.total_allocation_count, 2);
assert_eq!(stats.total_bytes_allocated, 4096 + 8192);

// 7. Reallocate (grow buffer)
let buf = allocator.reallocate(buf, 8192, 4096, None).unwrap();
assert_eq!(buf.len(), 8192);

// 8. Free memory
allocator.free(buf);
allocator.free(zbuf);

// 9. Verify stats after free
let stats = allocator.stats();
assert_eq!(stats.total_allocation_count, 0);
assert_eq!(stats.total_bytes_allocated, 0);
```

## Running Tests

```bash
cargo test -p memory-allocator-cpu
```

Note: Tests that require actual SPDK initialization need hugepages and VFIO. Unit tests use the component framework's receptacle/interface patterns for testing without live SPDK.

## Running Benchmarks

```bash
cargo bench -p memory-allocator-cpu
```

Criterion benchmarks measure allocation, zmalloc, free, reallocate, and stats query latency.

## Key Types

| Type | Crate | Description |
|------|-------|-------------|
| `MemoryAllocatorCpu` | memory-allocator-cpu | The component |
| `IMemoryManagement` | interfaces | Interface trait |
| `DmaBuffer` | interfaces | Memory handle |
| `AllocationStats` | interfaces | Stats snapshot |
| `ZoneStats` | interfaces | Per-NUMA-zone stats |
| `MemoryAllocatorError` | interfaces | Error type |
