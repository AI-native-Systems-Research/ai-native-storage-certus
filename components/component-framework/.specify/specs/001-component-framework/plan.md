# Implementation Plan: Component Framework

**Branch**: `001-component-framework` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

A COM-inspired Rust component framework providing runtime interface discovery, typed receptacle-based dependency injection, lock-free channel communication, an actor model with NUMA-aware thread pinning, and procedural macros for declarative component/interface definitions. Organized as a three-crate workspace (core types, proc macros, facade) targeting Linux exclusively.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `libc` 0.2 — Linux syscalls (sched_setaffinity, mmap, mbind)
- `crossbeam-channel` 0.5 — Bounded/unbounded MPMC channel backend
- `kanal` 0.1 — High-performance bounded MPMC channel backend
- `rtrb` 0.3 — Lock-free SPSC ring buffer backend
- `tokio` 1.x (sync feature) — Async-capable MPSC channel backend
- `proc-macro2`, `quote`, `syn` — Procedural macro infrastructure
- `criterion` 0.5 (dev) — Benchmarking framework

**Performance Goals**:
- Lock-free fast-path for all channel operations (send/recv)
- Progressive backoff (spin < 64, yield 64-255, park >= 256) to balance latency vs CPU usage
- Power-of-two ring buffer capacity for branchless index wrapping (bitwise AND)
- Separate cache-line-aligned head/tail atomics in SPSC to avoid false sharing
- NUMA-local memory allocation via mmap + mbind for cross-node latency minimization

## Architecture

### Component/Module Layer

```
+-------------------------------------------------------------------+
|                    component-framework (facade)                     |
|    Re-exports: component-core::* + component-macros::*             |
|    Provides: prelude, declare_interface!/declare_component! aliases |
|    Contains: 13 Criterion benchmark suites, integration tests      |
+-------------------------------------------------------------------+
         |                                    |
         v                                    v
+------------------------+     +---------------------------+
|   component-core       |     |   component-macros        |
|   (types & traits)     |     |   (proc macros)           |
|                        |     |                           |
| - iunknown (IUnknown)  |     | - define_interface!       |
| - interface (markers)  |     |   (trait + Interface impl)|
| - component (IntfMap)  |     | - define_component!       |
| - component_ref        |     |   (struct + IUnknown impl)|
| - receptacle           |     +---------------------------+
| - binding              |
| - registry             |
| - actor                |
| - channel/             |
|   - spsc (ring buffer) |
|   - mpsc (Vyukov)      |
|   - crossbeam_bounded  |
|   - crossbeam_unbounded|
|   - kanal_bounded      |
|   - rtrb_spsc          |
|   - tokio_mpsc         |
|   - queue (RingBuffer) |
| - numa/                |
|   - cpuset             |
|   - topology           |
|   - allocator          |
| - log (LogHandler)     |
| - error                |
| - prelude              |
+------------------------+
```

### Internal Module Structure

