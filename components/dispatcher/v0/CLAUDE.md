# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dispatcher v0 — a dispatcher component for the Certus storage system. Provides the `IDispatcher` interface with receptacles for `ILogger`, `IBlockDeviceAdmin`, and `IDispatchMap`. Built with the component-framework using `define_component!`.

## Build and Test Commands

This crate requires SPDK dependencies (via `IBlockDeviceAdmin`, `IDispatchMap`). It is a workspace member but not a default member.

```bash
cargo build -p dispatcher                         # Build
cargo test -p dispatcher                          # All tests
cargo fmt -p dispatcher --check                   # Check formatting
cargo clippy -p dispatcher -- -D warnings         # Lint (warnings are errors)
cargo doc -p dispatcher --no-deps                 # Build documentation
```

## Architecture

### Component Wiring

```
DispatcherComponentV0 --> [IDispatcher provider]
                      <-- [ILogger receptacle]
                      <-- [IBlockDeviceAdmin receptacle]
                      <-- [IDispatchMap receptacle]
```

**Lifecycle**: `new_default()` → bind receptacles → call `initialize()` → use `IDispatcher` methods → `shutdown()`.

### Key Internal Dependencies

- `component-framework`, `component-core`, `component-macros` — at `../../component-framework/crates/`
- `interfaces` — at `../../interfaces` — where `IDispatcher`, `ILogger`, `IBlockDeviceAdmin`, and `IDispatchMap` are defined

## Active Technologies
- Rust stable, edition 2021, MSRV 1.75 + `component-framework`, `component-core`, `component-macros`, `interfaces` (with `spdk` feature) (001-dispatcher-cache-interface)
- NVMe SSDs via SPDK (block-device-spdk-nvme), extent-manager for space allocation (001-dispatcher-cache-interface)

## Recent Changes
- 001-dispatcher-cache-interface: Added Rust stable, edition 2021, MSRV 1.75 + `component-framework`, `component-core`, `component-macros`, `interfaces` (with `spdk` feature)
