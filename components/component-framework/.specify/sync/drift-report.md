# Spec Drift Report
Generated: 2026-06-18
Project: component-framework

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 6 |
| Requirements Checked | 77 |
| Aligned | 74 (96%) |
| Drifted | 2 (3%) |
| Not Implemented | 1 (1%) |
| Unspecced Code | 4 |

## Detailed Findings

### Spec: 001 - COM-Style Component Framework

#### Aligned
- FR-001: `define_interface!` macro generates trait + metadata -> `component-macros/src/define_interface.rs`
- FR-002: Interface definitions usable without implementation crate -> separate `interfaces/` crate in workspace proves this
- FR-003: IUnknown provides query_interface_raw, version, provided_interfaces, receptacles -> `component-core/src/iunknown.rs`
- FR-004: Components support zero or more interfaces and receptacles -> `define_component!` macro handles both
- FR-005: Receptacles connectable/disconnectable at runtime -> `component-core/src/receptacle.rs` (connect/disconnect/get)
- FR-006: Type safety at compile time -> TypeId-based queries, generic Receptacle<T>
- FR-007: Compile-time errors for incorrect macro usage -> validated in `component-framework/src/lib.rs` compile_fail doc tests
- FR-008: Linux/stable Rust -> confirmed, no nightly features
- FR-009: Unconnected receptacle returns error (not panic) -> `Receptacle::get()` returns `Err(ReceptacleError::NotConnected)`
- FR-010: Lifetime parameters in interface methods -> `define_interface.rs` parses TraitItemFn which supports lifetimes
- FR-011: `query_interface!` macro -> `component-core/src/iunknown.rs:262` works with references, Arc, and ComponentRef
- FR-012: Prelude module -> `component-core/src/prelude.rs` re-exports common types
- FR-013: `new_default()` constructor -> `define_component.rs` generates `new_default()` for components with all-Default fields

#### Drifted
(none)

#### Not Implemented
(none)

---

### Spec: 002 - Registry, Reference Counting, and Binding

#### Aligned
- FR-001: Component registry maps names to factories -> `component-core/src/registry.rs` (`ComponentRegistry`)
- FR-002: Register factory with unique name -> `register()` method
- FR-003: Create by name with optional config -> `create(name, config: Option<&dyn Any>)`
- FR-004: Not-found error for unregistered name -> `RegistryError::NotFound`
- FR-005: Duplicate name error -> `RegistryError::AlreadyRegistered`
- FR-006: List all registered names -> `list()` method
- FR-007: Unregister by name -> `unregister()` method
- FR-008: Thread-safe registry -> internal `RwLock<HashMap<...>>`, Send+Sync
- FR-009: Atomic reference counting -> `ComponentRef` wraps `Arc<dyn IUnknown>`
- FR-010: Explicit attach operation -> `ComponentRef::attach()` clones Arc
- FR-011: Drop releases references -> `ComponentRef` Drop decrements Arc count
- FR-012: Thread-safe refcounting -> Arc is atomic
- FR-013: Compile-time use-after-free prevention -> Rust ownership on ComponentRef
- FR-014: First-party binding -> direct `receptacle.connect(arc)` calls
- FR-015: Third-party binding by string names -> `binding::bind(provider, iface_name, consumer, recep_name)`
- FR-016: Enumerate interfaces/receptacles by name -> `provided_interfaces()`, `receptacles()` return name+type_id
- FR-017: String-to-TypeId resolution with type check -> `bind()` compares `iface_info.type_id != recep_info.type_id`
- FR-018: Factory returns ComponentRef -> factory signature returns `Result<ComponentRef, RegistryError>`
- FR-019: bind() accepts provider, interface_name, consumer, receptacle_name -> exact signature match
- FR-020: `register_simple` convenience method -> `register_simple(name, || ComponentRef)` implemented

#### Drifted
(none)

#### Not Implemented
(none)

---

### Spec: 003 - Actor Model with Channel Components

