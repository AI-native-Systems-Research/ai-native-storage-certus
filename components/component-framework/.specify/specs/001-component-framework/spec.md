# Feature Specification: Component Framework

**Feature Branch**: `001-component-framework`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The Component Framework is a COM-inspired Rust component system for Linux that provides a structured way to define, discover, connect, and manage software components at runtime through standardized interfaces. It enables low-coupling component-based architecture with typed interface contracts, runtime interface discovery via `IUnknown`, receptacle-based dependency injection, and an actor model with dedicated OS threads for message-loop concurrency.

The framework is organized into three crates: `component-core` (types, traits, channels, NUMA support), `component-macros` (procedural macros for `define_interface!` and `define_component!`), and `component-framework` (facade crate re-exporting everything for single-dependency usage). It targets Linux exclusively using Rust stable (edition 2021, MSRV 1.75+) and provides first-class support for high-performance lock-free channels, NUMA-aware thread pinning, and comprehensive benchmarking.

## User Scenarios & Testing

### User Story 1 - Define and Query Interfaces (Priority: P1)
As a component developer, I want to define typed interfaces using a declarative macro and query them at runtime from any component, so that I can compose components without compile-time knowledge of concrete types.

**Acceptance Scenarios**:
- Given I define an interface with `define_interface!`, when I implement it on a component defined with `define_component!`, then I can query that interface at runtime using `query::<dyn IFoo + Send + Sync>()` and receive an `Arc<dyn IFoo + Send + Sync>`.
- Given I define an interface, when I attempt to use `&mut self` in a method signature, then the macro emits a compile error requiring `&self` with interior mutability.
- Given I define an interface with zero methods, then the macro emits a compile error requiring at least one method.

### User Story 2 - Wire Components via Receptacles (Priority: P1)
As a system assembler, I want to declare typed dependency slots (receptacles) on components and wire them together at runtime using string-based binding, so that I can compose systems without compile-time knowledge of all component types.

**Acceptance Scenarios**:
- Given a consumer component with a receptacle named "backend" requiring `IStorage`, when I call `bind(&provider, "IStorage", &consumer, "backend")`, then the consumer's receptacle is connected to the provider's interface.
- Given a receptacle that is already connected, when I attempt to bind again without disconnecting first, then I receive `ReceptacleError::AlreadyConnected`.
- Given a provider that does not implement the required interface, when I attempt binding, then I receive `RegistryError::BindingFailed` with a type mismatch message.

### User Story 3 - Actor Message Processing (Priority: P1)
As a developer, I want to create actors that own dedicated OS threads and process messages sequentially with lifecycle management, so that I can build concurrent systems with isolated mutable state.

**Acceptance Scenarios**:
- Given an actor with an `ActorHandler<M>`, when I call `activate()`, then a new OS thread is spawned and `on_start()` is called before the message loop begins.
- Given an active actor, when I call `handle.deactivate()`, then remaining messages are drained, `on_stop()` is called, and the thread is joined.
- Given an actor whose handler panics during `handle()`, then the panic is caught, the error callback is invoked, and the actor continues processing subsequent messages.
- Given an actor with CPU affinity set, when I activate it, then its thread is pinned to the specified CPUs.

### User Story 4 - Channel-Based Communication (Priority: P1)
As a developer, I want high-performance lock-free channels as first-class components that can be discovered and bound through the standard interface model, so that I can compose communication topologies declaratively.

**Acceptance Scenarios**:
- Given a `SpscChannel<T>`, when I request a second sender, then `ChannelError::BindingRejected` is returned (SPSC topology enforced).
- Given a `MpscChannel<T>`, when multiple senders send messages, then the single receiver receives all messages.
- Given all senders are dropped, when the receiver drains remaining messages, then it receives `ChannelError::Closed`.

### User Story 5 - Component Registry (Priority: P2)
As an application builder, I want to register component factories by name and create instances at runtime with optional configuration, so that I can build plugin-style architectures.

