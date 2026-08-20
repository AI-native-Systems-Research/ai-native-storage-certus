# Component Framework

A Rust component framework for Linux inspired by COM (Component Object Model) principles. It provides a structured way to define, discover, connect, and manage software components at runtime through standardized interfaces, with first-class support for the actor model and high-performance lock-free channels.

## Core Concepts

### Interface Definition

Components expose capabilities through **interfaces** — trait objects that can be queried at runtime. The `define_interface!` macro generates traits with the necessary metadata for runtime discovery:

```rust
define_interface! {
    pub IStorage {
        fn read(&self, key: &str) -> Option<Vec<u8>>;
        fn write(&self, key: &str, value: &[u8]) -> Result<(), String>;
    }
}
```

All interface methods take `&self` (interior mutability for thread safety). Lifetime parameters in method signatures are supported.

### Component Definition

Components are concrete types that implement one or more interfaces. Interfaces and receptacles must to associated with a component. The `define_component!` macro generates the boilerplate for `IUnknown` implementation, interface map construction, and receptacle wiring:

```rust
define_component! {
    pub CacheComponent {
        version: "1.0.0",
        provides: [IStorage],
        receptacles: {
            backend: IStorage,
        },
        fields: {
            capacity: usize,
        },
    }
}
```

Each component gets an `Arc`-returning `::new()` constructor, automatic `IUnknown` implementation, and `Send + Sync` guarantees. Components with user-defined `fields` also get a `::new_default()` constructor that initializes all fields to their `Default` values.

### IUnknown and Interface Querying

Every component implements `IUnknown`, the base trait providing:

- **`query_interface_raw`** — runtime interface lookup by `TypeId`
- **`version`** — component version string
- **`provided_interfaces`** — list of all interfaces the component provides
- **`receptacles`** — list of all required interface slots

The type-safe `query<I>()` free function wraps the raw lookup:

```rust
let storage: Arc<dyn IStorage + Send + Sync> = query::<dyn IStorage + Send + Sync>(&*comp).unwrap();
```

The `query_interface!` convenience macro eliminates the need to spell out `dyn Trait + Send + Sync` and works with direct references, `Arc<T>`, and `ComponentRef`:

```rust
let storage: Arc<dyn IStorage + Send + Sync> = query_interface!(comp, IStorage).unwrap();
```

### ComponentRef

`ComponentRef` is a type-erased wrapper around `Arc<dyn IUnknown>`. It allows components to be stored, passed, and managed without knowing the concrete type, while still supporting interface queries and introspection.

## Component Registry

`ComponentRegistry` maps string names to factory closures. Factories receive an optional `&dyn Any` configuration parameter and return a `ComponentRef`:

```rust
let registry = ComponentRegistry::new();
registry.register("cache", |config| {
    let cap = config.and_then(|c| c.downcast_ref::<usize>()).copied().unwrap_or(1024);
    Ok(ComponentRef::from(CacheComponent::new(cap)))
}).unwrap();

let comp = registry.create("cache", Some(&2048usize)).unwrap();
```

The registry is thread-safe (`RwLock`-based) and supports concurrent access, factory registration, unregistration, and component creation.

## Reference Counting

Components use `Arc`-based atomic reference counting. `ComponentRef::from(arc)` wraps any `Arc<dyn IUnknown>`. Cloning increments the strong count; dropping decrements it. When the last reference is dropped, the component is deallocated.

## Receptacles and Binding

A **receptacle** is a typed slot representing a required interface dependency. Components declare receptacles in `define_component!` and consumers call `.get()` to access the connected provider.

Two binding modes are supported:

- **First-party binding** — the application has compile-time knowledge of both components and connects them directly through the receptacle field
- **Third-party binding** — an assembler connects components by string names, with no compile-time knowledge of concrete types, using the `bind()` function

```rust
// Third-party binding by name
bind(&*provider, "IStorage", &*consumer, "backend").unwrap();
```

## Actor Model

