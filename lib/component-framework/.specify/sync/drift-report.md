# Drift Report: component-framework

**Generated**: pending
**Component**: `lib/component-framework` (relocated from `components/component-framework`)
**Scope**: spec.md (+ plan/data-model skim) under `specs/*/` vs implementation
under `crates/{component-core,component-macros,component-framework}/src/` and
`examples/`. READ-ONLY analysis.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 6 |
| Requirements Checked (FR + SC) | 149 |
| Aligned | 149 |
| Drifted | 2 (doc-only, stale path refs) |
| Not Implemented | 0 |
| Unspecced Features | 1 |

All functional requirements (FR) and success criteria (SC) across the six specs
are implemented and match the specs. The only drift is stale `components/…`
path references in two prior spec-sync artifact files left over from the
relocation of this crate from `components/` to `lib/`.

---

## Detailed Findings

### Spec 001-com-component-framework — COM-Style Component Framework

All 13 FRs and 6 SCs Aligned.

- FR-001 (`define_interface!`) ✓ `crates/component-macros/src/define_interface.rs`, `lib.rs:72`
- FR-002 (interface usable without impl crate) ✓ interface trait + `Interface` marker `crates/component-core/src/interface.rs`
- FR-003 (`IUnknown`: query/version/enumerate interfaces+receptacles) ✓ `crates/component-core/src/iunknown.rs`
- FR-004 (zero+ interfaces / receptacles) ✓ `define_component.rs`
- FR-005 (connect/disconnect; first-party disconnect only) ✓ `crates/component-core/src/receptacle.rs:110` (`disconnect()`); spec explicitly documents the third-party-disconnect limitation — matches reality
- FR-006 (compile-time type safety) ✓ typed `query()`/`Receptacle<T>`
- FR-007 (compile-time errors on bad macro use) ✓ macro expansion diagnostics
- FR-008 (Linux/stable) ✓ builds on stable
- FR-009 (unconnected receptacle returns error, no panic) ✓ `receptacle.rs`
- FR-010 (lifetime params in interface methods) ✓ `define_interface.rs`; doctest `component-macros/src/lib.rs:60-72`
- FR-011 (`query_interface!` macro, works with refs/Arc/ComponentRef) ✓ `crates/component-core/src/iunknown.rs:263`
- FR-012 (prelude module) ✓ `crates/component-core/src/prelude.rs`
- FR-013 (`new_default()` for Default fields) ✓ `crates/component-macros/src/define_component.rs:284-297`
- SC-001..006 ✓ (macro brevity, decoupled composition, constant-time query via TypeId map, doc/unit tests, benches compile, crate-level docs)

### Spec 002-registry-refcount-binding — Registry, Ref-Counting, Binding

All 20 FRs and 7 SCs Aligned.

- FR-001..008 (registry map/register/create-by-name/not-found/duplicate/list/unregister/thread-safe) ✓ `crates/component-core/src/registry.rs:62-210` (`RegistryError::{NotFound,AlreadyRegistered,FactoryFailed}`)
- FR-009..013 (atomic refcount via Arc, attach, Drop-based release, cross-thread, compile-time UAF prevention) ✓ `crates/component-core/src/component_ref.rs`
- FR-014..019 (first-party + third-party binding, enumerate by name, TypeId resolution + mismatch error, single wiring op) ✓ `crates/component-core/src/binding.rs:100-128`
- FR-020 (`register_simple`) ✓ `registry.rs:147`
- FR-018 (factory returns single `ComponentRef`) ✓ `registry.rs` `ComponentFactory::create`
- Factory panic isolation ✓ `catch_unwind` `registry.rs:195-200`
- SC-001..007 ✓

### Spec 003-actor-channels — Actor Model with Channel Components

All 31 FRs and 7 SCs Aligned.

- FR-001..006 (actor owns thread, same component model / `IUnknown`, sequential processing, activate/deactivate with error on double, introspection, panic caught + error callback) ✓ `crates/component-core/src/actor.rs:71-745`
- FR-028 (`on_idle` hook, default no-op returns no-work) ✓ `actor.rs:88-93`
- FR-029 (`signal_stop` non-blocking) ✓ `actor.rs:190`
- FR-030 (non-blocking `try_send` on handle + sender) ✓ `actor.rs:173`; `channel/mod.rs:298`
- FR-007..012 (channels first-class w/ sender+receiver interfaces, SPSC, MPSC, lock-free queue, typed msgs, closure signal) ✓ `channel/spsc.rs`, `channel/mpsc.rs`, `channel/queue.rs`, `channel/mod.rs:315-347`
- FR-013..017 (binding enforcement: SPSC reject 2nd sender/receiver, MPSC accept N senders / reject 2nd receiver, slot reuse on disconnect) ✓ `channel/spsc.rs:191`, `channel/mpsc.rs:468-495`; `ChannelError::BindingRejected`
- FR-018..020 (registerable, first+third-party binding, configurable capacity) ✓ `Actor::with_capacity` `actor.rs:453`
- FR-021..024 (ping-pong, producer-consumer, fan-in, tokio ping-pong examples) ✓ `examples/actor_ping_pong.rs`, `actor_pipeline.rs`, `actor_fan_in.rs`, `tokio_ping_pong.rs`
- FR-025 (`pipe()` + `pipe_mpsc()`) ✓ `actor.rs:834,884`
- FR-026 (`Actor::simple()`, default cap 1024) ✓ `actor.rs:408`
- FR-027 (channel `split()`) ✓ `spsc.rs:171`, `mpsc.rs:433`
- FR-031 (`register_for_unpark` on MPSC receiver, internal to poll loop) ✓ `mpsc.rs:297`; used at `actor.rs:679`
- SC-001..007 ✓