**Acceptance Scenarios**:
- Given a registered factory "cache", when I call `registry.create("cache", Some(&2048usize))`, then a new component instance is created with the provided configuration.
- Given a factory that panics, when I call `registry.create(...)`, then the panic is caught and `RegistryError::FactoryFailed` is returned.
- Given a name that is already registered, when I call `registry.register(...)`, then `RegistryError::AlreadyRegistered` is returned.

### User Story 6 - NUMA-Aware Actors (Priority: P2)
As a performance engineer, I want to discover NUMA topology at runtime, pin actor threads to specific cores, and allocate memory on specific NUMA nodes, so that I can minimize cross-node memory access penalties.

**Acceptance Scenarios**:
- Given a Linux system with NUMA nodes, when I call `NumaTopology::discover()`, then I get the correct node count, per-node CPU lists, and inter-node distances.
- Given an actor with CPU affinity set to invalid/offline CPUs, when I call `activate()`, then `ActorError::AffinityFailed` is returned and no thread is left running.
- Given a non-NUMA system, when I call `NumaTopology::discover()`, then it falls back to a single-node topology containing all online CPUs.

## Requirements

### Functional Requirements

#### Macros

- **FR-001**: `define_interface!` SHALL generate a trait with `Send + Sync + 'static` bounds and an `Interface` marker implementation for the trait object type `dyn T + Send + Sync`.
- **FR-002**: `define_interface!` SHALL reject interfaces with zero methods at compile time.
- **FR-003**: `define_interface!` SHALL reject methods using `&mut self` at compile time, requiring `&self` with interior mutability.
- **FR-004**: `define_interface!` SHALL support lifetime parameters in method signatures.
- **FR-005**: `define_component!` SHALL generate a struct with automatic `IUnknown` implementation, interface map population, receptacle fields, and an `Arc<Self>`-returning `new()` constructor.
- **FR-006**: `define_component!` SHALL require a `version` field as the first entry and a `provides` list as the second entry; `receptacles` and `fields` are optional.
- **FR-007**: `define_component!` SHALL generate a `new_default()` constructor when user `fields` are present, using `Default::default()` for each field.
- **FR-008**: `define_component!` SHALL generate `Send + Sync` implementations for the component struct.
- **FR-009**: `define_component!` SHALL generate `connect_receptacle_raw` that matches receptacle names, queries the provider for the needed interface, and connects it.

#### IUnknown and Interface Query

- **FR-010**: Every component SHALL implement `IUnknown` providing `query_interface_raw`, `version`, `provided_interfaces`, `receptacles`, and `connect_receptacle_raw`.
- **FR-011**: The `query<I>()` free function SHALL look up interface `I` by `TypeId::of::<Arc<I>>()`, downcast the result, and return a cloned `Arc<I>`.
- **FR-012**: The `query_interface!` macro SHALL accept `Arc<T>`, `&T`, and `ComponentRef` as the component argument, eliminating the need to spell `dyn Trait + Send + Sync`.
- **FR-013**: `provided_interfaces()` SHALL always include `IUnknown` itself when generated by `define_component!`.

#### ComponentRef

- **FR-014**: `ComponentRef` SHALL wrap `Arc<dyn IUnknown>` and implement `Deref<Target = dyn IUnknown>`, `Clone` (via `attach()`), `Send`, `Sync`, and `From<Arc<T>>` for any `T: IUnknown + 'static`.
- **FR-015**: `ComponentRef::ref_count()` SHALL return the current strong reference count of the inner Arc.
- **FR-016**: When the last `ComponentRef` is dropped, the underlying component SHALL be deallocated.

#### Receptacles

- **FR-017**: `Receptacle<T>` SHALL be a typed slot that holds at most one `Arc<T>` connection at a time.
- **FR-018**: `connect()` SHALL return `ReceptacleError::AlreadyConnected` if a provider is already connected.
- **FR-019**: `disconnect()` SHALL return `ReceptacleError::NotConnected` if no provider is connected.
- **FR-020**: `get()` SHALL return a clone of the connected `Arc<T>` or `ReceptacleError::NotConnected`.
- **FR-021**: `Receptacle<T>` SHALL be `Send + Sync` and use `RwLock` internally for concurrent `get()` access with exclusive `connect()`/`disconnect()`.

#### Binding

