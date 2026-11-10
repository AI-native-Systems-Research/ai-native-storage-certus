# Research: COM-Style Component Framework

**Date**: 2026-03-30
**Feature**: 001-com-component-framework

## R1: Macro Strategy — `macro_rules!` vs Procedural Macros

**Decision**: Procedural macros (`proc-macro` crate)

**Rationale**: The `define_interface!` and `define_component!` macros
must parse arbitrary Rust method signatures including lifetime
parameters, generic bounds, and return types. Procedural macros via
`syn` provide a full Rust parser, precise `Span`-based error messages,
and the ability to generate complex derived code (trait impls, metadata
structs, `TypeId` registration). `macro_rules!` lacks the parsing
fidelity needed for lifetime-aware method signatures and produces
opaque error messages on malformed input.

**Alternatives considered**:
- `macro_rules!`: Simpler, no extra crate, but cannot robustly parse
  method signatures with lifetimes; poor error diagnostics.
- Build script codegen: Over-engineered for this use case; not
  ergonomic (generated files, no IDE macro expansion).

## R2: Interface Identity — TypeId-based Lookup

**Decision**: Use `std::any::TypeId` keyed on the trait object type
`dyn IFoo + Send + Sync` for interface identification.

**Rationale**: `TypeId` is zero-cost (compiler-assigned, no runtime
overhead), requires no manual GUID/UUID assignment, and leverages
Rust's type system directly. Since all interface traits require
`Send + Sync + 'static` (enforced by the macro), `TypeId::of::<dyn
IFoo + Send + Sync>()` is valid for `?Sized` types.

**Lookup mechanism**: Each component stores a
`HashMap<TypeId, Box<dyn Any + Send + Sync>>` populated at
construction time. Each entry boxes an `Arc<dyn IFoo + Send + Sync>`.
On query, the caller provides a `TypeId`, looks up the entry, and
downcasts the `Box<dyn Any>` to `&Arc<dyn IFoo + Send + Sync>` via
`downcast_ref`. This works because `Arc<dyn IFoo + Send + Sync>` is
a `Sized` concrete type (two-word fat pointer) that implements `Any`.

**Performance**: `HashMap` lookup is O(1) amortized. For components
with ≤20 interfaces, a `Vec<(TypeId, Box<dyn Any + Send + Sync>)>`
with linear scan may be faster due to cache locality. The initial
implementation will use `HashMap`; benchmarks will determine if a
`SmallVec`-based approach is warranted.

**Alternatives considered**:
- UUID/GUID: COM-faithful but requires manual assignment and runtime
  comparison of 128-bit values.
- String names: Human-readable but runtime string comparison cost;
  fragile to typos.
- Marker structs: Works but adds boilerplate per interface; `TypeId`
  of `dyn Trait` is more direct.

## R3: Thread Safety — `Send + Sync` with `Arc`

**Decision**: All framework types are `Send + Sync`. Components are
shared via `Arc`. Interior mutability (where needed) uses
`RwLock` or `Mutex`.

**Rationale**: The clarification session mandated thread safety from
the start. `Arc` is the natural Rust smart pointer for shared
ownership across threads. `RwLock` on receptacle connections allows
concurrent reads (method dispatch) with exclusive writes
(connect/disconnect).

**Alternatives considered**:
- `Rc` + `RefCell` (single-threaded): Simpler, less overhead, but
  spec requires `Send + Sync`.
- Dual API (thread-safe + single-threaded): Doubles API surface and
  maintenance burden for unclear benefit.

## R4: Receptacle Design

**Decision**: `Receptacle<T>` wraps `RwLock<Option<Arc<dyn T + Send +
Sync>>>`. Connection state is explicit via `Option`. Each receptacle
connects to exactly one provider.

**Rationale**: `RwLock` allows concurrent method dispatch (read lock)
while exclusive connect/disconnect (write lock). `Option::None`
represents the disconnected state; invoking a disconnected receptacle
returns `Err(ReceptacleError::NotConnected)`.

**Connect semantics**: `connect()` fails with
`Err(ReceptacleError::AlreadyConnected)` if the receptacle already
holds a connection. Caller must `disconnect()` first. This was
decided in the clarification session.

**Lifetime safety**: When a provider component is dropped, its `Arc`
ref count decrements. Receptacles holding an `Arc` to the provider
keep it alive. No use-after-free is possible because `Arc` prevents
deallocation while references exist.

**Alternatives considered**:
- `Mutex<Option<...>>`: Simpler but blocks all readers during dispatch.
- `AtomicPtr` + unsafe: Lower overhead but requires careful unsafe
  code, violating the constitution's minimize-unsafe principle.
- Multi-connection receptacles: Deferred; single connection per spec.

## R5: `IUnknown` Trait Design

**Decision**: `IUnknown` is a trait with four methods:

1. `query_interface<I>(&self) -> Option<Arc<dyn I + Send + Sync>>`
   (typed wrapper, not object-safe)
2. `version(&self) -> &str`
3. `provided_interfaces(&self) -> &[InterfaceInfo]`
4. `receptacles(&self) -> &[ReceptacleInfo]`

Internally, a non-generic `query_interface_raw(&self, id: TypeId) ->
Option<&(dyn Any + Send + Sync)>` enables the object-safe path. The
`define_component!` macro generates both.

**Rationale**: Rust trait objects cannot have generic methods, so
`query_interface<I>` cannot be called through `dyn IUnknown`. The
raw method provides object-safe dispatch; the generic wrapper provides
ergonomic typed access on concrete component types. A free function
`query::<I>(component: &dyn IUnknown) -> Option<Arc<dyn I + Send +
Sync>>` bridges the gap for trait-object callers.

**InterfaceInfo / ReceptacleInfo**: Lightweight structs containing
`TypeId`, `&'static str` name (for diagnostics/introspection), and
in the case of `ReceptacleInfo`, a `&'static str` receptacle name.

**Alternatives considered**:
- Single non-generic method only: Less ergonomic; caller must manually
  downcast.
- Associated-type based: Limits to one interface per query call
  pattern; not flexible.

## R6: Procedural Macro Error Handling

**Decision**: Use `syn::Error` with precise `Span` locations for all
macro validation errors. Emit errors via `compile_error!` at the
call site.

**Rationale**: Constitution Principle I requires clear compile-time
errors. `syn::Error` preserves source location so IDE/compiler output
points directly to the malformed macro input. Errors covered:
missing method signatures, invalid attribute combinations, type
constraint violations.

**Alternatives considered**:
- `panic!` in proc macro: Produces poor error messages without source
  location.
- Silent defaults: Violates principle of explicit, readable code.

## R7: Crate Organization

**Decision**: Three-crate workspace: `component-core` (types/traits),
`component-macros` (proc macros), `component-framework` (facade).

**Rationale**: Cargo requires proc-macro crates to be standalone (no
mixing with regular library code). The core crate holds all runtime
types so interface-definition crates can depend on it without pulling
in the macro crate. The facade crate re-exports both for convenience.

**Dependency graph**:
```
component-framework  →  component-core
                     →  component-macros  →  component-core (for type references in generated code)

User interface crate →  component-core (traits only, no macros needed for consumers)
User component crate →  component-framework (gets both macros and types)
```

**Alternatives considered**:
- Single crate: Impossible due to Cargo proc-macro restriction.
- Two crates (macros + everything else): Works but forces interface
  consumers to depend on the macro crate even when they only need
  traits.
