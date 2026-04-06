# Spec Drift Report

Generated: 2026-04-06T12:00:00Z
Project: component-framework

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 6 |
| Requirements Checked | 97 |
| Aligned | 83 (85.6%) |
| Drifted | 12 (12.4%) |
| Not Implemented | 2 (2.1%) |
| Unspecced Code | 0 |

## Detailed Findings

### Spec: 001-com-component-framework — COM-Style Component Framework

#### Aligned
- FR-001: `define_interface!` macro generates trait + `Interface` marker impl → `crates/component-macros/src/define_interface.rs:78-90`
- FR-002: Interface definitions usable without impl crate → `tests/interface_definition.rs:101-112`
- FR-003: IUnknown with query, version, enumerate interfaces/receptacles → `crates/component-core/src/iunknown.rs:40-132`
- FR-004: Components support 0+ interfaces and 0+ receptacles → `crates/component-macros/src/define_component.rs:75-113`
- FR-005: Receptacles connectable/disconnectable at runtime → `crates/component-core/src/receptacle.rs:84,110,141`
- FR-006: Type safety enforced at compile time (first-party) and runtime (third-party) → `crates/component-core/src/binding.rs:125`
- FR-008: Linux + stable Rust (MSRV 1.75) → `Cargo.toml:11`
- FR-009: Unconnected receptacle returns `Err(NotConnected)` → `crates/component-core/src/receptacle.rs:141-147`
- FR-010: Lifetime params in interface methods supported → `tests/interface_definition.rs:13-18`
- FR-011: `query_interface!` macro works with direct refs, Arc, ComponentRef → `crates/component-core/src/iunknown.rs:262-292`
- FR-013: `define_component!` generates `new_default()` → `crates/component-macros/src/define_component.rs:286-303`
- SC-001: Interface + component definable in ~7 lines
- SC-002: Two components composable without mutual impl dependency
- SC-003: Interface query via HashMap is O(1) → `crates/component-core/src/component.rs:31`
- SC-006: New contributor can build/test from docs

#### Drifted
- FR-007: Compile-time errors exist for bad macro usage but **no trybuild/compile-fail tests** validate them
  - Location: `crates/component-macros/src/define_interface.rs:44-68`
  - Severity: minor
- FR-012: Prelude module missing `query_interface!` and `Receptacle` from explicit re-exports
  - Location: `crates/component-core/src/prelude.rs`
  - Severity: minor
- SC-004: Most public APIs have doc tests but gaps remain (`InterfaceMap::info()`, prelude doc test is inert)
  - Severity: minor
- SC-005: Criterion benchmarks exist but no CI regression gate (`cargo bench` not in CI)
  - Severity: minor

---

### Spec: 002-registry-refcount-binding — Registry, Reference Counting, and Binding

#### Aligned
- FR-001: `ComponentRegistry` with `RwLock<HashMap<String, Box<dyn ComponentFactory>>>` → `crates/component-core/src/registry.rs:56-58`
- FR-002: `register()` with unique name enforcement → `registry.rs:93-106`
- FR-003: `create()` by name with `Option<&dyn Any>` config → `registry.rs:182-205`
- FR-004: `NotFound` error for unregistered name → `registry.rs:187-190`
- FR-005: `AlreadyRegistered` error for duplicate name → `registry.rs:99-102`
- FR-006: `list()` returns all registered names → `registry.rs:210-213`
- FR-007: `unregister()` by name → `registry.rs:157-165`
- FR-008: Thread-safe via `RwLock`; concurrent test with 10 threads → `tests/registry.rs:53-72`
- FR-009: Atomic reference counting via `Arc` → `crates/component-core/src/component_ref.rs:41-43`
- FR-010: `attach()` clones Arc → `component_ref.rs:63-67`
- FR-011: Drop semantics via Arc's Drop impl → `component_ref.rs:77-81`
- FR-012: Thread-safe refcounting (`Send + Sync`) → `component_ref.rs:99-101`
- FR-013: Compile-time use-after-free prevention via ownership → language guarantee
- FR-014: First-party binding works → `tests/binding.rs:80-90`
- FR-015: Third-party binding via `bind()` → `crates/component-core/src/binding.rs:100-138`
- FR-016: Enumerate interfaces/receptacles by name → `iunknown.rs:87,115`
- FR-017: String-to-TypeId resolution with mismatch error → `binding.rs:125-133`
- FR-019: `bind()` signature matches spec exactly → `binding.rs:100-105`

#### Drifted
- FR-018: Spec says "initial reference count of 1" but macro-generated components have higher count due to internal self-referential Arcs in the interface map
  - Location: `tests/component_ref.rs:129-146` (tests use `base_count` workaround)
  - Severity: moderate
- FR-020: `register_simple` is implemented (`registry.rs:147-152`) with doc-test, but has **no integration test** in `tests/registry.rs`
  - Severity: minor

---

### Spec: 003-actor-channels — Actor Model with Channel Components

