# Feature Specification: Registry, Reference Counting, and Binding

**Feature Branch**: `002-registry-refcount-binding`
**Created**: 2026-03-31
**Status**: Draft
**Input**: User description: "The framework must support first-party and third-party binding. Atomic reference counting with explicit attach/release should be used for safe destruction of components. Implement a component registry that uses a factory pattern to instantiate components."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Component Registry with Factory Instantiation (Priority: P1)

A framework integrator registers component factories in a central registry, then creates component instances on demand by name. The registry serves as the single entry point for discovering and creating components — both first-party (compiled into the same binary) and third-party (registered at runtime by external code). The integrator looks up a component by its registered name, the registry invokes the corresponding factory, and returns a new component instance.

**Why this priority**: The registry is the foundation for decoupled component creation. Without it, components must be instantiated directly by type, defeating the purpose of a pluggable framework. All other features (binding, lifecycle management) depend on having a way to discover and create components.

**Independent Test**: Register a factory for a component, request an instance by name from the registry, and verify the returned component is functional and implements the expected interfaces.

**Acceptance Scenarios**:

1. **Given** a registry with a factory registered under the name "StorageEngine", **When** the integrator requests a component by that name, **Then** the registry returns a new, fully initialized component instance that supports the expected interfaces.
2. **Given** an empty registry, **When** the integrator requests a component by an unregistered name, **Then** the registry returns a clear "not found" error.
3. **Given** a registry with multiple factories registered, **When** the integrator lists all registered component names, **Then** the registry returns the complete set of registered names.
4. **Given** a factory already registered under a name, **When** the integrator attempts to register another factory under the same name, **Then** the registry returns an error indicating the name is already taken.
5. **Given** a simple component that needs no configuration, **When** the integrator registers it using the simplified method (`register_simple`) with just a name and factory closure, **Then** the component can be created by name from the registry.

---

### User Story 2 - Atomic Reference Counting with Attach/Release (Priority: P1)

A developer uses explicit attach and release operations to manage the lifetime of a component instance. Each attach increments an atomic reference count; each release decrements it. When the count reaches zero, the component is destroyed. This provides deterministic, safe destruction semantics that work across thread boundaries and allow multiple owners to share a component without data races on the lifetime.

**Why this priority**: Without explicit lifetime management, components created by the registry would either leak or be prematurely destroyed. Reference counting is the fundamental mechanism that makes component sharing safe and deterministic.

**Independent Test**: Create a component, attach multiple references, release them one by one, and verify the component is destroyed exactly when the last reference is released.

**Acceptance Scenarios**:

1. **Given** a newly created component with a reference count of 1, **When** the holder calls attach, **Then** the reference count increments to 2 and both holders can use the component.
2. **Given** a component with a reference count of 2, **When** one holder calls release, **Then** the reference count decrements to 1 and the remaining holder can still use the component.
3. **Given** a component with a reference count of 1, **When** the holder calls release, **Then** the reference count reaches zero and the component is destroyed (drop semantics triggered).
4. **Given** a component shared across multiple threads, **When** threads concurrently call attach and release, **Then** the reference count remains correct and the component is destroyed exactly once when the final release occurs.
5. **Given** a ComponentRef that has been released (dropped), **When** code attempts to use it, **Then** the compiler rejects the access (ownership-based use-after-free prevention).

---

### User Story 3 - First-Party and Third-Party Binding (Priority: P2)

A framework user wires components together using two modes of binding. In first-party binding, the integrator directly connects a component's receptacle to another component's provided interface using explicit code at assembly time. In third-party binding, an external assembler or configuration-driven system wires components together without the components themselves knowing the identity of their partners. Both binding modes use the same receptacle/interface mechanism, but differ in who performs the wiring.

**Why this priority**: Binding modes determine how flexible the framework is for real-world use. First-party binding is simpler and covers most use cases; third-party binding enables plugin architectures, dependency injection, and declarative assembly. This builds on the existing receptacle system from the first feature set.

