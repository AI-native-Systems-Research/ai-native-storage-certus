# Research: Registry, Reference Counting, and Binding

**Feature**: 002-registry-refcount-binding
**Date**: 2026-03-31

## Decision 1: ComponentRef Design

**Decision**: ComponentRef is a newtype wrapper around `Arc<dyn IUnknown>`.

**Rationale**: The existing framework already constructs components as `Arc<Self>` and the InterfaceMap stores `Arc<dyn IFoo>` entries. ComponentRef wraps the IUnknown Arc to provide the COM-style vocabulary (attach/release) while leveraging Rust's ownership and drop semantics for safety. `attach()` calls `Arc::clone`, `release()` calls `drop`. The compiler prevents use-after-free.

**Alternatives considered**:
- Manual reference counting with raw pointers: Rejected — replicates what Arc already does, introduces unsafe code without benefit.
- Hybrid (Arc + explicit release with drop-panic): Rejected — surprising semantics, not idiomatic Rust.

## Decision 2: Registry Thread Safety

**Decision**: Use `RwLock<HashMap<String, Box<dyn ComponentFactory>>>` for the registry's internal state.

**Rationale**: RwLock allows concurrent reads (listing, creating) with exclusive writes (register, unregister). This matches the expected access pattern where creation far outnumbers registration. The same pattern is already used by Receptacle (`RwLock<Option<Arc<T>>>`).

**Alternatives considered**:
- Mutex: Simpler but serializes all access including concurrent reads. Rejected for unnecessary contention.
- DashMap: Lock-free concurrent map. Rejected — adds external dependency for minimal benefit at this scale.

## Decision 3: Factory Signature

**Decision**: `Fn(Option<&dyn Any>) -> Result<ComponentRef, RegistryError>` where the `Option<&dyn Any>` carries optional typed configuration.

**Rationale**: Type-erased config (dyn Any) allows each factory to downcast to its expected config type, keeping the factory trait signature uniform. `Option` allows factories that need no config. Returning `Result` handles factory construction failures cleanly.

**Alternatives considered**:
- Zero-argument factory: Rejected — clarification confirmed typed config is needed.
- Key-value string map: Rejected — loses type safety, requires parsing.

## Decision 4: Third-Party Binding Mechanism

**Decision**: A `bind()` free function that accepts two `&dyn IUnknown` references plus string names for the interface and receptacle. It resolves names via `provided_interfaces()` and `receptacles()` metadata, matches by `TypeId`, and performs the connection.

**Rationale**: String-based matching at the API surface with TypeId verification internally gives the best of both worlds — assemblers don't need compile-time type knowledge, but type safety is enforced at runtime. The existing `InterfaceInfo` and `ReceptacleInfo` already carry both `name` and `type_id` fields.

**Key challenge**: The current receptacle `connect()` method requires `Arc<T>` where T is the specific trait object type. Third-party binding must query the provider's interface by TypeId (returns `&dyn Any`), then connect it to the receptacle. This requires a new `connect_raw()` method on IUnknown that accepts a receptacle name and a type-erased `&dyn Any` provider.

**Alternatives considered**:
- TypeId-only matching: Rejected — assemblers would need access to TypeId values.
- Dual mode (string + TypeId): Overcomplicated — string matching with internal TypeId verification covers all cases.

## Decision 5: ComponentRef and IUnknown Access

**Decision**: ComponentRef provides `Deref` to `dyn IUnknown`, allowing direct method calls. For typed interface access, callers use the existing `query::<dyn IFoo + Send + Sync>()` free function.

**Rationale**: This integrates naturally with the existing query pattern. ComponentRef is the owning handle; IUnknown is the discovery mechanism; `query()` provides typed access.

## Decision 6: Third-Party Binding Implementation — connect_raw on IUnknown

**Decision**: Add a `connect_receptacle_raw(&self, receptacle_name: &str, provider: Box<dyn Any + Send + Sync>) -> Result<(), BindingError>` method to IUnknown. The generated IUnknown impl matches the receptacle name, downcasts the provider to the expected `Arc<dyn IFoo + Send + Sync>` type, and calls the typed `connect()`.

**Rationale**: This keeps third-party binding type-safe at runtime while operating through string names. The macro generates match arms for each receptacle, so no trait-object-unsafe generics are needed.

**Alternatives considered**:
- Adding a trait method to Receptacle that accepts `dyn Any`: Would require Receptacle to know its own TypeId at runtime, adding complexity.
- Separate "binding engine" struct: Unnecessary indirection — IUnknown already has the metadata and the macro generates the struct.
