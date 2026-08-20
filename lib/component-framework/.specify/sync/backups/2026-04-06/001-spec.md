# Feature Specification: COM-Style Component Framework

**Feature Branch**: `001-com-component-framework`
**Created**: 2026-03-30
**Status**: Draft
**Input**: User description: "Build a Microsoft-COM like component framework in Rust"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Define an Interface with Macros (Priority: P1)

A library author defines a new interface (e.g., `IStorage`) using a
declarative macro. The macro generates trait definitions and associated
metadata that other crates can reference without depending on the
implementing crate. This allows components to be developed in isolation.

**Why this priority**: Interfaces are the fundamental building block.
Nothing else in the framework can exist without a way to declare and
reference interfaces.

**Independent Test**: Define an interface with the macro, compile a
second crate that depends only on the interface definition, and verify
the trait is usable as a type bound.

**Acceptance Scenarios**:

1. **Given** a Rust crate with the framework's macro dependency,
   **When** the developer invokes the interface-definition macro with
   a name and method signatures,
   **Then** a trait and associated types are generated that compile
   without errors.
2. **Given** a generated interface definition in crate A,
   **When** crate B depends on crate A but not on any implementation,
   **Then** crate B can use the interface as a type parameter and
   compile successfully.
3. **Given** an interface macro invocation,
   **When** the developer omits required fields (e.g., no methods),
   **Then** a clear compile-time error is produced.

---

### User Story 2 - Implement a Component with IUnknown (Priority: P1)

A developer creates a component that implements `IUnknown` and one or
more domain-specific interfaces. The component can be queried at
runtime for the interfaces it provides, its version, and the
receptacles it requires.

**Why this priority**: Components and `IUnknown` are the core runtime
unit. Without them, interfaces have no concrete host.

**Independent Test**: Instantiate a component, call `IUnknown` methods
to query interfaces and version, and verify correct results.

**Acceptance Scenarios**:

1. **Given** a component implementing `IStorage` and `IUnknown`,
   **When** the caller queries `IUnknown` for `IStorage`,
   **Then** a valid reference to the `IStorage` implementation is
   returned.
2. **Given** a component that does not implement `INetwork`,
   **When** the caller queries `IUnknown` for `INetwork`,
   **Then** the query returns an appropriate "not supported" result.
3. **Given** a component with version `1.2.0`,
   **When** the caller queries `IUnknown` for the component version,
   **Then** the version `1.2.0` is returned.
4. **Given** a component with a receptacle for `ILogger`,
   **When** the caller queries `IUnknown` for required interfaces
   (receptacles),
   **Then** `ILogger` appears in the list of receptacles.
5. **Given** a component implementing `IStorage` wrapped in `Arc`,
   **When** the caller uses the `query_interface!` macro to query
   `IStorage`,
   **Then** a valid `Arc<dyn IStorage + Send + Sync>` is returned
   without requiring manual deref or type annotations.

---

### User Story 3 - Connect Receptacles Between Components (Priority: P2)

A system integrator connects a component's receptacle (required
interface) to another component's provided interface. Once connected,
the requiring component can invoke methods on the provided interface.

**Why this priority**: Receptacle wiring enables composition, which is
the primary value proposition of a component framework beyond simple
trait objects.

**Independent Test**: Create two components — one providing `ILogger`
and one requiring `ILogger` — wire the receptacle, and verify the
requiring component can invoke logging methods.

**Acceptance Scenarios**:

1. **Given** component A providing `ILogger` and component B with a
   receptacle for `ILogger`,
   **When** the integrator connects B's `ILogger` receptacle to A,
   **Then** B can call `ILogger` methods that are dispatched to A's
   implementation.
2. **Given** a connected receptacle,
   **When** the integrator disconnects it,
   **Then** subsequent calls through the receptacle return an error
   indicating the receptacle is not connected.
3. **Given** component B with a receptacle for `ILogger`,
   **When** the integrator attempts to connect an `IStorage` provider,
   **Then** a type-mismatch error is produced at compile time.

---

### User Story 4 - Introspect Component Capabilities (Priority: P3)

A diagnostic tool or runtime system enumerates all interfaces and
receptacles of a component at runtime to build wiring diagrams,
validate configurations, or generate documentation.

**Why this priority**: Introspection supports tooling and runtime
validation but is not required for basic component composition.

**Independent Test**: Instantiate a component, enumerate its
interfaces and receptacles via `IUnknown`, and verify the lists match
the component's declaration.

**Acceptance Scenarios**:

1. **Given** a component providing `IStorage` and `INetwork` with a
   receptacle for `ILogger`,
   **When** the tool enumerates provided interfaces via `IUnknown`,
   **Then** the list contains `IStorage`, `INetwork`, and `IUnknown`.
2. **Given** the same component,
   **When** the tool enumerates receptacles via `IUnknown`,
   **Then** the list contains `ILogger`.

---

### Edge Cases

- What happens when a receptacle is invoked before it is connected?
  The framework MUST return a well-defined error (not a panic).