- **FR-022**: `bind(provider, interface_name, consumer, receptacle_name)` SHALL resolve the interface by name from the provider's metadata, verify TypeId compatibility with the consumer's receptacle, and delegate to `connect_receptacle_raw`.
- **FR-023**: `bind()` SHALL return `RegistryError::BindingFailed` if: the provider lacks the named interface, the consumer lacks the named receptacle, types are incompatible, or the receptacle is already connected.

#### Component Registry

- **FR-024**: `ComponentRegistry` SHALL map string names to factory closures and support concurrent access via `RwLock`.
- **FR-025**: `register()` SHALL reject duplicate names with `RegistryError::AlreadyRegistered`.
- **FR-026**: `create()` SHALL catch factory panics and convert them to `RegistryError::FactoryFailed`.
- **FR-027**: `unregister()` SHALL remove a factory by name or return `RegistryError::NotFound`.
- **FR-028**: `register_simple()` SHALL provide a convenience wrapper for factories that ignore configuration.
- **FR-029**: `list()` SHALL return all registered component names.

#### Actor Model

- **FR-030**: `Actor<M, H>` SHALL own a dedicated OS thread and process messages of type `M` sequentially through the handler `H`.
- **FR-031**: `activate()` SHALL spawn the actor's thread, call `on_start()`, and return an `ActorHandle<M>`. It SHALL fail with `ActorError::AlreadyActive` if the actor is running.
- **FR-032**: `deactivate()` SHALL close the channel, drain remaining messages, call `on_stop()`, and join the thread.
- **FR-033**: Panics in `handle()` SHALL be caught via `std::panic::catch_unwind`, the error callback invoked, and the actor SHALL continue processing.
- **FR-034**: Panics in `on_idle()` SHALL be caught similarly; the idle counter SHALL increment on panic.
- **FR-035**: `Actor` SHALL implement `IUnknown` and provide `ISender<M>` as a queryable interface, allowing other components to send messages through the standard component model.
- **FR-036**: The actor's internal channel SHALL be created at construction time so `ISender<M>` is available via IUnknown even before activation.
- **FR-037**: `ActorHandle::signal_stop()` SHALL close the channel without joining the thread, enabling concurrent multi-actor shutdown.
- **FR-038**: Dropping an `ActorHandle` without calling `deactivate()` SHALL close the channel and perform a best-effort thread join.
- **FR-039**: `Actor::with_capacity()` SHALL allow custom MPSC channel buffer sizes (must be power of two).
- **FR-040**: `Actor::simple()` SHALL create an actor with default capacity (1024) and a no-op error callback.

#### Actor NUMA Support

- **FR-041**: `Actor::with_cpu_affinity(CpuSet)` SHALL set a CPU affinity mask (builder pattern) applied on activation.
- **FR-042**: `Actor::set_cpu_affinity()` SHALL only succeed when the actor is idle; it SHALL return `ActorError::AlreadyActive` if running.
- **FR-043**: On activation, if CPU affinity is set, the actor SHALL validate CPU IDs against online CPUs before spawning, and apply `sched_setaffinity` on the spawned thread. Failure SHALL return `ActorError::AffinityFailed` and leave the actor idle.
- **FR-044**: The actor's poll loop SHALL use a progressive backoff strategy: spin (< 64 iterations), yield (64-255), then park with timeout (>= 256).

#### Actor Utilities

- **FR-045**: `pipe(Receiver<M>, ActorHandle<M>)` SHALL spawn a forwarder thread that reads from the receiver and sends to the actor, deactivating the actor when the channel closes.
- **FR-046**: `pipe_mpsc(MpscReceiver<M>, ActorHandle<M>)` SHALL provide the same forwarding for MPSC receivers.

#### Channels - SPSC

