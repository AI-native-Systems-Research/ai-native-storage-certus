# example-helloworld-dylib

**Crate**: `example-helloworld-dylib`
**Path**: `components/example-helloworld-dylib/`
**Version**: 0.1.0
**Crate type**: `dylib`

## Description

Dynamic-library factory wrapper around `HelloWorldComponent`. Exports a `#[no_mangle]` factory function `create_component() -> ComponentRef` that instantiates the component and returns it as a type-erased `ComponentRef` (`Arc<dyn IUnknown>`). Demonstrates the project's dynamic-library component loading pattern.

Uses Rust-ABI `dylib` (not `cdylib`) — both host and plugin must dynamically link the same `component-core` and `example-helloworld` shared libraries so that `TypeId` values match across the dlopen boundary. Requires same `rustc` version on both sides.

## Interfaces Provided

Via the underlying `HelloWorldComponent`:
- `IGreeter` — `greeting_prefix(&self) -> &str` (returns `"Hello"`)
- `IUnknown` — runtime interface discovery

## Receptacles

Via the underlying `HelloWorldComponent`:

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `logger` | `ILogger` | No | Optional logging of greeting events |