- What happens when the same receptacle is connected twice without
  disconnecting first? The framework MUST return an error; the caller
  MUST disconnect before reconnecting.
- What happens when a component is dropped while receptacles in other
  components still reference it? The framework MUST prevent
  use-after-free at the type-system level or via runtime checks.
- What happens when an interface method signature in the macro
  contains lifetime parameters? The macro MUST support lifetime
  parameters in interface method signatures.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The framework MUST provide a macro (`define_interface!`
  or similar) that generates a trait and associated metadata from a
  declarative specification of method signatures.
- **FR-002**: Interface definitions MUST be usable as dependencies
  without requiring access to any implementation crate.
- **FR-003**: Every component MUST implement `IUnknown`, which provides
  methods to: (a) query for a provided interface by type, (b) retrieve
  the component version, (c) enumerate all provided interfaces,
  (d) enumerate all receptacles.
- **FR-004**: Components MUST support zero or more provided interfaces
  and zero or more receptacles.
- **FR-005**: Receptacles MUST be connectable to a matching provided
  interface on another component and disconnectable at runtime.
- **FR-006**: Type safety MUST be enforced at compile time for
  interface queries and receptacle connections wherever possible.
- **FR-007**: The framework MUST produce compile-time errors when macro
  usage is incorrect (e.g., missing required attributes, type mismatches).
  Errors MAY be standard Rust compiler diagnostics generated from the
  macro's expanded code rather than custom `compile_error!()` messages.
- **FR-010**: The interface-definition macro MUST support lifetime
  parameters in method signatures.
- **FR-008**: All framework code MUST compile and run on the Linux
  operating system using the stable Rust toolchain.
- **FR-009**: Receptacle invocation on an unconnected receptacle MUST
  return an error, not panic.
- **FR-011**: The framework MUST provide a convenience macro
  (`query_interface!`) that wraps the typed `query()` function,
  eliminating the need to spell out `dyn Trait + Send + Sync`. The
  macro MUST work with direct component references, `Arc<T>` wrappers,
  and `ComponentRef` handles.
- **FR-012**: The framework MUST provide a prelude module that
  re-exports the most commonly used types and functions, enabling
  single-import onboarding (`use component_core::prelude::*`).
- **FR-013**: The `define_component!` macro MUST generate a
  `new_default()` constructor for components whose custom fields all
  implement `Default`, providing zero-argument component creation.

### Key Entities

- **Interface**: A named collection of method signatures, defined via
  macro. Serves as a contract between provider and consumer. Uniquely
  identified at runtime by Rust `TypeId`.
- **Component**: A concrete unit that hosts one or more interface
  implementations (provided interfaces) and declares zero or more
  receptacles (required interfaces). Always implements `IUnknown`.
- **Receptacle**: A typed slot on a component representing a required
  interface. Connects to exactly one provider at a time. Can be
  connected and disconnected at runtime; must be disconnected before
  reconnecting to a different provider.
- **IUnknown**: The base interface that every component MUST implement.
  Provides introspection capabilities (interface query, version,
  enumeration of interfaces and receptacles).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer can define a new interface and implement a
  component in under 20 lines of macro-assisted code (excluding
  business logic).
- **SC-002**: Two components can be composed via receptacle wiring
  without either component depending on the other's implementation
  crate.
- **SC-003**: Interface query via `IUnknown` completes in constant
  time relative to the number of interfaces on the component.
- **SC-004**: 100% of public APIs have passing doc tests and unit
  tests.
- **SC-005**: All Criterion benchmarks for interface query, receptacle
  connection, and method dispatch demonstrate no regressions between
  releases.
- **SC-006**: A new contributor can build the project and run the full
  test suite by following crate-level documentation, with no external
  guidance.

## Clarifications

### Session 2026-03-30

- Q: How are interfaces uniquely identified at runtime for query and introspection? → A: Rust `TypeId` (automatic, zero-cost, no manual ID assignment).
- Q: Should framework types be thread-safe (`Send + Sync`) or single-threaded? → A: Thread-safe from the start (`Send + Sync`, `Arc`-based sharing).
- Q: What happens when connecting an already-connected receptacle? → A: Return an error (caller must disconnect first).
- Q: Can a receptacle connect to multiple providers simultaneously? → A: No, single connection per receptacle (exactly one provider at a time).
- Q: Should interface macros support lifetime parameters in method signatures? → A: Yes, support lifetimes from the start.

## Assumptions

- The framework targets Rust library consumers; no CLI, web service,
  or GUI is required.
- Components are composed within a single process; cross-process or
  network communication is out of scope.
- All framework types MUST be `Send + Sync`. Component references
  use `Arc`-based sharing to support multi-threaded composition.
- The interface macro operates at compile time via `macro_rules!` or
  procedural macros; runtime reflection (like COM's `IDispatch`) is
  out of scope.
- Reference counting semantics (like COM's `AddRef`/`Release`) are not
  required; Rust's ownership model is used instead.