- **FR-047**: `SpscChannel<T>` SHALL provide a lock-free ring buffer with exactly one sender and one receiver endpoint.
- **FR-048**: `SpscChannel::sender()` SHALL reject a second sender with `ChannelError::BindingRejected`.
- **FR-049**: `SpscChannel::receiver()` SHALL reject a second receiver with `ChannelError::BindingRejected`.
- **FR-050**: `SpscChannel` SHALL implement `IUnknown` providing `ISender<T>` and `IReceiver<T>` interfaces.
- **FR-051**: The ring buffer capacity SHALL be a power of two; construction SHALL panic if it is not.
- **FR-052**: `Sender::send()` SHALL block with progressive backoff (spin/yield/park) when the queue is full.
- **FR-053**: `Receiver::recv()` SHALL block with progressive backoff when the queue is empty.
- **FR-054**: When all senders are dropped, the receiver SHALL return `ChannelError::Closed` after draining remaining messages.
- **FR-055**: `try_send()` SHALL return `ChannelError::Full` without blocking; `try_recv()` SHALL return `ChannelError::Empty` without blocking.

#### Channels - MPSC

- **FR-056**: `MpscChannel<T>` SHALL provide a lock-free bounded queue (Vyukov-style with per-slot sequence numbers) supporting multiple senders and one receiver.
- **FR-057**: `MpscChannel::sender()` SHALL allow creating multiple senders (cloneable).
- **FR-058**: `MpscChannel::receiver()` SHALL reject a second receiver with `ChannelError::BindingRejected`.
- **FR-059**: `MpscChannel` SHALL implement `IUnknown` providing `ISender<T>` and `IReceiver<T>` interfaces.
- **FR-060**: `MpscChannel::close()` SHALL force-close the channel so the receiver exits even when senders from IUnknown queries are still alive.

#### Channels - Third-Party Backends

- **FR-061**: `CrossbeamBoundedChannel<T>` SHALL wrap crossbeam-channel bounded MPMC and implement `ISender<T>` + `IReceiver<T>` + `IUnknown`.
- **FR-062**: `CrossbeamUnboundedChannel<T>` SHALL wrap crossbeam-channel unbounded MPMC and implement the same interfaces.
- **FR-063**: `KanalChannel<T>` SHALL wrap kanal bounded MPMC and implement the same interfaces.
- **FR-064**: `RtrbChannel<T>` SHALL wrap rtrb SPSC lock-free and enforce single sender/receiver binding constraints.
- **FR-065**: `TokioMpscChannel<T>` SHALL wrap tokio MPSC (sync feature) and implement the same interfaces.

#### Logging

- **FR-066**: `LogHandler` SHALL implement `ActorHandler<LogMessage>` writing timestamped lines in format `YYYY-MM-DDTHH:MM:SS.mmmZ [LEVEL] text` to stderr.
- **FR-067**: `LogHandler::with_file(path)` SHALL additionally write log lines to the specified file (append mode, created if absent).
- **FR-068**: `LogHandler::with_min_level(level)` SHALL filter messages below the specified severity level.
- **FR-069**: `LogHandler::on_stop()` SHALL flush the file buffer to ensure all lines are written before the actor exits.
- **FR-070**: `LogLevel` SHALL have the ordering `Debug < Info < Warn < Error`.
- **FR-071**: `LogMessage` SHALL provide factory methods `debug()`, `info()`, `warn()`, `error()` and accessors `level()`, `text()`.

#### NUMA Awareness

- **FR-072**: `CpuSet` SHALL wrap `libc::cpu_set_t` and provide `new()`, `from_cpu()`, `from_cpus()`, `add()`, `remove()`, `contains()`, `count()`, `is_empty()`, `iter()` operations.
- **FR-073**: `CpuSet::add()` SHALL return `NumaError::CpuOutOfRange` for CPU IDs >= `CPU_SETSIZE`.
- **FR-074**: `CpuSet::iter()` SHALL yield CPU IDs in ascending order.
- **FR-075**: `CpuSet` SHALL implement `Clone`, `Debug`, `Default`, `Send`, and `Sync`.
- **FR-076**: `set_thread_affinity(cpuset)` SHALL call `sched_setaffinity` on the calling thread and return `NumaError::EmptyCpuSet` for empty sets or `NumaError::AffinityFailed` on syscall failure.
- **FR-077**: `get_thread_affinity()` SHALL call `sched_getaffinity` and return the current thread's CPU mask.
- **FR-078**: `validate_cpus(cpuset)` SHALL read `/sys/devices/system/cpu/online` and return `NumaError::CpuOffline` for any CPU not in the online set.
- **FR-079**: `NumaTopology::discover()` SHALL read sysfs to enumerate NUMA nodes, their CPUs, and inter-node distances. It SHALL fall back to a single-node topology on non-NUMA systems.
- **FR-080**: `NumaNode` SHALL expose `id()`, `cpus()` (as CpuSet), and distance information.
- **FR-081**: `NumaAllocator` SHALL allocate memory bound to a specific NUMA node using `mmap` + `mbind`.

