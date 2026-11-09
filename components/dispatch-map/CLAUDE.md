# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

DispatchMap v0 — a dispatch map component for the Certus storage system. Provides the `IDispatchMap` interface with receptacles for `ILogger` and `IExtentManager`. Built with the component-framework using `define_component!`.

## Build and Test Commands

This crate requires SPDK dependencies (via `IExtentManager`). It is a workspace member but not a default member.

```bash
cargo build -p dispatch-map                       # Build
cargo test -p dispatch-map                        # All tests
cargo fmt -p dispatch-map --check                 # Check formatting
cargo clippy -p dispatch-map -- -D warnings       # Lint (warnings are errors)
cargo doc -p dispatch-map --no-deps               # Build documentation
```

## Architecture

### Component Wiring

```
DispatchMapComponent --> [IDispatchMap provider]
                       <-- [ILogger receptacle]
                       <-- [IExtentManager receptacle]
```

**Lifecycle**: `new_default()` → bind receptacles → use `IDispatchMap` methods.

### Key Internal Dependencies

- `component-framework`, `component-core`, `component-macros` — at `../../component-framework/crates/`
- `interfaces` — at `../../interfaces` — where `IDispatchMap`, `ILogger`, and `IExtentManager` are defined

## Active Technologies
- Rust stable, edition 2021, MSRV 1.75 + component-framework, component-core, component-macros, interfaces (with `spdk` feature) (001-dispatch-map)
- In-memory `HashMap`; persistence delegated to `IExtentManager` (001-dispatch-map)

## Recent Changes
- 001-dispatch-map: Added Rust stable, edition 2021, MSRV 1.75 + component-framework, component-core, component-macros, interfaces (with `spdk` feature)
