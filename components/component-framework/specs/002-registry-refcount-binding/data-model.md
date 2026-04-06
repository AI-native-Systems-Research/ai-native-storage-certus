# Data Model: Registry, Reference Counting, and Binding

**Feature**: 002-registry-refcount-binding
**Date**: 2026-03-31

## Entity: ComponentRef

A reference-counted handle to a component instance. Wraps `Arc<dyn IUnknown>`.

**Fields**:
- `inner: Arc<dyn IUnknown>` — the underlying component reference

**Operations**:
- `attach() -> ComponentRef` — clones the Arc, returning a new handle (maps to `Arc::clone`)
- `release(self)` — consumes the handle, dropping the Arc (maps to `drop`)
- `Deref<Target = dyn IUnknown>` — transparent access to IUnknown methods

**Lifecycle**: Created by factory (initial count = 1). Attach increments count. Release/drop decrements. Component destroyed when count reaches zero.

**Relationships**: Created by ComponentRegistry via factory. Used by assemblers for third-party binding.

## Entity: ComponentRegistry

A standalone catalog mapping string names to component factories.

**Fields**:
- `factories: RwLock<HashMap<String, Box<dyn ComponentFactory>>>` — thread-safe factory storage

**Operations**:
- `new() -> Self` — creates empty registry
- `register(name: &str, factory: impl ComponentFactory) -> Result<(), RegistryError>` — registers factory under unique name
- `unregister(name: &str) -> Result<(), RegistryError>` — removes factory by name
- `create(name: &str, config: Option<&dyn Any>) -> Result<ComponentRef, RegistryError>` — creates component by name with optional config
- `list() -> Vec<String>` — returns all registered component names

**Invariants**:
- Names are unique (register returns error on duplicate)
- All operations are thread-safe
- Factory panics during create are caught and converted to errors

**Relationships**: Contains ComponentFactory instances. Produces ComponentRef handles.

## Entity: ComponentFactory (Trait)

A factory that produces component instances.

**Signature**: `Fn(Option<&dyn Any>) -> Result<ComponentRef, RegistryError>`

Implemented as a trait with a single method to allow both closures and named types.

**Fields**: None (stateless trait). Implementors may capture configuration.

**Relationships**: Registered in ComponentRegistry. Produces ComponentRef.

## Entity: RegistryError

Error type for registry and binding operations.

**Variants**:
- `NotFound { name: String }` — requested component name not registered
- `AlreadyRegistered { name: String }` — name already taken
- `FactoryFailed { name: String, source: String }` — factory returned error or panicked
- `BindingError { detail: String }` — third-party binding failed (type mismatch, receptacle not found, interface not found)

**Derives**: Debug, Clone, PartialEq, Eq, Display, Error

## Entity: Third-Party Binding

Not a struct but a set of free functions and an IUnknown extension.

**New IUnknown method**:
- `connect_receptacle_raw(&self, receptacle_name: &str, provider: Box<dyn Any + Send + Sync>) -> Result<(), BindingError>` — connects a type-erased provider to a named receptacle

**Free function**:
- `bind(provider: &dyn IUnknown, interface_name: &str, consumer: &dyn IUnknown, receptacle_name: &str) -> Result<(), RegistryError>` — resolves interface by name from provider, connects to consumer's named receptacle

**Resolution flow**:
1. Look up `interface_name` in `provider.provided_interfaces()` → get TypeId
2. Call `provider.query_interface_raw(type_id)` → get `&dyn Any`
3. Box/clone the provider Arc from the Any ref
4. Call `consumer.connect_receptacle_raw(receptacle_name, boxed_provider)`
5. Generated match arm in consumer downcasts and calls typed `connect()`

## State Transitions

### ComponentRef Lifecycle
```
Created (count=1) → Attached (count=N) → Released (count=0) → Destroyed
```

### Registry Entry Lifecycle
```
Empty → Registered (factory stored) → Unregistered (factory removed) → Empty
```

### Receptacle (unchanged from feature 001)
```
Disconnected → Connected → Disconnected (via disconnect) → Connected (new provider)
```