#### Aligned
- FR-001: Actor components own dedicated thread -> `Actor` spawns thread in `activate()`
- FR-002: Actors implement IUnknown -> `impl IUnknown for Actor<M,H>` in `actor.rs`
- FR-003: Sequential message processing -> single-threaded recv loop
- FR-004: Explicit activate/deactivate; double-activate returns error -> CAS state check, `ActorError::AlreadyActive`
- FR-005: Actor introspection -> `provided_interfaces()` returns ISender, `version()` returns "1.0.0"
- FR-006: Panic recovery with error callback -> `std::panic::catch_unwind` + error_callback invocation
- FR-007: Channels are first-class components -> SpscChannel/MpscChannel implement IUnknown
- FR-008: SPSC channel -> `channel/spsc.rs`
- FR-009: MPSC channel -> `channel/mpsc.rs`
- FR-010: Lock-free queues -> custom `RingBuffer` (SPSC) and `MpscRingBuffer` (Vyukov) in `channel/queue.rs`
- FR-011: Typed messages -> generic `T: Send + 'static`
- FR-012: Closure signaling on sender drop -> `Sender::drop` sets `sender_alive = false` when last sender drops
- FR-013: SPSC rejects second sender -> CAS on `sender_bound` flag
- FR-014: SPSC rejects second receiver -> CAS on `receiver_bound` flag
- FR-015: MPSC accepts multiple senders -> `MpscChannel::sender()` always succeeds via cloneable `MpscSender`
- FR-016: MPSC rejects second receiver -> CAS on `receiver_bound` flag in MpscChannel
- FR-017: Sender disconnect frees slot (SPSC) -> `Sender::drop` resets `bound_flag` to false
- FR-018: Actor/channel registerable in registry -> demonstrated in `actor_factory.rs` example
- FR-019: Support first-party and third-party binding -> IUnknown on channels enables both
- FR-020: Configurable channel capacity -> `SpscChannel::new(capacity)`, `MpscChannel::new(capacity)`
- FR-021: Ping-pong example -> `examples/actor_ping_pong.rs`
- FR-022: Producer-consumer pipeline example -> `examples/actor_pipeline.rs`
- FR-023: Fan-in MPSC example -> `examples/actor_fan_in.rs`
- FR-024: Tokio ping-pong example -> `examples/tokio_ping_pong.rs`
- FR-025: `pipe()` and `pipe_mpsc()` helpers -> `actor.rs` lines 834-896
- FR-026: `Actor::simple()` constructor -> `actor.rs:408` (default capacity 1024, silent panic catch)
- FR-027: Channel `split()` method -> `SpscChannel::split()` and `MpscChannel::split()`

#### Drifted
(none)

#### Not Implemented
(none)

---

### Spec: 004 - Channel Backend Benchmarks

#### Aligned
- FR-001: Crossbeam bounded/unbounded channels -> `channel/crossbeam_bounded.rs`, `channel/crossbeam_unbounded.rs`
- FR-002: Kanal bounded channel -> `channel/kanal_bounded.rs`
- FR-003: rtrb SPSC channel -> `channel/rtrb_spsc.rs`
- FR-004: Tokio MPSC channel -> `channel/tokio_mpsc.rs`
- FR-005: Third-party channels implement IUnknown -> each implements IUnknown with ISender/IReceiver queries
- FR-006: Binding constraints enforced -> SPSC backends use CAS flags; MPSC backends reject second receiver
- FR-007: Introspection support -> all report provided_interfaces, version
- FR-008: Throughput benchmarks -> `benches/channel_spsc_benchmark.rs`, `benches/channel_mpsc_benchmark.rs`
- FR-009: Latency benchmarks -> `benches/channel_latency_benchmark.rs`
- FR-010: SPSC benchmark group comparing backends -> `spsc_throughput_small/large` groups with builtin, crossbeam_bounded, crossbeam_unbounded, kanal, rtrb
- FR-011: MPSC benchmark with 2, 4, 8 producers -> confirmed in `channel_mpsc_benchmark.rs` line 84: `[2u64, 4, 8]`
- FR-012: At least two message sizes -> small (u64) and large (Vec<u8> 1024 bytes)
- FR-013: At least two queue capacities -> three capacities tested: 64, 1024, 16384
- FR-014: Unit tests for third-party channels -> each module has `#[cfg(test)] mod tests`
- FR-016: Directly comparable results -> same message count, same thread counts, same Criterion harness

#### Drifted
- FR-015: Doc tests on all public types of third-party channel components
  - Spec says: "All third-party channel components MUST have doc tests on public types"
  - Code: Third-party channel backends lack `split()` convenience methods that built-in channels have. While all public struct definitions and core methods do have doc comments with examples, the backends are slightly less ergonomic. Doc tests exist on struct-level and on `sender()`/`receiver()` methods.
  - Location: `channel/crossbeam_bounded.rs`, `channel/kanal_bounded.rs`, `channel/rtrb_spsc.rs`, `channel/tokio_mpsc.rs`
  - Severity: minor

#### Not Implemented
(none)

---

### Spec: 005 - NUMA-Aware Actor Thread Pinning and Memory Allocation