#### Aligned
- FR-001: Actor owns dedicated thread → `crates/component-core/src/actor.rs:612`
- FR-003: Sequential message processing → `actor.rs:624-637`
- FR-004: activate/deactivate with error on double-activate; deactivate consumes self → `actor.rs:576-589`, `actor.rs:206`
- FR-005: Actor discoverable via IUnknown introspection → `actor.rs:711-731`
- FR-006: Panic caught via `catch_unwind`, callback invoked, actor continues → `actor.rs:627-633`
- FR-007: Channels are first-class IUnknown components → `channel/spsc.rs:239`, `channel/mpsc.rs:498`
- FR-008: SPSC channel with lock-free ring buffer → `channel/spsc.rs:51`
- FR-009: MPSC channel with Vyukov queue → `channel/mpsc.rs:53`
- FR-010: Lock-free queues (atomic head/tail, per-slot sequences) → `channel/queue.rs`
- FR-011: Typed messages (`T: Send + 'static`) → `channel/spsc.rs:51`, `channel/mpsc.rs:53`
- FR-012: Closure signal when all senders disconnect → `channel/mod.rs:271-293`, `channel/mpsc.rs:150-166`
- FR-013: SPSC rejects second sender → `channel/spsc.rs:190-205`
- FR-014: SPSC rejects second receiver → `channel/spsc.rs:221-236`
- FR-015: MPSC accepts multiple senders → `channel/mpsc.rs:459-464`
- FR-016: MPSC rejects second receiver → `channel/mpsc.rs:480-494`
- FR-017: Sender disconnect frees slot for rebind → `channel/mod.rs:288-292`
- FR-020: Configurable channel capacity → `channel/spsc.rs:77`, `actor.rs:440`
- FR-021: Ping-pong example → `examples/actor_ping_pong.rs`
- FR-022: Pipeline example → `examples/actor_pipeline.rs`
- FR-023: Fan-in MPSC example → `examples/actor_fan_in.rs`
- FR-024: Tokio ping-pong example → `examples/tokio_ping_pong.rs`
- FR-025: `pipe()` and `pipe_mpsc()` helpers → `actor.rs:773-785`, `actor.rs:823-835`
- FR-026: `Actor::simple()` with default capacity 1024 → `actor.rs:395-397`
- FR-027: Channel `split()` method → `channel/spsc.rs:169-173`, `channel/mpsc.rs:424-428`

#### Drifted
- FR-002: Spec says actors must "use the same component, interface, and receptacle model (defined via the existing macro system)." `Actor<M,H>` is hand-coded with manual `IUnknown` impl, bypassing `define_component!`
  - Location: `actor.rs:348-366` (struct), `actor.rs:689-731` (manual IUnknown)
  - Severity: moderate
- FR-018: Channel components never registered as named factories in registry; only actor registration tested
  - Location: `tests/actor.rs:274-321` (actors only)
  - Severity: minor
- FR-019: Third-party binding (`bind()` by string names) never tested or demonstrated for actor-to-channel wiring; all examples use first-party binding only
  - Location: No actor/channel third-party binding test exists
  - Severity: moderate

---

### Spec: 004-channel-benchmarks — Channel Backend Benchmarks

#### Aligned
- FR-001: Crossbeam bounded + unbounded channels → `channel/crossbeam_bounded.rs:128`, `channel/crossbeam_unbounded.rs:115`
- FR-002: Kanal bounded channel → `channel/kanal_bounded.rs:117`
- FR-003: rtrb SPSC ring buffer → `channel/rtrb_spsc.rs:139`
- FR-004: Tokio MPSC channel → `channel/tokio_mpsc.rs:120`
- FR-005: All backends implement IUnknown with ISender/IReceiver
- FR-006: Binding constraints enforced per topology (CAS flags)
- FR-007: Introspection (provided_interfaces) on all backends
- FR-008: Throughput benchmarks → `benches/channel_throughput.rs:14-77`
- FR-009: Latency benchmarks → `benches/channel_latency_benchmark.rs:29-135`
- FR-010: SPSC benchmark groups → `benches/channel_spsc_benchmark.rs:63-201`
- FR-011: MPSC groups with 2, 4, 8 producers → `benches/channel_mpsc_benchmark.rs:84-181`
- FR-012: Small (u64) and large (Vec<u8> 1024B) message sizes
- FR-013: Queue capacities 64, 1024, 16384
- FR-014: Integration tests → `tests/channel_spsc.rs`, `tests/channel_mpsc.rs`
- FR-015: Doc tests on all public channel types
- FR-016: Comparable results via Criterion groups with consistent labels

#### Drifted
(None)

#### Not Implemented
(None)

---

### Spec: 005-numa-aware-actors — NUMA-Aware Actor Thread Pinning and Memory Allocation

