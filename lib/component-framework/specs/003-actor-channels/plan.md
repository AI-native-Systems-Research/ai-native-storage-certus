# Implementation Plan: Actor Model with Channel Components

**Branch**: `003-actor-channels` | **Date**: 2026-03-31 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/003-actor-channels/spec.md`

## Summary

Add actor components (thread-owning components with message-loop semantics) and channel components (first-class SPSC/MPSC lock-free queues) to the existing component framework. Actors and channels use the same component/interface/receptacle model from features 001-002. Channels enforce topology constraints at bind time (SPSC rejects second sender/receiver, MPSC allows multiple senders). All new types integrate with the existing registry, binding, and introspection infrastructure.

## Technical Context

**Language/Version**: Rust stable (edition 2021, MSRV 1.75+)
**Primary Dependencies**: `proc-macro2`, `quote`, `syn` (existing proc macros); no new external dependencies
**Storage**: N/A (in-memory runtime constructs)
**Testing**: `cargo test --all` (unit, integration, doc tests); Criterion benchmarks for channel throughput and actor message latency
**Target Platform**: Linux
**Project Type**: Library (Rust crate workspace)
**Performance Goals**: Lock-free channel operations; 100K+ messages/sec sustained throughput per channel; sub-microsecond send/receive for uncontended paths
**Constraints**: No external lock-free queue crate (built in-house per constitution); no `unsafe` beyond what's justified for the lock-free queue core; no nightly features
**Scale/Scope**: Extends existing 3-crate workspace (component-core, component-macros, component-framework) plus examples

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Requirement | Status | Notes |
|-----------|-------------|--------|-------|
| I. Correctness | Public APIs have unit tests; doc tests on all public APIs; unsafe justified and tested | PASS | Lock-free queue will require `unsafe` — must be minimized, justified, and thoroughly tested |
| II. Testing | Unit + integration + doc tests; TDD preferred; `cargo test --all` zero failures | PASS | TDD approach: write failing tests per user story, then implement |
| III. Performance | Criterion benchmarks for perf-sensitive APIs | PASS | Benchmarks needed for: channel send/recv throughput, actor message latency, MPSC contention |
| IV. Documentation | Doc comments + examples on all public types/functions; `cargo doc --no-deps` clean | PASS | All new public types (Actor, Channel, Sender, Receiver) need doc comments with examples |
| V. Maintainability | Minimal API surface; `cargo fmt` + `cargo clippy` clean; single-responsibility modules | PASS | New modules: `actor.rs`, `channel/mod.rs`, `channel/spsc.rs`, `channel/mpsc.rs`, `channel/queue.rs` |
| Platform | Linux only, Rust stable, no nightly | PASS | No nightly features needed |
| CI Gate | fmt + clippy + test + doc all pass | PASS | Will validate continuously |

**Unsafe justification**: The lock-free ring buffer requires `unsafe` for:
1. `UnsafeCell` access for the ring buffer slots (concurrent read/write to different slots)
2. `MaybeUninit` for slot storage (avoid requiring `Default`)
3. Atomic ordering for head/tail pointers

Each `unsafe` block will have a `// SAFETY:` comment and dedicated tests exercising the invariants.

## Project Structure

### Documentation (this feature)