### Spec 004-channel-benchmarks — Channel Backend Benchmarks

All 17 FRs and 7 SCs Aligned.

- FR-001..004 (crossbeam bounded+unbounded, kanal, rtrb SPSC-only, tokio MPSC channel components) ✓ `channel/{crossbeam_bounded,crossbeam_unbounded,kanal_bounded,rtrb_spsc,tokio_mpsc}.rs`
- FR-005..007 (same `IUnknown`/ISender/IReceiver model, topology binding constraints, introspection) ✓ per-backend components
- FR-008..013 (throughput + latency benches, SPSC/MPSC groups, 2/4/8 producers, ≥2 message sizes, ≥2 capacities) ✓ `benches/channel_spsc_benchmark.rs`, `channel_mpsc_benchmark.rs`, `channel_latency_benchmark.rs`, `channel_throughput.rs`
- FR-014, FR-015 (unit tests + doc examples per backend) ✓
- FR-016 (comparable results; group IDs `{topology}_throughput_{type}/{backend}/{capacity}`) ✓ `channel_spsc_benchmark.rs:64-70` emits `spsc_throughput_u64/builtin/{64,1024,16384}` — matches the 2026-08-07 backfilled wording
- FR-017 (backpressure implicitly exercised at small capacities) ✓ capacity 64 group present
- SC-001..007 ✓

### Spec 005-numa-aware-actors — NUMA-Aware Actor Pinning & Allocation

All 20 FRs and 8 SCs Aligned.

- FR-001..006 (CpuSet affinity via `set_cpu_affinity()`/`with_cpu_affinity()`, pin before loop, no-affinity backward compat, validate CPU IDs, OS-error propagation, empty-set rejected) ✓ `crates/component-core/src/numa/cpuset.rs`; `actor.rs:511,535,558`; tests `actor.rs:1126-1146`
- FR-015..019 (`NumaAllocator` node-bound alloc, channel `new_numa()` first-touch delegation, handler first-touch, default policy when unset, fallback to default on failure) ✓ `numa/allocator.rs:41-119`; `spsc.rs:147`, `mpsc.rs:409` `new_numa` delegates (matches FR-016 documented behavior)
- FR-007..009 (`NumaTopology::discover()`, all online CPUs accounted, single-node fallback) ✓ `numa/topology.rs:135-141`
- FR-010..013, FR-020 (same-node + cross-node latency/throughput benches, labeled, numa-local vs default comparison) ✓ `benches/numa_latency_benchmark.rs`, `numa_throughput_benchmark.rs` (`spsc` vs `spsc_numa_alloc`, `same_node`/`cross_node` at lines 42,78,124,159)
- FR-014 (NUMA pinning example) ✓ `examples/numa_pinning.rs`
- SC-001..008 ✓

### Spec 006-log-handler — Generic Log Handler

All 8 FRs and 5 SCs Aligned. Implemented in `crates/component-core/src/log.rs`.

- FR-001 (`LogLevel` enum ordered) ✓ `log.rs:39-49`
- FR-002 (`LogMessage` + debug/info/warn/error ctors) ✓ `log.rs:82-145`
- FR-003 (`LogHandler` impl `ActorHandler<LogMessage>`, stderr) ✓ `log.rs:291-307`
- FR-004 (`with_file`, append, buffered) ✓ `log.rs:229-235`
- FR-005 (`with_min_level` filtering) ✓ `log.rs:247`, `293-295`
- FR-006 (flush on `on_stop`) ✓ `log.rs:309-314`
- FR-007 (ISO-8601 timestamp + 5-char level tag) ✓ `log.rs:285-289`, Display `51-60`
- FR-008 (timestamp from `SystemTime`, no external deps) ✓ `log.rs:260`
- SC-001..005 ✓ (incl. `actor_log` example `examples/actor_log.rs`, `Default==new()` `log.rs:253`)

---

## Drifted Items ⚠️

| # | Requirement | Spec vs Actual | Location | Severity |
|---|-------------|----------------|----------|----------|
| 1 | Post-relocation path references | Doc references old path `components/component-framework/...` after crate moved to `lib/component-framework/` | `.specify/sync/align-tasks.md:29` | minor |
| 2 | Post-relocation path references | Suggested `git add`/commit command references old `components/component-framework/specs/...` path | `.specify/sync/apply-report.md:60` | minor |

Both are stale spec-sync artifacts from before the `components/` → `lib/`
relocation. No spec.md, plan.md, quickstart.md, README.md, or Cargo manifest
retains the old path. Severity minor: they do not break a working build/command
(the commands are historical), but the referenced paths no longer exist.

---

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| Additional actor benches beyond spec-named ones (`actor_latency`, `binding`, `component_ref`, `method_dispatch`, `query_interface`, `receptacle`, `registry`) | `crates/component-framework/benches/` | — | Covered in spirit by SC-005 (001) / performance-accountability constitution; no dedicated FR names each bench. No action needed. |

---

## Recommendations

1. Update the two stale `components/component-framework` path references in
   `.specify/sync/align-tasks.md:29` and `.specify/sync/apply-report.md:60` to
   `lib/component-framework/...` (or note them as historical). Minor.
2. No source or spec changes required — implementation is fully aligned with
   all six specs.