#### Aligned
- FR-001: `CpuSet` type with full API → `numa/cpuset.rs:63-253`
- FR-002: Thread pinned before message loop → `actor.rs:612-619`
- FR-003: No affinity = backward compatible → `actor.rs:614`, `numa_integration.rs:81-106`
- FR-005: OS error propagated as `ActorError::AffinityFailed` → `cpuset.rs:300-316`, `actor.rs:615-617`
- FR-006: Empty CPU set rejected with `NumaError::EmptyCpuSet` → `cpuset.rs:300-303`
- FR-007: NUMA topology discovery via sysfs → `numa/topology.rs:135-152`
- FR-008: All CPUs in exactly one node → `topology.rs:384-396`
- FR-009: Fallback to single node when NUMA unavailable → `topology.rs:136-151`
- FR-010: Same-node latency benchmark → `benches/numa_latency_benchmark.rs:41-83`
- FR-011: Cross-node latency benchmark → `benches/numa_latency_benchmark.rs:132-230`
- FR-012: Throughput benchmark (same/cross node) → `benches/numa_throughput_benchmark.rs:22-198`
- FR-013: Benchmark results labeled with NUMA config
- FR-014: NUMA pinning example → `examples/numa_pinning.rs:101-199`
- FR-015: `NumaAllocator` with mmap + mbind → `numa/allocator.rs:80-139`
- FR-018: Default allocation uses system default
- FR-019: mbind failure ignored (fallback to default) → `allocator.rs:119-129`
- FR-020: Benchmarks compare NUMA-local vs default channels

#### Drifted
- FR-004: `validate_cpus()` function exists (`cpuset.rs:355`) but is **not called** in `Actor::activate()` — invalid CPUs produce generic kernel errors rather than structured `CpuOffline(id)` errors
  - Location: `actor.rs:614-616`
  - Severity: moderate
- FR-016: `SpscChannel::new_numa` and `MpscChannel::new_numa` accept a node parameter but **discard it**, delegating to `Self::new(capacity)`. `NumaAllocator` is not used for ring buffer allocation
  - Location: `channel/spsc.rs:145-147`, `channel/mpsc.rs:400-402`
  - Severity: moderate

#### Not Implemented
- FR-017: No framework integration point for NUMA-local handler state allocation. `NumaAllocator` exists but no builder/helper connects it to actor handler construction
  - Severity: moderate

---

### Spec: 006-log-handler — Generic Log Handler

#### Aligned
- FR-001: `LogLevel` enum with `Debug < Info < Warn < Error` ordering → `log.rs:40-49`
- FR-002: `LogMessage` with convenience constructors → `log.rs:77-156`
- FR-003: `LogHandler` implements `ActorHandler<LogMessage>` → `log.rs:291-315`
- FR-004: Optional file output via `with_file(path)` → `log.rs:229-235`
- FR-005: Minimum level filtering via `with_min_level` → `log.rs:247-250`
- FR-006: File buffers flushed on `on_stop` → `log.rs:309-314`
- FR-007: ISO-8601 timestamp + 5-char padded level tag → `log.rs:260-298`
- FR-008: Timestamp from `SystemTime` (no external deps) → `log.rs:261,297`

#### Drifted
(None)

#### Not Implemented
(None)

---

## Unspecced Code

No significant unspecced features detected. All source modules map to one of the six specs.

## Inter-Spec Conflicts

None detected. The specs build on each other in a clear dependency chain (001 → 002 → 003 → 004/005/006) with no contradictions.

## Recommendations

1. **Add trybuild compile-fail tests** (001-FR-007, SC-004): Validate that incorrect macro usage produces the expected compile-time errors. Without these, a macro regression could silently accept invalid input.

2. **Update spec 002-FR-018 or fix implementation**: Either reword the spec to say "caller holds exactly one strong reference" (describing observable semantics) or refactor `define_component!` to avoid internal self-referential Arcs that inflate the initial count.

3. **Wire `validate_cpus()` into `Actor::activate()`** (005-FR-004): The validation function exists but isn't called. Actors with offline/invalid CPUs get generic kernel errors instead of the structured `CpuOffline(id)` error the framework provides.

4. **Implement NUMA-local channel buffer allocation** (005-FR-016): `new_numa()` constructors currently discard the node parameter. Either use `NumaAllocator` for the ring buffer backing store or update the spec to reflect the first-touch-only approach.

5. **Add framework integration for NUMA-local handler state** (005-FR-017): Provide a builder method or helper that connects `NumaAllocator` to actor handler construction.

6. **Add third-party binding tests for actors/channels** (003-FR-019): The string-name `bind()` path is never exercised for actor-to-channel wiring. Add integration tests demonstrating registry-based assembly of actor pipelines.

7. **Add channel registry factory tests** (003-FR-018): Channel components implement `IUnknown` but are never registered as named factories. Add tests showing channel creation via the registry.

8. **Complete prelude re-exports** (001-FR-012): Add `Receptacle` and ensure `query_interface!` is documented as available via the prelude.

9. **Add benchmark regression gate to CI** (001-SC-005): Consider running `cargo bench` in CI and storing baseline results for comparison.