```text
specs/003-actor-channels/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
crates/
├── component-core/src/
│   ├── actor.rs              # NEW: Actor trait, ActorHandle, ActorError, activate/deactivate
│   ├── channel/
│   │   ├── mod.rs            # NEW: Channel traits (ISender, IReceiver), ChannelError
│   │   ├── queue.rs          # NEW: Lock-free ring buffer (SPSC core)
│   │   ├── spsc.rs           # NEW: SpscChannel component
│   │   └── mpsc.rs           # NEW: MpscChannel component
│   ├── binding.rs            # EXISTING (unchanged)
│   ├── component_ref.rs      # EXISTING (unchanged)
│   ├── component.rs          # EXISTING (unchanged)
│   ├── error.rs              # EXTENDED: ActorError variants
│   ├── interface.rs          # EXISTING (unchanged)
│   ├── iunknown.rs           # EXISTING (unchanged)
│   ├── lib.rs                # EXTENDED: new module declarations and re-exports
│   ├── receptacle.rs         # EXISTING (unchanged)
│   └── registry.rs           # EXISTING (unchanged)
├── component-macros/src/
│   ├── define_component.rs   # EXISTING (unchanged)
│   ├── define_actor.rs       # NEW: define_actor! macro (generates actor boilerplate)
│   └── lib.rs                # EXTENDED: export define_actor!
└── component-framework/
    ├── src/lib.rs             # EXISTING: re-exports (automatically picks up new core modules)
    ├── tests/
    │   ├── actor.rs           # NEW: Actor lifecycle integration tests
    │   ├── channel_spsc.rs    # NEW: SPSC channel integration tests
    │   ├── channel_mpsc.rs    # NEW: MPSC channel integration tests
    │   ├── binding_enforcement.rs # NEW: Channel bind constraint tests
    │   ├── actor_pipeline.rs  # NEW: End-to-end actor pipeline tests
    │   └── ...                # EXISTING tests (unchanged)
    └── benches/
        ├── channel_throughput.rs  # NEW: SPSC/MPSC throughput benchmarks
        ├── actor_latency.rs       # NEW: Actor message latency benchmark
        └── ...                    # EXISTING benchmarks (unchanged)

examples/
├── actor_ping_pong.rs     # NEW: Two actors exchanging messages
├── actor_pipeline.rs      # NEW: Producer -> processor -> consumer pipeline
├── actor_fan_in.rs        # NEW: Multiple producers -> MPSC -> single consumer
└── ...                    # EXISTING examples (unchanged)
```

**Structure Decision**: Extends the existing 3-crate workspace. New actor and channel code lives in `component-core` (core traits and implementations). The `channel/` subdirectory groups channel-related code to keep the module tree organized. A new `define_actor!` macro in `component-macros` provides the same ergonomic experience as `define_component!`.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| `unsafe` in lock-free queue | Required for concurrent slot access without locks; lock-free is an explicit spec requirement (FR-010) | Mutex-based queue would violate FR-010 (lock-free requirement) and defeat performance goals |

## Phase 0: Research (Complete)

**Output**: [research.md](research.md)

Decisions made:
1. **Lock-free ring buffer**: Bounded power-of-two SPSC ring buffer with `AtomicUsize` head/tail, `UnsafeCell<MaybeUninit<T>>` slots, cache-line padding. No external deps.
2. **MPSC strategy**: Ticket-based serialized writes atop SPSC core. Each sender atomically claims a slot index.
3. **Actor threading**: One OS thread per actor via `std::thread::spawn`. Shutdown via `AtomicBool` flag + channel close.
4. **Panic recovery**: `std::panic::catch_unwind` around handler, invoke user error callback, continue loop.
5. **Channel-as-component**: Non-generic `ISender`/`IReceiver` traits at IUnknown boundary, typed `Sender<T>`/`Receiver<T>` wrappers for users.
6. **Blocking**: Default blocking via `thread::park`/`unpark`. Non-blocking `try_send`/`try_recv` variants.
7. **Disconnect tracking**: `AtomicUsize` sender count; zero senders → closed signal to receiver.

## Phase 1: Design (Complete)

**Outputs**:
- [data-model.md](data-model.md) — Entity definitions, state transitions, relationships
- [contracts/public-api.md](contracts/public-api.md) — Full public API surface
- [quickstart.md](quickstart.md) — Integration scenarios and test patterns

**Constitution Re-Check (Post-Design)**:

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Correctness | PASS | All public APIs defined with clear error semantics; unsafe limited to queue.rs |
| II. Testing | PASS | Test patterns defined in quickstart.md; TDD approach planned |
| III. Performance | PASS | Benchmarks identified: channel throughput, actor latency, MPSC contention |
| IV. Documentation | PASS | All public types documented in contracts/public-api.md |
| V. Maintainability | PASS | Modular design: actor.rs, channel/{mod,queue,spsc,mpsc}.rs |
| Platform | PASS | No nightly features, no new external dependencies |
| CI Gate | PASS | Existing gate covers all requirements |

## Phase 2: Task Generation

Run `/speckit.tasks` to generate the dependency-ordered task list.
