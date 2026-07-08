# Feature Specification: example-helloworld-dylib

**Feature Branch**: `001-example-helloworld-dylib`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice
> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `example-helloworld-dylib` component is a dynamic library (dylib) wrapper around the `example-helloworld` component. It exports a Rust-ABI factory function (`create_component`) that returns a `ComponentRef`, enabling runtime component loading without a C-ABI shim.

Because both the dylib and the host binary dynamically link the same `component-core` and `example-helloworld` shared libraries, `TypeId` values remain consistent across the dylib boundary. This allows the host to use `query_interface` directly on the returned `ComponentRef`.

## User Scenarios & Testing

### User Story 1 - Dynamic Component Loading (Priority: P1)

**As** a Certus host application developer,
**I want** to load a HelloWorld component at runtime from a shared library,
**So that** I can demonstrate and test the dynamic loading capability of the component framework.

**Acceptance Criteria**:
- The dylib exports a `create_component` symbol with `#[no_mangle]` linkage.
- Calling `create_component()` returns a valid `ComponentRef`.
- The returned `ComponentRef` supports `query_interface` for `IGreeter`.
- Both the dylib and host must be compiled with the same `rustc` version.

## Requirements

### Functional Requirements

| ID | Requirement | Priority |
|----|-------------|----------|
| FR-1 | Export a `#[no_mangle]` function named `create_component` | P1 |
| FR-2 | `create_component` returns a `ComponentRef` wrapping a `HelloWorldComponent` | P1 |
| FR-3 | The returned component supports `IGreeter` via `query_interface` | P1 |
| FR-4 | TypeId consistency is maintained by dynamically linking shared dependencies | P1 |

### Non-Functional Requirements

| ID | Requirement | Priority |
|----|-------------|----------|
| NFR-1 | Must be compiled as `crate-type = ["dylib"]` | P1 |
| NFR-2 | Host and dylib must use the same `rustc` version (no stable ABI guarantee) | P1 |
| NFR-3 | No C-ABI shim or FFI boundary required | P2 |

## Key Entities

| Entity | Description |
|--------|-------------|
| `create_component` | Exported factory function returning a `ComponentRef` |
| `ComponentRef` | Arc-based wrapper around `dyn IUnknown` from `component-core` |
| `HelloWorldComponent` | The underlying component implementation from `example-helloworld` |
| `IGreeter` | Interface trait provided by the component |

## Dependencies

| Dependency | Purpose |
|------------|---------|
| `component-core` | Provides `ComponentRef` and `IUnknown` trait infrastructure |
| `example-helloworld` | Provides the `HelloWorldComponent` implementation |

## Success Criteria

- The crate compiles as a `.so` dynamic library.
- A host application can `dlopen` / dynamically link the library and call `create_component`.
- The returned `ComponentRef` can be queried for `IGreeter` and used to call greeting methods.

## Implementation Notes

- The entire implementation is a single 24-line `src/lib.rs` file.
- This serves primarily as a reference example for the dynamic component loading pattern.
- Rust's lack of a stable ABI means this approach requires matching compiler versions between host and plugin.
