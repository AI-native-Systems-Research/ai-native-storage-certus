# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

GPU Services is a component (v0) in the Certus system that exposes the `IGpuServices` interface. It is currently a skeleton — `initialize()` and `shutdown()` lifecycle methods with an optional `ILogger` receptacle. This crate is a workspace default-member, so plain `cargo build`/`cargo test` includes it.

## Build & Test

```bash
cargo build -p gpu-services
cargo test -p gpu-services
cargo clippy -p gpu-services -- -D warnings
cargo doc -p gpu-services --no-deps
```

## Architecture

- **Interface**: `IGpuServices` (defined in `components/interfaces/src/igpu_services.rs`) — `initialize()` and `shutdown()`.
- **Component**: `GpuServicesComponentV0` declared via `define_component!`. Provides `IGpuServices`, has one receptacle (`logger: ILogger`).
- **Logger receptacle**: Optional. Operations succeed silently when no logger is connected. Connect via `component.logger.connect(arc_logger)`.

## Patterns

- Use `query_interface!(component, IGpuServices)` to obtain the interface from an instantiated component.
- Receptacle access is fallible (`self.logger.get()` returns `Result`); always handle the unconnected case gracefully.
- Follow existing test patterns: test interface availability, test operations without receptacles connected, test operations with receptacles connected.

## Active Technologies
- Rust stable, edition 2021, MSRV 1.75 + CUDA runtime API (libcudart via FFI), (001-gpu-cuda-services)
- N/A (operates on GPU device memory) (001-gpu-cuda-services)

## Recent Changes
- 001-gpu-cuda-services: Added Rust stable, edition 2021, MSRV 1.75 + CUDA runtime API (libcudart via FFI),