#### Error Types

- **FR-082**: `ReceptacleError` SHALL have variants `NotConnected` and `AlreadyConnected`, implementing `Display`, `Debug`, `Clone`, `PartialEq`, `Eq`, and `std::error::Error`.
- **FR-083**: `QueryError` SHALL have variant `InterfaceNotFound` with the same trait implementations.
- **FR-084**: `RegistryError` SHALL have variants `NotFound`, `AlreadyRegistered`, `FactoryFailed`, and `BindingFailed` with the same trait implementations.
- **FR-085**: `ChannelError` SHALL have variants `Full`, `Empty`, `Closed`, and `BindingRejected` with the same trait implementations.
- **FR-086**: `ActorError` SHALL have variants `AlreadyActive`, `NotActive`, `SendFailed`, `ShutdownTimeout`, and `AffinityFailed` with the same trait implementations.
- **FR-087**: `NumaError` SHALL have variants `CpuOutOfRange`, `CpuOffline`, `EmptyCpuSet`, `InvalidNode`, `TopologyUnavailable`, `AffinityFailed`, and `AllocationFailed`.

#### Facade and Prelude

- **FR-088**: The `component-framework` facade crate SHALL re-export all public items from `component-core` and `component-macros`.
- **FR-089**: `component_framework::prelude` SHALL re-export core prelude items plus `define_interface!` and `define_component!`.
- **FR-090**: `declare_interface!` and `declare_component!` SHALL be backwards-compatible aliases that forward to the primary macros.

#### InterfaceMap

- **FR-091**: `InterfaceMap` SHALL store `Box<dyn Any + Send + Sync>` keyed by `TypeId`, supporting `insert()`, `lookup()`, and `info()` for introspection metadata.

### Non-Functional Requirements

- **NFR-001**: All channel operations (send/recv) SHALL be lock-free on the fast path, using atomic operations only.
- **NFR-002**: The framework SHALL be Linux-only; no support for Windows or macOS is required.
- **NFR-003**: MSRV SHALL be Rust 1.75+ with stable toolchain only (no nightly features).
- **NFR-004**: All public types SHALL be `Send + Sync` to support safe multi-threaded composition.
- **NFR-005**: All public APIs SHALL have doc comments with runnable examples; `cargo doc --no-deps` SHALL be warning-free.
- **NFR-006**: Performance-sensitive code (channels, query, binding) SHALL have Criterion benchmarks.
- **NFR-007**: Unsafe code SHALL have `// SAFETY:` justification comments.
- **NFR-008**: The actor poll loop SHALL use progressive backoff (spin, yield, park with 10ms timeout after 10M idle iterations) to balance latency and CPU usage.
- **NFR-009**: Channel capacity SHALL be power-of-two to enable branchless index wrapping via bitwise AND.
- **NFR-010**: The framework SHALL support concurrent registry access, factory registration, and component creation without external synchronization.

## Key Entities

