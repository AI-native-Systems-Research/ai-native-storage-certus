# Implementation Plan: example-helloworld-dylib

**Branch**: `001-example-helloworld-dylib` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.

## Summary

This component wraps the `example-helloworld` component in a Rust dylib, exporting a single factory function that returns a `ComponentRef`. It demonstrates dynamic component loading without C-ABI overhead by relying on shared dynamic linking of `component-core`.

## Technical Context

- **Crate type**: `dylib` -- produces a platform-native shared library (`.so` on Linux).
- **ABI strategy**: Rust-ABI (not `cdylib`). Both host and plugin link against the same `component-core` and `example-helloworld` dylibs, keeping `TypeId` values consistent.
- **Factory pattern**: A single `#[no_mangle] pub fn create_component() -> ComponentRef` entry point.

## Architecture

```
Host Binary
  |
  |-- dlopen / dynamic link
  v
example-helloworld-dylib.so
  |-- create_component() -> ComponentRef
  |       |
  |       v
  |   HelloWorldComponent (from example-helloworld)
  |       implements IGreeter + IUnknown
  |
  +-- shared deps: component-core.so, example-helloworld.so
```

### Key Design Decisions

1. **dylib not cdylib**: Using Rust-ABI (`dylib`) avoids the need for C-compatible types at the boundary. The trade-off is requiring the same compiler version.
2. **Minimal wrapper**: The dylib contains only the factory; all logic lives in `example-helloworld`.

## Dependencies

| Crate | Role |
|-------|------|
| `component-core` (workspace) | `ComponentRef`, `IUnknown` |
| `example-helloworld` (workspace) | `HelloWorldComponent` implementation |

## Testing

- **Integration test**: A host binary loads the dylib, calls `create_component`, queries for `IGreeter`, and invokes a greeting method.
- **Compile-time**: The crate compiles successfully as a dylib (verified by `cargo build`).

## Future Considerations

- If Rust stabilizes an ABI (e.g., via `crabi`), the `dylib` approach could become version-independent.
- A `cdylib` variant with a thin C-ABI shim could support cross-compiler-version loading.
- Plugin discovery/registry mechanisms could be built atop this factory pattern.
