# Public API Contract: Registry, Reference Counting, and Binding

**Feature**: 002-registry-refcount-binding
**Date**: 2026-03-31

## New Public Types (component-core)

### ComponentRef

```text
ComponentRef
  ├── attach() → ComponentRef          // Clone the handle (Arc::clone)
  ├── release(self)                     // Drop the handle (consume)
  ├── Deref → dyn IUnknown             // Transparent IUnknown access
  ├── Clone → ComponentRef              // Same as attach
  └── Send + Sync                       // Thread-safe
```

### ComponentFactory (trait)

```text
trait ComponentFactory: Send + Sync
  └── create(&self, config: Option<&dyn Any>) → Result<ComponentRef, RegistryError>
```

### ComponentRegistry

```text
ComponentRegistry
  ├── new() → Self                                              // Empty registry
  ├── register(&self, name: &str, factory: F) → Result<()>     // Register factory (F: ComponentFactory)
  ├── unregister(&self, name: &str) → Result<()>               // Remove factory
  ├── create(&self, name: &str, config: Option<&dyn Any>) → Result<ComponentRef>  // Create by name
  └── list(&self) → Vec<String>                                 // List registered names
```

### RegistryError (enum)

```text
RegistryError
  ├── NotFound { name: String }
  ├── AlreadyRegistered { name: String }
  ├── FactoryFailed { name: String, source: String }
  └── BindingFailed { detail: String }
```

## Modified Public Types

### IUnknown (extended — component-core)

```text
IUnknown (existing methods unchanged)
  └── NEW: connect_receptacle_raw(&self, receptacle_name: &str, provider: Box<dyn Any + Send + Sync>) → Result<(), RegistryError>
```

The `define_component!` macro generates the implementation by matching receptacle names and downcasting providers.

## New Free Functions (component-core)

### bind

```text
bind(
    provider: &dyn IUnknown,
    interface_name: &str,
    consumer: &dyn IUnknown,
    receptacle_name: &str,
) → Result<(), RegistryError>
```

Third-party binding: resolves interface by name from provider, connects to consumer's named receptacle via `connect_receptacle_raw`.

## Re-exports (component-framework)

All new types and functions re-exported through `component-framework` facade:
- `ComponentRef`
- `ComponentFactory`
- `ComponentRegistry`
- `RegistryError`
- `bind()`

## Backward Compatibility

- All existing types (`IUnknown`, `Receptacle`, `InterfaceMap`, errors, etc.) unchanged
- Existing `define_interface!` macro unchanged
- Existing `define_component!` macro extended with `connect_receptacle_raw` in IUnknown impl
- First-party binding (direct `receptacle.connect(arc)`) continues to work unchanged
- All existing tests continue to pass