```
components/component-framework/
├── Cargo.toml                          # Workspace stub (root workspace member)
├── crates/
│   ├── component-core/
│   │   ├── Cargo.toml                  # libc, crossbeam-channel, kanal, rtrb, tokio
│   │   ├── src/
│   │   │   ├── lib.rs                  # Module declarations + pub use re-exports
│   │   │   ├── actor.rs                # Actor<M,H>, ActorHandle, ActorHandler, pipe/pipe_mpsc
│   │   │   ├── binding.rs             # bind() free function — third-party wiring by name
│   │   │   ├── channel/
│   │   │   │   ├── mod.rs             # ISender/IReceiver traits, Sender/Receiver, ChannelState
│   │   │   │   ├── spsc.rs            # SpscChannel — lock-free SPSC ring buffer component
│   │   │   │   ├── mpsc.rs            # MpscChannel — Vyukov bounded MPSC queue component
│   │   │   │   ├── queue.rs           # RingBuffer — power-of-two atomic ring buffer
│   │   │   │   ├── crossbeam_bounded.rs
│   │   │   │   ├── crossbeam_unbounded.rs
│   │   │   │   ├── kanal_bounded.rs
│   │   │   │   ├── rtrb_spsc.rs
│   │   │   │   └── tokio_mpsc.rs
│   │   │   ├── component.rs           # InterfaceMap (TypeId -> Box<dyn Any>)
│   │   │   ├── component_ref.rs       # ComponentRef (Arc<dyn IUnknown> wrapper)
│   │   │   ├── error.rs               # ReceptacleError, QueryError, RegistryError
│   │   │   ├── interface.rs           # Interface marker trait, InterfaceInfo, ReceptacleInfo
│   │   │   ├── iunknown.rs            # IUnknown trait, query() fn, query_interface! macro
│   │   │   ├── log.rs                 # LogHandler, LogLevel, LogMessage
│   │   │   ├── numa/
│   │   │   │   ├── mod.rs             # NumaError enum
│   │   │   │   ├── cpuset.rs          # CpuSet, set/get_thread_affinity, validate_cpus
│   │   │   │   ├── topology.rs        # NumaTopology, NumaNode (sysfs discovery)
│   │   │   │   └── allocator.rs       # NumaAllocator (mmap + mbind)
│   │   │   ├── prelude.rs             # Convenience re-exports
│   │   │   ├── receptacle.rs          # Receptacle<T> — typed slot with RwLock
│   │   │   └── registry.rs            # ComponentRegistry, ComponentFactory
│   │   └── tests/
│   │       └── numa_integration.rs    # NUMA integration tests
│   ├── component-framework/
│   │   ├── Cargo.toml                 # Depends on component-core + component-macros
│   │   ├── src/
│   │   │   └── lib.rs                 # Facade: re-exports, compile-fail tests, prelude, aliases
│   │   ├── benches/                   # 13 Criterion benchmark suites
│   │   │   ├── actor_latency.rs
│   │   │   ├── binding.rs
│   │   │   ├── channel_latency_benchmark.rs
│   │   │   ├── channel_mpsc_benchmark.rs
│   │   │   ├── channel_spsc_benchmark.rs
│   │   │   ├── channel_throughput.rs
│   │   │   ├── component_ref.rs
│   │   │   ├── method_dispatch.rs
│   │   │   ├── numa_latency_benchmark.rs
│   │   │   ├── numa_throughput_benchmark.rs
│   │   │   ├── query_interface.rs
│   │   │   ├── receptacle.rs
│   │   │   └── registry.rs
│   │   └── tests/                     # Integration tests
│   │       ├── actor_pipeline.rs
│   │       ├── actor.rs
│   │       ├── assembly.rs
│   │       ├── binding_enforcement.rs
│   │       ├── binding.rs
│   │       ├── channel_mpsc.rs
│   │       ├── channel_spsc.rs
│   │       ├── component_iunknown.rs
│   │       ├── component_ref.rs
│   │       ├── interface_definition.rs
│   │       ├── receptacle_wiring.rs
│   │       └── registry.rs
│   └── component-macros/
│       ├── Cargo.toml                 # proc-macro2, quote, syn
│       └── src/
│           ├── lib.rs                 # Proc macro entry points
│           ├── define_interface.rs    # Interface macro expansion logic
│           └── define_component.rs    # Component macro expansion logic
└── examples/                          # Standalone runnable examples
    ├── Cargo.toml
    ├── actor_factory.rs
    ├── actor_fan_in.rs
    ├── actor_log.rs
    ├── actor_ping_pong.rs
    ├── actor_pipeline.rs
    ├── basic.rs
    ├── binding.rs
    ├── introspection.rs
    ├── numa_pinning.rs
    ├── tokio_ping_pong.rs
    └── wiring.rs
```

### Data Flow / Key Paths

#### Interface Query Path

```
Caller                          Component (via define_component!)
  │                                 │
  ├─ query::<dyn IFoo + Send +     │
  │   Sync>(component)              │
  │                                 │
  ├─ TypeId::of::<Arc<dyn IFoo +   │
  │   Send + Sync>>()               │
  │         │                       │
  │         └───── query_interface_raw(type_id)
  │                                 │
  │                    InterfaceMap::lookup(type_id)
  │                         │
  │                         ├─ HashMap<TypeId, Box<dyn Any>>
  │                         │
  │                    downcast_ref::<Arc<dyn IFoo>>()
  │                         │
  └────── Arc::clone() ◄────┘
```

#### Third-Party Binding Path

```
bind(provider, "IFoo", consumer, "backend")
  │
  ├─ provider.provided_interfaces()
  │     └─ Find InterfaceInfo where name == "IFoo"
  │           └─ Get TypeId
  │
  ├─ consumer.receptacles()
  │     └─ Find ReceptacleInfo where name == "backend"
  │           └─ Verify TypeId matches provider's interface
  │
  └─ consumer.connect_receptacle_raw("backend", provider)
        │
        ├─ Match receptacle name in generated code
        ├─ query::<dyn IFoo + Send + Sync>(provider)
        └─ receptacle.connect(arc)
```

