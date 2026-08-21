# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dispatcher v1 — a dispatcher component for the Certus storage system with DRAM memory-tier caching. Provides the `IDispatcher` interface for GPU-to-SSD cache operations with LRU-managed DRAM pool. Built with the component-framework using `define_component!`.

## Build and Test Commands

This crate requires SPDK dependencies. It is a workspace member but not a default member.

```bash
cargo build -p dispatcher-v1                         # Build
cargo test -p dispatcher-v1                          # Unit + lazy migration tests
cargo test -p dispatcher-v1 --features hardware-test --test integration -- --test-threads=1  # Hardware tests
cargo fmt -p dispatcher-v1 --check                   # Check formatting
cargo clippy -p dispatcher-v1 -- -D warnings         # Lint (warnings are errors)
cargo doc -p dispatcher-v1 --no-deps                 # Build documentation
```

## Architecture

### Component Wiring

```
DispatcherComponent --> [IDispatcher provider]
                      <-- [ILogger receptacle]
                      <-- [IDispatchMap receptacle]
                      <-- [IGpuServices receptacle]
                      <-- [ISPDKEnv receptacle]
```

**Lifecycle**: `new_default()` → bind receptacles → call `initialize(config)` → use `IDispatcher` methods → `shutdown()`.

Block devices and extent managers are created internally during `initialize()` based on the `DispatcherConfig` PCI addresses. If the ISPDKEnv receptacle is not connected, operates in staging-only mode (for unit testing without hardware).

### Key Internal Dependencies

- `component-framework`, `component-core`, `component-macros` — at `../../lib/component-framework/crates/`
- `interfaces` — at `../../interfaces` — where `IDispatcher`, `ILogger`, `IDispatchMap`, `IGpuServices` are defined
- `spdk-env` — at `../../spdk-env` — provides `ISPDKEnv` trait
- `block-device-spdk-nvme` — NVMe block device driver
- `extent-manager` — fixed-size extent allocator

### Internal Modules

- `io_segmenter` — MDTS-aware I/O splitting (128 KiB default)
- `background` — Async staging-to-SSD write worker thread

## Active Technologies
- Rust stable, edition 2021, MSRV 1.75 + `component-framework`, `component-core`, `component-macros`, `interfaces` (with `spdk` feature)
- NVMe SSDs via SPDK (block-device-spdk-nvme), extent-manager for space allocation
- GPU DMA via IGpuServices (dma_copy_to_host for populate, dma_copy_to_device for lookup)
