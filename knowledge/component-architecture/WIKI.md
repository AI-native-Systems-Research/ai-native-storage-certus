This is a placeholder for the knowledge base wiki.

Inspired by Karpathy's blog: https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f

# Software Component Architecture

## Szyperski's Component Software (Key Concepts)

The following summarizes foundational ideas from *Component Software: Beyond
Object-Oriented Programming* by Clemens Szyperski (2nd edition, 2002). These
principles inform the design of our component framework.

### What Is a Software Component?

> "A software component is a unit of composition with contractually specified
> interfaces and explicit context dependencies only. A software component can
> be deployed independently and is subject to composition by third parties."

Key properties that distinguish components from plain objects or modules:

- **Independent deployment** — a component is a deliverable unit that can be
  installed, versioned, and replaced without recompiling consumers.
- **Third-party composition** — components are assembled by someone other than
  the original author; the assembler works only through published interfaces.
- **No persistent state in the component itself** — a component is not an
  instance. It is a factory/template from which instances are created.
  Instances hold state; the component definition does not.

### Interfaces as Contracts

Interfaces are the sole mechanism by which components interact. Szyperski
emphasizes:

- **Syntactic contract** — method signatures, types, and ordering.
- **Semantic contract** — pre/post-conditions, invariants, and protocols that
  govern the order and meaning of calls.
- **Immutability of published interfaces** — once an interface is released, it
  must not change. New functionality is added through new interfaces, not by
  modifying existing ones.