#### Actor Lifecycle

```
Actor::new(handler, error_cb)
  │
  ├─ MpscChannel<M>::new(capacity)  ← channel created at construction
  │     └─ ISender<M> available via IUnknown immediately
  │
  ├─ activate()
  │     ├─ Validate CPU affinity (if set)
  │     ├─ Transition state: IDLE → RUNNING
  │     ├─ Obtain receiver from channel
  │     ├─ Spawn OS thread
  │     │     ├─ Apply sched_setaffinity (if CPU affinity set)
  │     │     ├─ handler.on_start()
  │     │     └─ Message loop:
  │     │           ├─ try_recv() → handle(msg)  [with catch_unwind]
  │     │           ├─ Empty → on_idle()         [with catch_unwind]
  │     │           └─ Closed → drain & exit
  │     └─ Return ActorHandle<M>
  │
  └─ deactivate() (via ActorHandle)
        ├─ channel.close() (force_closed flag)
        ├─ Drop sender
        ├─ thread.join()
        ├─ handler.on_stop()
        └─ Transition state: RUNNING → IDLE
```

#### Channel Send/Recv (SPSC) — Lock-Free Hot Path

```
Sender::send(value)                    Receiver::recv()
  │                                       │
  ├─ queue.push(value)                    ├─ queue.pop()
  │   ├─ Load tail (Relaxed)              │   ├─ Load head (Relaxed)
  │   ├─ Check: tail - head < cap         │   ├─ Check: head != tail
  │   ├─ Write slot[tail & mask]          │   ├─ Read slot[head & mask]
  │   └─ Store tail+1 (Release)          │   └─ Store head+1 (Release)
  │                                       │
  ├─ If receiver_parked:                  ├─ If sender_parked:
  │   └─ unpark receiver thread           │   └─ unpark sender thread
  │                                       │
  └─ If full: progressive backoff         └─ If empty: progressive backoff
       spin(< 64) → yield(< 256)              spin(< 64) → yield(< 256)
       → park_timeout(50us)                   → park_timeout(50us)
```

### Key Design Decisions

1. **TypeId-based interface lookup**: Interface identity is `TypeId::of::<Arc<dyn IFoo + Send + Sync>>()` rather than string names. This provides zero-cost type-safe queries while still allowing runtime discovery. String names are used only for third-party binding metadata.

2. **Post-construction interface map initialization**: The `define_component!` macro uses an unsafe pattern — creating the Arc first, then mutating through a raw pointer to populate the InterfaceMap. This is sound because the Arc has a reference count of 1 and no other thread can observe it. The alternative (two-phase construction) would require Option-wrapping every interface field.

3. **Progressive backoff on channels**: Rather than using OS-level synchronization (futex, condvar), channels use a three-phase backoff (spin → yield → park with timeout). This minimizes latency for fast producers/consumers while bounding CPU waste when idle. The 50us park timeout prevents missed wakeups without blocking indefinitely.

4. **Separate cache-line-aligned atomics (SPSC)**: Head and tail counters in the SPSC ring buffer are on separate cache lines to prevent false sharing between producer and consumer cores — critical for cross-core throughput.

5. **Vyukov bounded queue (MPSC)**: The MPSC channel uses per-slot sequence numbers for wait-free enqueue by multiple producers. This avoids the CAS-loop contention of naive MPSC queues and provides excellent scalability with producer count.

6. **Actor owns its channel**: The MpscChannel is created at Actor construction time (not activation time). This means `ISender<M>` is queryable via IUnknown before the actor is activated, enabling wiring phases that complete before runtime activation.

7. **Force-close flag**: Actor deactivation sets `force_closed` on the channel state. This ensures the receiver exits even when ISender references obtained via IUnknown queries are still alive — without requiring all senders to be dropped.

8. **User fields before receptacles in struct layout**: The `define_component!` macro orders user fields (which may hold Actor instances) before receptacle fields. This ensures actors (and their threads) are joined during Drop before dependencies held in receptacles are released.

9. **`&self` only on interfaces (no `&mut self`)**: All interface methods use `&self` with interior mutability. This is enforced at compile time by the macro. The rationale is that interfaces are shared behind `Arc` — `&mut self` would require external synchronization that breaks the component model's thread-safety guarantees.

10. **Single-receiver enforcement on all channel types**: Both SPSC and MPSC enforce exactly one receiver at bind time (via AtomicBool flags). This prevents the complexity and potential for lost messages inherent in multi-consumer topologies.

