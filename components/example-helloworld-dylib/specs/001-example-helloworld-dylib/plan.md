# Implementation Plan: example-helloworld-dylib

**Branch**: `001-example-helloworld-dylib` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation.

## Summary

This component wraps the `example-helloworld` component in a Rust dylib, exporting a single factory function that returns a `ComponentRef`. It demonstrates dynamic component loading without C-ABI overhead; `TypeId` consistency across the dylib boundary comes from compiling both sides against the same crate source with the same `rustc` version, not from shared dynamic linking of `component-core`.

## Technical Context

- **Crate type**: `dylib` -- produces a platform-native shared library (`.so` on Linux).
- **ABI strategy**: Rust-ABI (not `cdylib`). Neither `component-core` nor `example-helloworld` declares `crate-type = ["dylib"]`, so both the host and the plugin statically embed their own `rlib` copies of these crates rather than dynamically linking a shared `.so`. `TypeId` values stay consistent because `TypeId` equality is derived from compile-time type identity (same source, same `rustc` version), not from runtime linkage.
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
  +-- statically embeds its own rlib copies of: component-core, example-helloworld
      (Host Binary embeds its own separate rlib copies of the same crates;
       no component-core.so / example-helloworld.so is actually shared at runtime)
```

### Key Design Decisions

1. **dylib not cdylib**: Using Rust-ABI (`dylib`) avoids the need for C-compatible types at the boundary. The trade-off is requiring the same compiler version. Note: `component-core` and `example-helloworld` are ordinary `rlib` crates (no `crate-type = ["dylib"]`), so each side of the boundary statically embeds its own copy of them; `TypeId` consistency relies on compile-time type identity (same source, same `rustc` version) rather than on dynamically sharing a `.so` for these dependencies.
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