- **Interface discovery** — components must support runtime querying of which
  interfaces they provide (cf. COM's `IUnknown::QueryInterface`).

### Separation of Interfaces from Implementations

A central theme: the strict separation between what a component offers
(its interfaces) and how it achieves it (its implementation). This enables:

- **Polymorphic substitution** — any component implementing an interface can
  replace another.
- **Independent evolution** — implementations change without breaking consumers.
- **Late binding** — wiring of providers to consumers can happen at deployment
  or even at runtime.

### Provided and Required Interfaces

Components have two kinds of interface relationship:

- **Provided interfaces** — what the component offers to others.
- **Required interfaces (receptacles)** — what the component needs from others
  to function.

Explicitly declaring required interfaces makes dependencies visible and
enables third-party binding: an assembler can wire a provider's interface to
a consumer's receptacle without either party knowing the other's identity.

### Component Frameworks and Platforms

Szyperski distinguishes three layers:

1. **Component model** — the rules (interface contracts, naming, metadata,
   versioning, lifecycle protocols).
2. **Component framework** — runtime infrastructure that enforces the model
   and provides services (registration, discovery, binding, lifecycle
   management).
3. **Component platform** — the broader execution environment (OS, language
   runtime) on which frameworks operate.

A component framework typically provides:

- A **registry** for discovering and instantiating components by name.
- A **binding mechanism** that connects providers to consumers.
- **Lifecycle management** — creation, activation, deactivation, disposal.
- **Introspection** — querying a component for its interfaces and metadata
  at runtime.

### Composition and Wiring

Szyperski stresses that composition should be performed externally:

- Components should not hard-wire dependencies on specific other components.
- A third party (assembler, configuration script, or framework) resolves
  dependencies and performs the wiring.
- This is the "inversion of control" principle applied at the component level.

### Objects vs. Components

| Aspect | Object (OOP) | Component |
|---|---|---|
| Unit of | instantiation | deployment and composition |
| Identity | instance identity | interface identity |
| State | instance carries state | component is stateless template; instances carry state |
| Dependencies | often implicit (imports, inheritance) | explicitly declared (required interfaces) |
| Composition by | the programmer (hard-coded) | a third party (external wiring) |
| Reuse mechanism | inheritance | interface-based polymorphism and delegation |
| Versioning | class-level | interface-level (published interfaces are immutable) |

### Safety and the "Fragile Base Class" Problem

Szyperski argues that deep inheritance hierarchies create fragile coupling.
Components solve this by:

- Preferring **delegation** over inheritance.
- Exposing only **interfaces**, never base classes.
- Making all coupling go through contracts, never through implementation details.

### Component Lifecycle

A well-designed component model defines explicit lifecycle states:

1. **Registered** — the component is known to the registry.
2. **Instantiated** — an instance has been created.
3. **Connected** — receptacles have been wired to providers.
4. **Active** — the instance is processing work.
5. **Deactivated/Disposed** — the instance is torn down.

### Relevance to This Project

Our component framework directly implements many of Szyperski's principles:

| Szyperski Concept | Our Implementation |
|---|---|
| `IUnknown` / `QueryInterface` | `IUnknown` trait with `query_interface_raw()` and `query::<I>()` |
| Published, immutable interfaces | `define_interface!` macro; interfaces are traits with `Send + Sync + 'static` |
| Provided interfaces | `define_component! { provides: [...] }` |
| Required interfaces (receptacles) | `Receptacle<T>` and `define_component! { receptacles: { ... } }` |
| Third-party binding | `bind(provider, iface, consumer, receptacle)` free function |
| Component registry | `ComponentRegistry` with factory-based instantiation |
| Independent deployment | Each component is a separate Rust crate |
| Lifecycle management | `Actor::activate()` / `ActorHandle::deactivate()` with `on_start` / `on_stop` hooks |
| Composition over inheritance | No base classes; all coupling through interface traits |

## Component Framework

The component framework is a Rust workspace under `lib/component-framework/`
comprising three crates:

- **`component-core`** — all types, traits, and runtime machinery
- **`component-macros`** — procedural macros (`define_interface!`, `define_component!`)
- **`component-framework`** — facade crate that re-exports everything from the above two

### Component Model

The component model is based on `IUnknown`, a base trait that every component implements.
It provides runtime interface discovery, version introspection, and receptacle wiring.

| Concept | Description |
|---|---|
| `IUnknown` | Base trait. Methods: `query_interface_raw()`, `version()`, `provided_interfaces()`, `receptacles()`, `connect_receptacle_raw()`. |
| `Interface` | Marker supertrait (`Send + Sync + 'static`) for all user-defined interfaces. |
| `query::<I>(component)` | Free function that returns `Option<Arc<I>>` — the primary typed way to obtain an interface pointer. |
| `query_interface!(component, I)` | Convenience macro wrapping `query()`. |
| `ComponentRef` | `Arc<dyn IUnknown>` wrapper with `attach()` (clone) and `ref_count()`. |
| `Receptacle<T>` | Typed slot for a required interface. Thread-safe via `RwLock`. Methods: `connect()`, `disconnect()`, `get()`, `is_connected()`. |
| `InterfaceMap` | Internal `HashMap<TypeId, Box<dyn Any>>` storing `Arc<dyn IFoo>` values. Populated at construction. |

### Macros

**`define_interface!`** declares a trait with `Send + Sync + 'static` bounds:

```rust
define_interface! {
    pub IStorage {
        fn read(&self, key: &str) -> Option<Vec<u8>>;
        fn write(&self, key: &str, value: &[u8]) -> Result<(), String>;
    }
}
```

All methods must take `&self` (not `&mut self`). Generates the trait plus
`impl Interface for dyn IStorage + Send + Sync`.

**`define_component!`** generates a struct, its `IUnknown` impl, and a `new() -> Arc<Self>` constructor:

```rust
define_component! {
    pub MyComponent {
        version: "1.0.0",
        provides: [IStorage, IGreeter],
        receptacles: { logger: ILogger },   // optional
        fields: { data: HashMap<String, Vec<u8>> },  // optional
    }
}
```

### Registry and Binding

`ComponentRegistry` is a string-keyed factory registry (no global state):

| Method | Purpose |
|---|---|
| `register(name, factory)` | Register a `ComponentFactory` |
| `register_simple(name, closure)` | Convenience for no-config factories |
| `create(name, config)` | Create a `ComponentRef` via the factory |
| `list()` | List registered component names |

`bind(provider, iface_name, consumer, receptacle_name)` performs third-party wiring:
verifies `TypeId` compatibility between the provider's interface and the consumer's
receptacle, then calls `connect_receptacle_raw`.

### Actor System

Actors are thread-owning components that process messages sequentially. An actor is a
first-class component implementing `IUnknown` and exposing `ISender<M>` as its interface.

**`ActorHandler<M>`** — the trait users implement:

```rust
pub trait ActorHandler<M: Send + 'static>: Send + 'static {
    fn handle(&mut self, msg: M);   // required
    fn on_start(&mut self) {}       // optional lifecycle hook
    fn on_stop(&mut self) {}        // optional lifecycle hook
}
```

**`Actor<M, H>`** — owns a dedicated OS thread. Constructors:

| Constructor | Description |
|---|---|
| `Actor::simple(handler)` | Default capacity 1024, silent panic handler |
| `Actor::new(handler, error_callback)` | Custom panic callback |
| `Actor::with_capacity(handler, cap, cb)` | Custom capacity (power of 2) |

Builder: `.with_cpu_affinity(CpuSet)` pins the actor's thread for NUMA locality.

`actor.activate()` spawns the thread and returns an `ActorHandle<M>` with
`send()`, `try_send()`, and `deactivate()`. Dropping the handle force-closes
the channel and joins the thread.

**Pipeline helpers** connect channels to actors:

- `pipe(Receiver<M>, ActorHandle<M>) -> JoinHandle<()>` — SPSC forwarder
- `pipe_mpsc(MpscReceiver<M>, ActorHandle<M>) -> JoinHandle<()>` — MPSC forwarder

### Channel System

All channels implement `IUnknown` and expose `ISender<T>` / `IReceiver<T>` interfaces.

| Channel | Topology | Backend |
|---|---|---|
| `SpscChannel<T>` | SPSC | Native lock-free ring buffer |
| `MpscChannel<T>` | MPSC | Vyukov MPSC ring buffer (used internally by `Actor`) |
| `CrossbeamBoundedChannel<T>` | MPSC | `crossbeam_channel::bounded` |
| `CrossbeamUnboundedChannel<T>` | MPSC | `crossbeam_channel::unbounded` |
| `KanalChannel<T>` | MPSC | `kanal::bounded` |
| `RtrbChannel<T>` | SPSC | `rtrb::RingBuffer` |
| `TokioMpscChannel<T>` | MPSC | `tokio::sync::mpsc` |

Common traits:

```rust
pub trait ISender<T: Send + 'static>: Send + Sync + 'static {
    fn send(&self, value: T) -> Result<(), ChannelError>;
    fn try_send(&self, value: T) -> Result<(), ChannelError>;
}
pub trait IReceiver<T: Send + 'static>: Send + Sync + 'static {
    fn recv(&self) -> Result<T, ChannelError>;
    fn try_recv(&self) -> Result<T, ChannelError>;
}
```

### Logging

Built-in `LogHandler` implements `ActorHandler<LogMessage>`. Writes timestamped lines
(`2026-04-01T14:23:05.123Z [INFO ] message`) to stderr and optionally to a file.

- `LogLevel`: `Debug < Info < Warn < Error`
- `LogMessage`: constructed via `LogMessage::info("text")`, `.debug()`, `.warn()`, `.error()`
- `LogHandler::new()` — stderr only; `LogHandler::with_file(path)` — stderr + file
- `.with_min_level(LogLevel)` — filter messages below a threshold

### NUMA Support

Linux-only, using `libc` for syscalls.

| Type | Purpose |
|---|---|
| `CpuSet` | Wraps `libc::cpu_set_t`. Methods: `from_cpu()`, `from_cpus()`, `add()`, `contains()`, `iter()`. |
| `set_thread_affinity(cpuset)` | Calls `sched_setaffinity` on the current thread. |
| `get_thread_affinity()` | Reads current thread affinity. |
| `NumaNode` | Holds node id, `CpuSet`, and inter-node distances (from sysfs). |
| `NumaTopology::discover()` | Reads `/sys/devices/system/node/` to build the full topology. |
| `NumaAllocator` | NUMA-local memory via `mmap` + `mbind(MPOL_BIND)`. |

For NUMA-local actors: pin the constructing thread to the target node (first-touch
policy places allocations there), then call `.with_cpu_affinity(cpuset)` on the actor.

### Error Types

| Error | Variants |
|---|---|
| `ActorError` | `AlreadyActive`, `NotActive`, `SendFailed(String)`, `ShutdownTimeout`, `AffinityFailed(String)` |
| `ChannelError` | `Full`, `Empty`, `Closed`, `BindingRejected { reason }` |
| `ReceptacleError` | `NotConnected`, `AlreadyConnected` |
| `RegistryError` | `NotFound`, `AlreadyRegistered`, `FactoryFailed`, `BindingFailed` |
| `NumaError` | `CpuOutOfRange`, `CpuOffline`, `EmptyCpuSet`, `InvalidNode`, `TopologyUnavailable`, `AffinityFailed`, `AllocationFailed` |

### Prelude

`use component_framework::prelude::*` re-exports all commonly used types: `Actor`,
`ActorHandle`, `ActorHandler`, `pipe`, `pipe_mpsc`, `bind`, `MpscChannel`, `SpscChannel`,
`ISender`, `IReceiver`, `ComponentRef`, `ComponentRegistry`, `IUnknown`, `query`,
`LogHandler`, `LogLevel`, `LogMessage`, `Receptacle`, `define_component!`,
`define_interface!`, and all error types.