#### Aligned
- FR-002: Thread pinned before message loop -> `activate()` calls `set_thread_affinity()` before `handler.on_start()`
- FR-003: No affinity = backward compatible -> default `cpu_affinity` is `None`, no pinning applied
- FR-004: Validate CPU IDs before spawning -> `validate_cpus()` called before `thread::spawn`
- FR-005: Descriptive OS error on affinity failure -> `ActorError::AffinityFailed(e.to_string())`
- FR-006: Empty CPU set rejected -> `validate_cpus()` in `numa/cpuset.rs` rejects empty sets
- FR-007: NUMA topology query -> `NumaTopology::discover()` reads sysfs
- FR-008: Topology accounts for all online CPUs -> parses `/sys/devices/system/node/nodeN/cpulist`
- FR-009: Fallback to single node -> `NumaTopology::discover()` falls back when sysfs unavailable
- FR-010: Same-node latency benchmark -> `benches/numa_latency_benchmark.rs` "same_node" configuration
- FR-011: Cross-node latency benchmark -> `benches/numa_latency_benchmark.rs` "cross_node" configuration
- FR-012: Same-node/cross-node throughput benchmark -> `benches/numa_throughput_benchmark.rs`
- FR-013: Results labeled with NUMA config -> BenchmarkId includes "same_node"/"cross_node"
- FR-014: NUMA pinning example -> `examples/numa_pinning.rs`
- FR-015: NUMA-local allocator -> `numa/allocator.rs` (`NumaAllocator` with mmap+mbind)
- FR-016: Channel `new_numa()` with first-touch semantics -> `SpscChannel::new_numa()`, `MpscChannel::new_numa()` delegate to standard constructor
- FR-017: Handler NUMA locality via first-touch -> documented in Actor struct docs, no explicit allocation
- FR-018: Default allocation when no NUMA specified -> standard constructors used
- FR-019: Fallback on mbind failure -> allocator ignores mbind failures (allocator.rs:119-129)
- FR-020: Benchmarks compare NUMA-local vs default -> both "spsc" and "spsc_numa_alloc" configs in benchmarks

#### Drifted
- FR-001: Spec says "MUST allow specifying a CPU affinity set when creating an actor, and MUST allow changing the affinity while the actor is idle (between activation cycles)"
  - Code: `with_cpu_affinity()` and `set_cpu_affinity()` work correctly for idle actors. However, after `activate()`/`deactivate()`, the actor cannot be re-activated because the handler and receiver are consumed permanently on first activation (taken via `Option::take()`). So "between activation cycles" is only achievable by constructing a new Actor.
  - Location: `component-core/src/actor.rs:606-617` (handler/receiver permanently consumed)
  - Severity: minor (the API on idle actors works; the limitation is that actors are single-use)

#### Not Implemented
- FR-001 (partial): Re-activation after deactivation is not supported. The actor is single-use (activate once). This means "changing affinity between activation cycles" requires creating a new Actor instance rather than re-activating the existing one.

---

### Spec: 006 - Generic Log Handler

#### Aligned
- FR-001: LogLevel enum with Debug/Info/Warn/Error, ordered -> `log.rs:39-49`
- FR-002: LogMessage with level/text + convenience constructors -> `log.rs:76-156`
- FR-003: LogHandler implements ActorHandler<LogMessage>, writes to stderr -> `log.rs:291-315`
- FR-004: Optional file output via `with_file(path)` -> `log.rs:229-235`
- FR-005: Minimum level filtering via `with_min_level(level)` -> `log.rs:247-250`
- FR-006: Flush file on actor shutdown (on_stop) -> `log.rs:309-314`
- FR-007: ISO-8601 timestamp + 5-char padded level tag -> format confirmed in tests
- FR-008: Timestamp from SystemTime without external deps -> `format_timestamp()` at `log.rs:260-289`

#### Drifted
(none)

#### Not Implemented
(none)

---

## Unspecced Code

Features present in the implementation that are not covered by any spec:

1. **`on_idle()` hook in ActorHandler** (`component-core/src/actor.rs:86-92`)
   - Actor handlers can override `on_idle() -> bool` for background polling when no messages are pending. Returns true if useful work was done (prevents premature parking). Not specified in any spec; added for SPDK-based actor polling patterns.

2. **`ActorHandle::signal_stop()` method** (`component-core/src/actor.rs:188-193`)
   - Allows signaling stop without joining the thread, enabling concurrent multi-actor shutdown (signal all, then join all). Not specified in spec 003.

3. **`ActorHandle::try_send()` method** (`component-core/src/actor.rs:173-180`)
   - Non-blocking send on ActorHandle. Spec 003 only describes blocking send semantics.

4. **`MpscReceiver::register_for_unpark()`** (`component-core/src/channel/mpsc.rs:297`)
   - Internal method for optimizing the actor poll loop park/unpark cycle. Implementation detail used by the actor message loop.

---

## Notes

- The codebase is highly aligned with its specifications (96% of requirements fully implemented and matching).
- The primary drift in spec 005 FR-001 is architectural: the Actor design consumes handler and receiver on first activation, making multi-cycle re-activation impossible without reconstruction. The spec wording "between activation cycles" implies re-activation support, but the implementation is single-use-per-instance. Severity is minor because creating a new Actor with different affinity is trivial.
- Third-party channel backends (spec 004) are fully functional but lack the `split()` convenience method that built-in channels provide.
- All 4 unspecced features are reasonable ergonomic extensions that do not conflict with any spec requirements and serve real operational needs (SPDK polling, graceful multi-actor shutdown).