**Independent Test**: Wire two components using first-party binding (direct code), then wire two components using third-party binding (assembler function that takes component references and connects them by interface name), and verify both produce the same functional result.

**Acceptance Scenarios**:

1. **Given** two instantiated components (one providing ILogger, one requiring ILogger), **When** the integrator directly connects them using first-party binding, **Then** the requiring component can call ILogger methods successfully.
2. **Given** two instantiated components, **When** a third-party assembler wires them together using only interface names and component references (without compile-time knowledge of concrete types), **Then** the requiring component can call the provided interface methods successfully.
3. **Given** a component with multiple receptacles, **When** a third-party assembler wires each receptacle to a different provider, **Then** all receptacles are connected and functional.
4. **Given** a third-party assembler attempting to wire an incompatible interface (wrong type), **When** the binding is attempted, **Then** the system returns a type mismatch error.

---

### User Story 4 - Registry-Driven Assembly (Priority: P3)

A system builder uses the component registry to create components by name and then uses third-party binding to wire them together, forming a complete application from a declarative description. This is the full pipeline: registry lookup, factory creation, reference counting for lifetime, and third-party binding for wiring.

**Why this priority**: This is the integration story that ties all three capabilities together. It represents the end-to-end experience of building a component-based application, but each individual capability (US1, US2, US3) must work independently first.

**Independent Test**: Register multiple component factories, create instances by name, wire them together using third-party binding, invoke operations that cross component boundaries, and verify correct behavior end-to-end.

**Acceptance Scenarios**:

1. **Given** a registry with factories for "Logger" and "AppService", **When** the builder creates both by name, wires AppService's logger receptacle to the Logger component, and calls an AppService method that logs, **Then** the log message is produced by the Logger component.
2. **Given** multiple components created from the registry, **When** the builder releases all references, **Then** each component is destroyed in a safe order (no dangling references).

---

### Edge Cases

- What happens when a factory panics during component construction? The registry must not leave corrupted state; the error is propagated to the caller.
- What happens when all explicit ComponentRef handles are released while a receptacle still points to a component's interface? Since receptacles hold Arc references, the component stays alive as long as any receptacle is connected. The component is only destroyed when all references (both explicit handles and receptacle connections) are dropped.
- What happens when the registry is accessed concurrently from multiple threads? All registry operations (register, unregister, create) must be thread-safe.
- What happens when a third-party assembler attempts to bind a receptacle that is already connected? The existing "already connected" error from the receptacle system applies.
- What happens when attach is called on a component reference after the component has been destroyed? Since ComponentRef wraps Arc and receptacles also hold Arc, this situation cannot arise — Rust's ownership system prevents access to dropped values at compile time.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST provide a component registry that maps string names to component factories.
- **FR-002**: The registry MUST allow registering a factory with a unique name.
- **FR-003**: The registry MUST allow creating a component instance by looking up a registered name and invoking the corresponding factory, optionally passing a type-erased configuration parameter.
- **FR-004**: The registry MUST return an error when a requested component name is not registered.
- **FR-005**: The registry MUST return an error when attempting to register a duplicate name.
- **FR-006**: The registry MUST allow listing all registered component names.
- **FR-007**: The registry MUST allow unregistering a factory by name.
- **FR-008**: All registry operations MUST be thread-safe (safe to call concurrently from multiple threads).
- **FR-009**: Components MUST use atomic reference counting for lifetime management.
- **FR-010**: The system MUST provide an explicit attach operation that increments the reference count and returns a new handle.
- **FR-011**: The system MUST use Rust's `Drop` semantics to release references — when a `ComponentRef` is dropped, the reference count decrements and the component is destroyed when it reaches zero.
- **FR-012**: Reference counting MUST be safe across thread boundaries (atomic operations).
- **FR-013**: The system MUST prevent use-after-free at compile time — ComponentRef wraps Arc, so once released (dropped), no further access is possible through Rust's ownership rules.
- **FR-014**: The system MUST support first-party binding where the integrator directly connects a receptacle to a provided interface using explicit code.
- **FR-015**: The system MUST support third-party binding where an assembler connects components using string-based interface and receptacle names, without compile-time knowledge of concrete types.
- **FR-016**: Third-party binding MUST be able to enumerate a component's receptacles and provided interfaces (by name) to perform wiring.
- **FR-017**: Third-party binding MUST resolve string names to TypeId internally and produce a clear error when the resolved types are not compatible.
- **FR-019**: Third-party binding MUST accept a provider component reference, a provider interface name, a consumer component reference, and a consumer receptacle name to perform a single wiring operation.
- **FR-018**: Factory functions MUST return components wrapped in a single
  `ComponentRef`. The caller holds exactly one external reference. Internal
  reference count may be higher due to implementation details (e.g., the
  interface map holds Arc clones for query support). The component MUST be
  destroyed when all external `ComponentRef` handles and receptacle
  connections are dropped.
