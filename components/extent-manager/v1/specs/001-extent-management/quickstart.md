# Quickstart: Extent Management

**Feature**: 001-extent-management
**Date**: 2026-04-16

## Prerequisites

- Rust stable toolchain (MSRV 1.75, edition 2021)
- Linux operating system
- The workspace must have `component-framework`, `component-core`, `component-macros`, and `interfaces` crates accessible at the relative paths defined in Cargo.toml

## Build

```bash
cargo build -p extent-manager
```

## Run Tests

```bash
# All tests (unit + integration + doc tests)
cargo test -p extent-manager

# Single test by name
cargo test -p extent-manager test_create_and_lookup

# With output
cargo test -p extent-manager -- --nocapture
```

Tests run entirely in-memory using MockBlockDevice — no SPDK, hugepages, or NVMe hardware required.

## Run Benchmarks

```bash
# Run all benchmarks
cargo bench -p extent-manager

# Verify benchmarks compile (CI gate)
cargo bench -p extent-manager --no-run
```

## Code Quality

```bash
# Format check
cargo fmt -p extent-manager --check

# Lint (warnings = errors)
cargo clippy -p extent-manager -- -D warnings

# Build docs
cargo doc -p extent-manager --no-deps
```

## Full CI Gate

```bash
cargo fmt -p extent-manager --check \
  && cargo clippy -p extent-manager -- -D warnings \
  && cargo test -p extent-manager \
  && cargo doc -p extent-manager --no-deps \
  && cargo bench -p extent-manager --no-run
```

## Usage Pattern

```text
1. Create component:    ExtentManagerComponentV1::new_default()
2. Wire receptacles:    Connect IBlockDevice and ILogger
3. Set DMA allocator:   admin.set_dma_alloc(alloc_fn)
4. Initialize or open:  admin.initialize(sizes, slots, ns_id)  — fresh
                        admin.open(ns_id)                       — existing
5. Use IExtentManager:  create_extent, lookup_extent, remove_extent, extent_count
```

## Key Design Decisions

- **One 4KiB block per extent record**: enables atomic per-extent updates without read-modify-write
- **Two-phase write protocol**: record first, bitmap second — crash at any point is recoverable
- **RwLock for thread safety**: read lock for lookups/iteration, write lock for create/remove
- **Iteration holds exclusive access**: blocks create/remove until iteration completes
- **I/O errors propagate immediately**: no retry, no rollback — caller decides policy
