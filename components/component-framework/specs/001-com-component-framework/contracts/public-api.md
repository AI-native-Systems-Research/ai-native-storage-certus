# Public API Contract: component-framework

**Date**: 2026-03-30
**Feature**: 001-com-component-framework

## Crate: `component-core`

### Traits

```rust
/// Marker trait for all framework interfaces.
/// All user-defined interfaces extend this implicitly via the macro.
pub trait Interface: Send + Sync + 'static {}

/// Base interface for all components. Provides introspection.
pub trait IUnknown: Send + Sync {
    /// Look up an interface by TypeId. Returns a type-erased reference
    /// that the caller downcasts to `&Arc<dyn IFoo + Send + Sync>`.
    fn query_interface_raw(&self, id: TypeId) -> Option<&(dyn Any + Send + Sync)>;

    /// Component version string (e.g., "1.2.0").
    fn version(&self) -> &str;

    /// List of all interfaces this component provides.
    fn provided_interfaces(&self) -> &[InterfaceInfo];

    /// List of all receptacles (required interfaces) on this component.
    fn receptacles(&self) -> &[ReceptacleInfo];
}
```

### Free Function

```rust
/// Typed wrapper for query_interface_raw.
/// Looks up interface I on the given IUnknown implementor.
///
/// # Example
/// ```
/// let storage: Arc<dyn IStorage + Send + Sync> =
///     query::<dyn IStorage + Send + Sync>(&*component)?;
/// ```
pub fn query<I: Send + Sync + 'static + ?Sized>(
    component: &dyn IUnknown,
) -> Option<Arc<I>>
where
    Arc<I>: Any + Send + Sync;
```

### Structs

```rust
/// Metadata about a provided interface.
#[derive(Debug, Clone)]
pub struct InterfaceInfo {
    pub type_id: TypeId,
    pub name: &'static str,
}

/// Metadata about a required interface (receptacle).
#[derive(Debug, Clone)]
pub struct ReceptacleInfo {
    pub type_id: TypeId,
    pub name: &'static str,
    pub interface_name: &'static str,
}

/// A typed slot for a required interface. Thread-safe.
pub struct Receptacle<T: ?Sized + Send + Sync + 'static> {
    // internal: RwLock<Option<Arc<T>>>
}

impl<T: ?Sized + Send + Sync + 'static> Receptacle<T> {
    /// Create a new disconnected receptacle.
    pub fn new() -> Self;

    /// Connect a provider. Fails if already connected.
    pub fn connect(&self, provider: Arc<T>) -> Result<(), ReceptacleError>;

    /// Disconnect the current provider. Fails if not connected.
    pub fn disconnect(&self) -> Result<(), ReceptacleError>;

    /// Returns true if a provider is currently connected.
    pub fn is_connected(&self) -> bool;

    /// Access the connected provider. Fails if not connected.
    pub fn get(&self) -> Result<Arc<T>, ReceptacleError>;
}
```

### Error Types

```rust
/// Errors from receptacle operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceptacleError {
    /// Attempted to invoke or disconnect a receptacle with no connection.
    NotConnected,
    /// Attempted to connect a receptacle that already has a connection.
    AlreadyConnected,
}

/// Errors from interface queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// The requested interface is not provided by this component.
    InterfaceNotFound,
}
```

## Crate: `component-macros`

### Procedural Macros

```rust
/// Define a new interface.
///
/// Generates:
/// - A trait with `Send + Sync + 'static` bounds
/// - An `Interface` impl
/// - `InterfaceInfo` metadata
///
/// # Usage
/// ```
/// define_interface! {
///     IStorage {
///         fn read(&self, key: &str) -> Result<Vec<u8>, StorageError>;
///         fn write(&self, key: &str, value: &[u8]) -> Result<(), StorageError>;
///     }
/// }
/// ```
pub macro define_interface { ... }

/// Define a component implementing one or more interfaces.
///
/// Generates:
/// - `IUnknown` implementation
/// - InterfaceMap population
/// - Receptacle field declarations
/// - ReceptacleInfo metadata
///
/// # Usage
/// ```
/// define_component! {
///     MyStorageComponent {
///         version: "1.0.0",
///         provides: [IStorage, ISerializable],
///         receptacles: {
///             logger: ILogger,
///         },
///         fields: {
///             data: HashMap<String, Vec<u8>>,
///         },
///     }
/// }
/// ```
pub macro define_component { ... }
```

## Crate: `component-framework`

Facade crate. Re-exports all public items from `component-core` and
`component-macros`:

```rust
pub use component_core::*;
pub use component_macros::*;
```

## Behavioral Contracts

| Operation | Precondition | Postcondition | Error |
|-----------|-------------|---------------|-------|
| `query_interface_raw(id)` | Component constructed | Returns `Some` if interface provided, `None` otherwise | — |
| `query::<I>(comp)` | Component constructed | Returns `Some(Arc<dyn I>)` if provided | — |
| `Receptacle::connect(p)` | Disconnected | Connected to `p` | `AlreadyConnected` |
| `Receptacle::disconnect()` | Connected | Disconnected, drops `Arc` ref | `NotConnected` |
| `Receptacle::get()` | Connected | Returns `Arc` clone of provider | `NotConnected` |
| `provided_interfaces()` | Component constructed | Returns complete, ordered list | — |
| `receptacles()` | Component constructed | Returns complete, ordered list | — |
| `version()` | Component constructed | Returns version string | — |