- **FR-020**: The registry MUST provide a simplified factory registration method (`register_simple`) that accepts a closure returning a `ComponentRef` without requiring a configuration parameter. This is syntactic sugar over FR-003 for components that need no configuration.

### Key Entities

- **ComponentRegistry**: A standalone, independently instantiable catalog mapping string names to component factories. Multiple registries may coexist (no global state). Thread-safe. Supports register, unregister, create-by-name, and list operations.
- **ComponentFactory**: A callable that accepts a type-erased configuration parameter and produces a new component instance. The configuration is optional (callers may pass no config for components that don't require it). Returns a reference-counted component handle.
- **ComponentRef**: A reference-counted handle wrapping Arc internally. Attach clones the Arc (incrementing the reference count); release drops the handle (decrementing it). Rust's ownership system prevents use-after-free at compile time. Provides access to the underlying component's IUnknown interface.
- **Assembler**: A role (not necessarily a distinct entity) that wires components together using third-party binding. Operates on ComponentRef handles and interface/receptacle metadata.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Developers can register a factory and create a component by name in under 5 lines of code.
- **SC-002**: Reference counting correctly manages component lifetime — zero leaks and zero use-after-free in all test scenarios.
- **SC-003**: Concurrent registry access from 10+ threads produces no data races or incorrect results.
- **SC-004**: First-party binding works identically to the existing receptacle wiring mechanism (backward compatible).
- **SC-005**: Third-party binding can wire any two compatible components without compile-time knowledge of their concrete types.
- **SC-006**: End-to-end registry-create-bind-invoke workflow completes correctly for a multi-component scenario.
- **SC-007**: All public APIs have doc tests and unit tests per project constitution.

## Clarifications

### Session 2026-03-31

- Q: How does ComponentRef relate to Rust's Arc? → A: ComponentRef wraps Arc internally. Attach clones the Arc, release drops it. Rust ownership prevents use-after-free at compile time.
- Q: What happens when all ComponentRef handles are released but a receptacle still holds a connection? → A: Receptacles hold Arc, so the component stays alive. Natural Arc semantics, no special logic needed.
- Q: Should the registry be a singleton or allow multiple instances? → A: Multiple independent registries. Each is a standalone value, no global state. Supports test isolation and partitioned namespaces.
- Q: Should factories accept configuration parameters at creation time? → A: Yes, typed configuration. Factory accepts a type-erased config parameter (optional — callers may pass none for unconfigured components).
- Q: How should third-party binding match receptacles to interfaces without compile-time types? → A: String name matching. Assembler uses interface/receptacle names; framework resolves and verifies TypeId internally for type safety.

## Assumptions

- The existing `IUnknown`, `Receptacle`, `define_interface!`, and `define_component!` implementations from the first feature set are stable and will be extended (not replaced).
- First-party binding already works via the current receptacle connect/disconnect mechanism; this feature formalizes the terminology and adds third-party binding on top.
- Component factories are synchronous (no async factory support in this iteration).
- The component registry is a runtime construct (not compile-time); components are registered programmatically, not via configuration files.
- "Third-party binding" means the assembler code does not use the concrete component type — it operates through the `IUnknown` trait and metadata only.
- Platform remains Linux-only with Rust stable toolchain per the project constitution.