Actors are components that own a dedicated OS thread and process messages sequentially. The `ActorHandler<M>` trait defines the message processing contract:

```rust
impl ActorHandler<MyMessage> for MyHandler {
    fn handle(&mut self, msg: MyMessage) { /* process message */ }
    fn on_start(&mut self) { /* called once before message loop */ }
    fn on_stop(&mut self) { /* called once after message loop exits */ }
}
```

Key actor features:

- **Dedicated thread** — each actor runs on its own OS thread with exclusive `&mut self` access
- **Lifecycle management** — `activate()` spawns the thread and returns an `ActorHandle`; `deactivate()` shuts it down gracefully
- **Panic recovery** — panics in `handle()` are caught and reported via a user-supplied callback; the actor continues processing
- **IUnknown integration** — actors implement `IUnknown` and provide `ISender<M>` as a queryable interface, enabling other components to send messages without knowing the concrete actor type
- **Configurable capacity** — `Actor::with_capacity()` sets the internal MPSC channel buffer size (default 1024)

## Channels

Channels are first-class components implementing `ISender<T>` and `IReceiver<T>`. All channel types are queryable via `IUnknown` and enforce binding topology at runtime.

### Built-in Channels

| Type | Topology | Implementation |
|------|----------|---------------|
| `SpscChannel<T>` | Single-producer, single-consumer | Lock-free ring buffer with atomic head/tail |
| `MpscChannel<T>` | Multi-producer, single-consumer | Lock-free Vyukov bounded queue with per-slot sequence numbers |

Both use power-of-two capacity, support blocking and non-blocking send/recv, and signal channel closure when all senders or the receiver are dropped.

### Third-Party Channel Backends

Drop-in replacements that implement the same `ISender`/`IReceiver` interface:

| Type | Library | Notes |
|------|---------|-------|
| `CrossbeamBoundedChannel<T>` | crossbeam-channel 0.5 | Bounded MPMC |
| `CrossbeamUnboundedChannel<T>` | crossbeam-channel 0.5 | Unbounded MPMC |
| `KanalChannel<T>` | kanal 0.1 | Bounded MPMC |
| `RtrbChannel<T>` | rtrb 0.3 | SPSC-only, lock-free |
| `TokioMpscChannel<T>` | tokio 1.x (sync) | Async-capable MPSC |

All backends enforce binding constraints (e.g., SPSC channels reject a second sender) and are interchangeable through the interface abstraction.

## Built-in Logging

`LogHandler` is a reusable `ActorHandler<LogMessage>` that writes timestamped log lines to stderr and optionally to a file:

```rust
let handler = LogHandler::with_file("/tmp/app.log").unwrap()
    .with_min_level(LogLevel::Warn);
let actor = Actor::new(handler, |_| {});
let handle = actor.activate().unwrap();
handle.send(LogMessage::info("filtered out")).unwrap();
handle.send(LogMessage::error("this appears on stderr and in the file")).unwrap();
handle.deactivate().unwrap();
```

Log levels: `Debug`, `Info`, `Warn`, `Error`. Line format: `2026-04-01T14:23:05.123Z [INFO ] message text`.

## NUMA Awareness

The framework provides Linux NUMA (Non-Uniform Memory Access) support for performance-critical deployments:

### Thread Pinning

Actors can be pinned to specific CPU cores at construction or changed between activation cycles:

```rust
let actor = Actor::new(handler, |_| {})
    .with_cpu_affinity(CpuSet::from_cpus(&[0, 1]).unwrap());
let handle = actor.activate().unwrap(); // thread pinned to CPUs 0 and 1
handle.deactivate().unwrap();

// Change affinity while idle
actor.set_cpu_affinity(CpuSet::from_cpu(2).unwrap()).unwrap();
```

### Topology Discovery

Runtime discovery of NUMA nodes, their CPUs, and inter-node distances via Linux sysfs:

```rust
let topo = NumaTopology::discover().unwrap();
for node in topo.nodes() {
    println!("Node {}: CPUs {:?}", node.id(), node.cpus().iter().collect::<Vec<_>>());
}
```

Falls back to a single-node topology on non-NUMA systems.

### NUMA-Local Allocation

`NumaAllocator` allocates memory bound to a specific NUMA node using `mmap` + `mbind`. Channel constructors (`new_numa`) support NUMA-aware allocation via first-touch policy when threads are properly pinned.

### Public API

- `CpuSet` — set of CPU core IDs with add/remove/contains/iterate operations
- `set_thread_affinity` / `get_thread_affinity` — safe wrappers around `sched_setaffinity` / `sched_getaffinity`
- `validate_cpus` — verify all CPUs in a set are online
- `NumaTopology` — discover nodes, look up `node_for_cpu`, enumerate `online_cpus`
- `NumaNode` — per-node CPU list and inter-node distance vector
- `NumaAllocator` — NUMA-local allocation via `mmap` + `mbind`

## Benchmarks

13 Criterion benchmark suites covering:

| Area | Benchmarks |
|------|------------|
| Channel throughput | SPSC and MPSC across all backends, message sizes (u64, 1KB Vec), capacities (64, 1024, 16384) |
| Channel latency | Round-trip latency for SPSC and MPSC configurations |
| NUMA performance | Same-node vs cross-node latency and throughput for SPSC channels |
| Component operations | `query_interface`, `ComponentRef` creation, registry lookup, receptacle connect/get, method dispatch, binding, actor activation latency |

Run all benchmarks: `cargo bench`

## Examples

| Example | Description |
|---------|-------------|
| `basic` | Define an interface, implement a component, query it at runtime |
| `wiring` | Connect components via receptacles (required interface slots) |
| `introspection` | Enumerate provided interfaces and receptacles via IUnknown |
| `binding` | First-party vs third-party component binding |
| `actor_ping_pong` | Bidirectional actor communication through SPSC channels |
| `actor_pipeline` | Three-stage producer-processor-consumer pipeline |
| `actor_fan_in` | Multiple producers feeding a single consumer actor via MPSC |
| `actor_factory` | Registry-based actor creation with typed configuration |
| `actor_log` | Built-in LogHandler: stderr, file output, level filtering |
| `tokio_ping_pong` | Tokio MPSC channel components queried through IUnknown |
| `numa_pinning` | NUMA topology discovery, thread pinning, cross-node latency measurement |

A separate [pingpong example crate](../component-framework-example-pingpong) demonstrates importing shared interface and component definitions from another crate.

## Project Structure

```
component-framework/
├── crates/
│   ├── component-core/          Core types, traits, and implementations
│   │   └── src/
│   │       ├── actor.rs          Actor, ActorHandle, ActorHandler
│   │       ├── binding.rs        Third-party binding
│   │       ├── channel/          All channel types (built-in + backends)
│   │       ├── component.rs      InterfaceMap
│   │       ├── component_ref.rs  Type-erased component wrapper
│   │       ├── error.rs          Error types
│   │       ├── interface.rs      InterfaceInfo, ReceptacleInfo
│   │       ├── iunknown.rs       IUnknown trait, query() function
│   │       ├── log.rs            LogHandler, LogLevel, LogMessage
│   │       ├── numa/             NUMA topology, affinity, allocator
│   │       ├── receptacle.rs     Required interface slots
│   │       └── registry.rs       ComponentRegistry with factory pattern
│   ├── component-macros/         Proc macros (define_interface!, define_component!)
│   └── component-framework/      Facade crate re-exporting everything
│       └── benches/              13 Criterion benchmark suites
└── examples/                     11 runnable examples
```

## Platform and Toolchain

- **Platform**: Linux only
- **Toolchain**: Rust stable, edition 2021, MSRV 1.75+
- **External dependencies**: libc, crossbeam-channel, kanal, rtrb, tokio (sync), criterion (dev)