- **IUnknown**: Base trait for all components providing runtime interface discovery, version, introspection, and receptacle connection.
- **InterfaceMap**: Internal HashMap keyed by TypeId storing Arc interface references for runtime lookup.
- **ComponentRef**: Type-erased `Arc<dyn IUnknown>` wrapper with reference counting semantics.
- **Receptacle<T>**: Typed dependency slot holding at most one `Arc<T>` connection, with RwLock-based thread safety.
- **ComponentRegistry**: Thread-safe name-to-factory map for runtime component creation.
- **Actor<M, H>**: Thread-owning component with message-loop semantics, IUnknown integration, and NUMA-aware thread pinning.
- **ActorHandle<M>**: Handle to a running actor for sending messages and lifecycle management.
- **ActorHandler<M>**: Trait defining message-handling behavior with lifecycle hooks (on_start, on_stop, on_idle).
- **SpscChannel<T>**: Lock-free single-producer single-consumer ring buffer channel component.
- **MpscChannel<T>**: Lock-free multi-producer single-consumer bounded queue channel component.
- **ISender<T>** / **IReceiver<T>**: Interface traits for channel endpoints, queryable via IUnknown.
- **CpuSet**: CPU affinity mask wrapping libc::cpu_set_t with safe operations.
- **NumaTopology**: Runtime NUMA layout discovery from Linux sysfs.
- **NumaAllocator**: NUMA-local memory allocator using mmap + mbind.
- **LogHandler**: Reusable ActorHandler for timestamped logging to stderr and optional file output.
- **InterfaceInfo** / **ReceptacleInfo**: Metadata structs for runtime introspection of provided interfaces and required receptacles.

## Dependencies

- **libc** (0.2): Linux syscall wrappers for sched_setaffinity, sched_getaffinity, mmap, mbind
- **crossbeam-channel** (0.5): Bounded and unbounded MPMC channel backend
- **kanal** (0.1): High-performance bounded MPMC channel backend
- **rtrb** (0.3): Lock-free SPSC ring buffer channel backend
- **tokio** (1.x, sync feature): Async-capable MPSC channel backend
- **proc-macro2**, **quote**, **syn**: Procedural macro infrastructure for define_interface! and define_component!
- **criterion** (0.5, dev): Benchmarking framework for performance regression detection

## Success Criteria

- **SC-001**: `cargo test --all` passes with zero failures across all crates (component-core, component-macros, component-framework).
- **SC-002**: `cargo clippy -- -D warnings` reports no warnings.
- **SC-003**: `cargo fmt --check` reports no formatting issues.
- **SC-004**: `cargo doc --no-deps` completes without warnings.
- **SC-005**: All 13 Criterion benchmark suites compile and run without error.
- **SC-006**: Compile-fail tests verify that invalid macro usage (empty interfaces, `&mut self`, missing version) is rejected.
- **SC-007**: Actors with CPU affinity correctly pin their threads to specified cores (verified via `sched_getaffinity` in test).
- **SC-008**: Channel topology constraints are enforced at bind time (SPSC rejects second sender/receiver; MPSC rejects second receiver).
- **SC-009**: Third-party binding successfully connects components by string name without compile-time knowledge of concrete types.
- **SC-010**: Actor panic recovery allows continued message processing after handler panics.

## Implementation Notes

> These notes capture current implementation details that may not be part of the desired specification but document how the system works today.

- The `define_component!` macro uses an unsafe post-construction initialization pattern (`__init_interfaces_*` function) that mutates the struct through a raw pointer from the sole `Arc` reference. This is safe because no other thread can observe the mutation during construction.
- User fields are declared before receptacle fields in the generated struct to ensure actors (held in user fields) are joined before dependencies (held in receptacles) are released during drop.
- The actor message loop uses a park threshold of 10,000,000 idle iterations before switching to `thread::park_timeout(10ms)`. The `on_idle()` callback allows actors to do background work (e.g., polling IO completions) during idle periods.
- The SPSC ring buffer uses separate cache-line-aligned head and tail atomics to avoid false sharing between producer and consumer threads.
- `MpscChannel` uses a Vyukov-style bounded queue with per-slot sequence numbers for wait-free enqueue by multiple producers.
- The `force_closed` flag on channel state allows actor deactivation to terminate the receiver even when other senders (obtained via IUnknown queries) are still alive.
- `NumaTopology::discover()` reads from `/sys/devices/system/node/` and `/sys/devices/system/cpu/` sysfs paths.
- The facade crate provides `declare_interface!` and `declare_component!` as backwards-compatible aliases.
- `ComponentRef` implements unsafe `Send + Sync` manually; this is sound because `Arc<dyn IUnknown>` is `Send + Sync` given `IUnknown: Send + Sync`.
