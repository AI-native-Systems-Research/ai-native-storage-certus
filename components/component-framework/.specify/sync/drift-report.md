# Spec Drift Report
Generated: 2026-07-22
Project: component-framework
Status: **Aligned** — no code changes since prior analysis (2026-06-30); findings reconfirmed by independent re-read of specs and source.

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

### Spec: 001 - COM-Style Component Framework (`specs/001-com-component-framework`)

#### Aligned
- FR-001: `define_interface!` macro generates trait + metadata -> `crates/component-macros/src/define_interface.rs`
- FR-002: Interface definitions usable without implementation crate -> separate `interfaces/` workspace crate depends only on interface definitions
- FR-003: `IUnknown` provides query, version, provided_interfaces, receptacles -> `crates/component-core/src/iunknown.rs:40-131`
- FR-004: Components support zero or more interfaces/receptacles -> `crates/component-macros/src/define_component.rs`
- FR-005: Receptacles connectable/disconnectable at runtime -> `crates/component-core/src/receptacle.rs:84-117` (`connect`/`disconnect`)
- FR-006: Compile-time type safety -> `TypeId`-based queries, generic `Receptacle<T>`
- FR-007: Compile-time errors for incorrect macro usage -> relies on standard Rust diagnostics from macro-expanded code (per spec's own relaxed wording)
- FR-008: Linux/stable Rust -> confirmed, no nightly features used anywhere in the crate
- FR-009: Unconnected receptacle returns error, not panic -> `receptacle.rs:141-147` (`Receptacle::get()` returns `Err(ReceptacleError::NotConnected)`)
- FR-010: Lifetime parameters in interface methods -> `define_interface.rs` parses `TraitItemFn`, which supports lifetimes unmodified
- FR-011: `query_interface!` macro works with direct refs, `Arc<T>`, `ComponentRef` -> `iunknown.rs:262-292` (`AsIUnknown` impls for `T`, `Arc<T>`, `ComponentRef`)
- FR-012: Prelude module -> `crates/component-core/src/prelude.rs`
- FR-013: `new_default()` constructor for all-`Default` fields -> `define_component.rs:284-297`

#### Drifted
(none)

#### Not Implemented
(none)

---

### Spec: 002 - Registry, Reference Counting, and Binding (`specs/002-registry-refcount-binding`)

#### Aligned
- FR-001..FR-007: Registry maps names to factories; register/create/list/unregister with not-found and duplicate errors -> `crates/component-core/src/registry.rs:56-214`
- FR-008: Thread-safe registry -> internal `RwLock<HashMap<...>>`
- FR-009..FR-013: Atomic refcounting via `ComponentRef` wrapping `Arc`; attach/drop semantics; compile-time UAF prevention -> `crates/component-core/src/component_ref.rs`
- FR-014: First-party binding -> direct `receptacle.connect(arc)`
- FR-015..FR-017: Third-party binding by string name, TypeId resolution, type-mismatch error -> `crates/component-core/src/binding.rs:100-138`
- FR-018: Factory returns `ComponentRef` -> `registry.rs:29-32` (`ComponentFactory::create`)
- FR-019: `bind()` signature (provider, iface_name, consumer, recep_name) -> exact match, `binding.rs:100-105`
- FR-020: `register_simple` -> `registry.rs:147-152`

#### Drifted
(none)

#### Not Implemented
(none)

---

### Spec: 003 - Actor Model with Channel Components (`specs/003-actor-channels`)

#### Aligned
- FR-001..FR-006: Actor owns dedicated thread, implements `IUnknown`, sequential processing, activate/deactivate with `AlreadyActive` error, introspection, panic recovery via `catch_unwind` + error callback -> `crates/component-core/src/actor.rs:589-680`
- FR-007..FR-012: Channels as first-class components (SPSC/MPSC), lock-free ring buffers, typed messages, sender-drop closure signal -> `crates/component-core/src/channel/{spsc,mpsc,queue}.rs`
- FR-013..FR-017: SPSC/MPSC binding enforcement (CAS on bound flags), slot freed on sender disconnect -> `channel/spsc.rs`, `channel/mpsc.rs`
- FR-018..FR-020: Registry integration, first/third-party binding support, configurable capacity -> `Actor`/`SpscChannel`/`MpscChannel` all implement `IUnknown`; `examples/actor_factory.rs`
- FR-021..FR-024: Ping-pong, producer-consumer, fan-in, tokio ping-pong examples -> `examples/actor_ping_pong.rs`, `actor_pipeline.rs`, `actor_fan_in.rs`, `tokio_ping_pong.rs`
- FR-025: `pipe()`/`pipe_mpsc()` forwarder helpers -> `actor.rs` (forwarder thread bridging channel Receiver to ActorHandle)
- FR-026: `Actor::simple()` -> `actor.rs` (default capacity 1024, silent panic catch)
- FR-027: Channel `split()` -> `SpscChannel::split()`, `MpscChannel::split()`

#### Drifted
(none)

#### Not Implemented
(none)

---

### Spec: 004 - Channel Backend Benchmarks (`specs/004-channel-benchmarks`)

#### Aligned
- FR-001..FR-004: Crossbeam bounded/unbounded, kanal, rtrb (SPSC-only), tokio MPSC channel components -> `channel/crossbeam_bounded.rs`, `channel/crossbeam_unbounded.rs`, `channel/kanal_bounded.rs`, `channel/rtrb_spsc.rs`, `channel/tokio_mpsc.rs`
- FR-005..FR-007: Each implements `IUnknown` with ISender/IReceiver, binding enforcement, introspection -> confirmed in each module
- FR-008..FR-013: Throughput/latency benchmarks, SPSC/MPSC groups (2/4/8 producers), two message sizes, multiple queue capacities (64/1024/16384) -> `crates/component-framework/benches/channel_{spsc,mpsc}_benchmark.rs`, `channel_latency_benchmark.rs`
- FR-014: Unit tests per backend -> each channel module has `#[cfg(test)] mod tests`
- FR-016: Comparable results, `{topology}_throughput_{type}/{backend}/{capacity}` group IDs -> matches benchmark code

#### Drifted
- **FR-015**: Doc tests on all public types of third-party channel components.
  - **spec_text**: "All third-party channel components MUST have documentation examples on public types."
  - **actual**: Public structs and `sender()`/`receiver()` methods have doc examples, but the backends intentionally expose native construction APIs rather than the `split()` convenience method that built-in channels provide (per the spec's own qualifying clause), so example code is less uniform/ergonomic across backends than for built-in channels.
  - **location**: `crates/component-core/src/channel/crossbeam_bounded.rs`, `kanal_bounded.rs`, `rtrb_spsc.rs`, `tokio_mpsc.rs`
  - **severity**: minor

#### Not Implemented
(none)

---

### Spec: 005 - NUMA-Aware Actor Thread Pinning and Memory Allocation (`specs/005-numa-aware-actors`)

#### Aligned
- FR-002..FR-006: Thread pinned before message loop, backward-compatible when unset, CPU IDs validated before spawn, descriptive OS error on affinity failure, empty set rejected -> `actor.rs:589-639` (`activate()`), `crates/component-core/src/numa/cpuset.rs`
- FR-007..FR-009: NUMA topology discovery via sysfs, accounts for all online CPUs, falls back to single node -> `crates/component-core/src/numa/topology.rs`
- FR-010..FR-013: Same-node/cross-node latency and throughput benchmarks, labeled by config -> `crates/component-framework/benches/numa_latency_benchmark.rs`, `numa_throughput_benchmark.rs`
- FR-014: NUMA pinning example -> `examples/numa_pinning.rs`
- FR-015: NUMA-local allocator (`mmap` + `mbind`) -> `crates/component-core/src/numa/allocator.rs:80-140`
- FR-016..FR-018: Channel `new_numa()` delegates to standard constructor relying on first-touch; handler NUMA locality documented, not explicitly allocated; default policy when unset
- FR-019: Allocator ignores `mbind` failure and falls back to default policy -> `numa/allocator.rs:118-130`
- FR-020: Benchmarks compare NUMA-local vs default allocation -> both configs present in NUMA benchmarks

#### Drifted
- **FR-001**: Spec text implies CPU affinity is "configurable... between activation cycles" (i.e., an actor can be reactivated with different affinity).
  - **spec_text**: "CPU affinity is configurable before activation via `set_cpu_affinity()`. Actors are single-use; re-activation requires constructing a new Actor instance." (spec's own clarifying note softens the User Story wording, but the User Story text and Edge Cases section still frame this as a multi-cycle capability)
  - **actual**: `activate()` permanently consumes `handler` and `receiver` via `Option::take()`. A second `activate()` call after `deactivate()` panics with `.expect("handler already taken — actor activated twice without reset")` rather than returning a typed error, and there is no supported re-activation path — a new `Actor` must be constructed.
  - **location**: `crates/component-core/src/actor.rs:604-617`
  - **severity**: minor (spec's own FR-001 text already documents this constraint; the drift is between the earlier User-Story framing and the FR/implementation, not a code defect)

#### Not Implemented
- **FR-001 (partial)**: True re-activation of a deactivated `Actor` with new affinity is not supported; only single-use construction with pre-activation configuration is implemented. Callers wanting a "new activation cycle" must construct a new `Actor`.

---

### Spec: 006 - Generic Log Handler (`specs/006-log-handler`)

#### Aligned
- FR-001: `LogLevel` enum, ordered `Debug < Info < Warn < Error` -> `crates/component-core/src/log.rs:39-49`
- FR-002: `LogMessage` with level/text + constructors -> `log.rs:76-156`
- FR-003: `LogHandler: ActorHandler<LogMessage>`, writes to stderr -> `log.rs:291-307`
- FR-004: Optional file output via `with_file()` (append mode) -> `log.rs:229-235`
- FR-005: Minimum level filtering via `with_min_level()` -> `log.rs:247-250`, enforced at `log.rs:293-295`
- FR-006: Flush file buffer on `on_stop()` -> `log.rs:309-314`
- FR-007: ISO-8601 timestamp + 5-char padded level tag -> `format_timestamp()` (`log.rs:260-289`) + `Display` impl (`log.rs:51-60`); verified by `format_timestamp_produces_valid_output` test
- FR-008: Timestamp from `SystemTime`, no external deps -> `log.rs:260-289` uses only `std::time`

#### Drifted
(none)

#### Not Implemented
(none)

---

## Unspecced Code

Features present in the implementation that are not covered by any spec:

| # | Feature | Location | Notes |
|---|---------|----------|-------|
| 1 | `on_idle()` hook on `ActorHandler` | `crates/component-core/src/actor.rs:90` | Optional override for background polling when no messages are pending; returns `true` if useful work was done (prevents premature parking). Used for SPDK-style polling actors. Not mentioned in spec 003. |
| 2 | `ActorHandle::signal_stop()` | `crates/component-core/src/actor.rs:190` | Signals stop without joining the thread, enabling concurrent multi-actor shutdown (signal-all-then-join-all pattern). Spec 003 only describes `deactivate()` (blocking join). |
| 3 | `ActorHandle::try_send()` / `MpscSender::try_send()` | `crates/component-core/src/actor.rs:173`, `crates/component-core/src/channel/mpsc.rs:114` | Non-blocking send variants. Spec 003's Assumptions section states the default behavior is that "the sender blocks until space is available"; the non-blocking alternative is undocumented. |
| 4 | `MpscReceiver::register_for_unpark()` | `crates/component-core/src/channel/mpsc.rs:297` | Internal-facing API used by the actor poll loop to optimize park/unpark cycling under idle load (`actor.rs:679`). Implementation detail, not part of any public user-facing contract in spec 003. |

## Recommendations

1. **Spec 005 FR-001 wording**: Tighten the User Story 1 narrative and Edge Cases text to match the FR-001/clarification language ("single-use Actor; new affinity requires a new instance") so the top-level story doesn't imply reactivation is supported. This is documentation-only; no code change needed.
2. **Actor re-activation panic vs. error**: Consider converting the `.expect("handler already taken...")` panic in `activate()` (actor.rs:604-617) into a typed `ActorError` variant (e.g., `ActorError::AlreadyConsumed`) for defense-in-depth, since a misuse path currently panics rather than returning `Result::Err` as the rest of the API does.
3. **Spec 004 FR-015**: Either relax the FR-015 wording to explicitly acknowledge the native-construction-API tradeoff (already partially done) or add a `split()`-equivalent convenience constructor to the third-party backends for full ergonomic parity with built-in channels.
4. **Backfill spec coverage** for the 4 unspecced items above — most naturally as amendments to spec 003 (actor lifecycle: `on_idle`, `signal_stop`, `try_send`) since they extend the existing Actor/Channel FRs rather than introducing new user stories.