11. **NUMA topology from sysfs**: Rather than depending on libnuma, the framework reads `/sys/devices/system/node/` and `/sys/devices/system/cpu/` directly. This eliminates a C library dependency while providing the same information for thread pinning decisions.

12. **Facade crate pattern**: `component-framework` is a zero-logic crate that re-exports `component-core::*` and `component-macros::*`. Users add a single dependency; internal refactoring between core/macros doesn't break downstream.

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| libc | 0.2 | sched_setaffinity, sched_getaffinity, mmap, mbind, CPU_SETSIZE |
| crossbeam-channel | 0.5 | Bounded/unbounded MPMC channel backend |
| kanal | 0.1 | High-performance bounded MPMC channel backend |
| rtrb | 0.3 | Lock-free SPSC ring buffer backend (alternative to built-in) |
| tokio | 1.x (sync) | Async-capable MPSC channel backend |
| proc-macro2 | - | Token stream manipulation for proc macros |
| quote | - | Rust source generation in proc macros |
| syn | - | Rust syntax parsing in proc macros |
| criterion | 0.5 (dev) | Benchmarking framework |

## Testing

### Unit Tests (in-module)
- `channel/mod.rs` — ChannelError display formatting, Send+Sync bounds
- `error.rs` — All error type Display, PartialEq, std::error::Error implementations
- `numa/mod.rs` — NumaError display formatting, equality, std::error::Error

### Integration Tests (tests/ directory)
- `actor.rs` — Actor lifecycle, send/recv, deactivation, panic recovery
- `actor_pipeline.rs` — Multi-actor message pipelines
- `assembly.rs` — Full system assembly with multiple wired components
- `binding.rs` — Third-party bind() function behavior
- `binding_enforcement.rs` — Error cases: missing interface, wrong type, already connected
- `channel_spsc.rs` — SPSC topology enforcement, closure semantics
- `channel_mpsc.rs` — MPSC multi-sender, single-receiver enforcement
- `component_iunknown.rs` — IUnknown query, version, introspection
- `component_ref.rs` — ComponentRef attach/drop/ref_count
- `interface_definition.rs` — define_interface! macro output verification
- `receptacle_wiring.rs` — Receptacle connect/disconnect/get lifecycle
- `registry.rs` — ComponentRegistry register/create/unregister

### Compile-Fail Tests (doc tests in facade lib.rs)
- Empty interface (zero methods) — rejected
- `&mut self` receiver — rejected
- Missing `version` field in component — rejected

### NUMA Integration Tests
- `tests/numa_integration.rs` — Topology discovery, CPU validation, affinity set/get

### Benchmark Suites (13 total)
- `query_interface` — Interface lookup latency
- `receptacle` — Connect/get/disconnect cycle
- `method_dispatch` — Virtual dispatch through trait object
- `registry` — Factory registration and instance creation
- `binding` — Third-party bind() overhead
- `component_ref` — Attach/clone/drop reference counting
- `channel_throughput` — Messages/sec for all channel backends
- `actor_latency` — End-to-end message processing latency
- `channel_spsc_benchmark` — SPSC specific throughput/latency
- `channel_mpsc_benchmark` — MPSC specific throughput/latency
- `channel_latency_benchmark` — Cross-channel latency comparison
- `numa_latency_benchmark` — NUMA-local vs cross-node latency
- `numa_throughput_benchmark` — NUMA-local vs cross-node throughput

## Future Considerations

1. **Dynamic loading (dlopen)**: The current framework requires all components to be compiled into the same binary. A future extension could support loading component shared libraries at runtime, similar to COM DLLs.

2. **Async actor variant**: The current actor model uses dedicated OS threads. A tokio-based async actor could reduce thread count for IO-bound workloads while sharing the same IUnknown interface model.

3. **Distributed component model**: Extending receptacles and binding to work across process boundaries (via shared memory or network) would enable distributed system composition.

4. **Interface versioning/evolution**: Currently interfaces are identified by TypeId which changes on any signature modification. A versioning scheme could support backwards-compatible interface evolution.

5. **Compile-time wiring validation**: The current string-based binding discovers errors at runtime. A proc macro could validate wiring at compile time when all component types are known.

6. **Channel back-pressure policies**: The current channels block on full. Configurable policies (drop-oldest, drop-newest, error) would support more use cases.

7. **Actor supervision trees**: Adding parent-child relationships between actors with restart policies (like Erlang/Akka) would improve fault tolerance.
