---
spec_sync_component: component-framework
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:46:38Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: d9c608f67d6f163d3acb0569e618cd8fb316ee9032fc1d0ae53ac317b96094eb
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report: component-framework

**Generated**: 2026-09-02T21:46:38Z
**Component**: `lib/component-framework` (relocated from `components/component-framework`)
**Scope**: spec.md (+ plan/data-model skim) under `specs/*/` vs implementation
under `crates/{component-core,component-macros,component-framework}/src/`,
`benches/`, and `examples/`. READ-ONLY analysis.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 6 |
| Requirements Checked (FR + SC) | 149 |
| Aligned | 148 |
| Drifted (functional) | 1 (ALIGN, minor — actor re-activation panic) |
| Drifted (doc/artifact only) | 2 (stale `components/…` path refs) |
| Not Implemented | 0 |
| Unspecced Features | 1 (benign extra benches) |

Verified 149 FR/SC across the six specs against the actual sources with
file:line evidence (spec FR/SC counts confirmed by grep: 001=19, 002=27,
003=38, 004=24, 005=28, 006=13 → 149). One genuine, still-open **functional**
drift remains: the actor's `activate()` panics on re-activation instead of
returning a typed error, which contradicts the single-use / error-not-panic
intent stated in 005-FR-001 (and the error-not-panic pattern of 003-FR-004).
The resolution is code-side (ALIGN), so no spec text is changed; the item is
tracked in `align-tasks.md`. The remaining two drifts are stale
`components/component-framework/…` path strings in sync artifacts left over
from the crate's relocation to `lib/`.

> **Correction vs. the prior generated report**: the previous
> `drift-report.md` marked 003-FR-004 as "Aligned ✓" and reported only the two
> doc-path drifts. That understated reality — the `activate()` re-activation
> panic (logged as an OPEN align-task on 2026-07-22 and confirmed still open in
> the 2026-08-07 sweep) is a live spec-vs-code divergence. It is re-surfaced
> here rather than rubber-stamped.

---

## Detailed Findings

### Spec 001-com-component-framework — COM-Style Component Framework

All 13 FRs and 6 SCs Aligned.

- FR-001 (`define_interface!`) ✓ `crates/component-macros/src/define_interface.rs`, `crates/component-macros/src/lib.rs:72`
- FR-002 (interface usable without impl crate) ✓ interface trait + `Interface` marker `crates/component-core/src/interface.rs`
- FR-003 (`IUnknown`: query/version/enumerate interfaces+receptacles) ✓ `crates/component-core/src/iunknown.rs`
- FR-004 (zero+ interfaces / receptacles) ✓ `define_component.rs`
- FR-005 (connect/disconnect; first-party disconnect only) ✓ `crates/component-core/src/receptacle.rs` (`disconnect()`); spec documents the third-party-disconnect limitation — matches reality
- FR-006 (compile-time type safety) ✓ typed `query()`/`Receptacle<T>`
- FR-007 (compile-time errors on bad macro use) ✓ macro expansion diagnostics
- FR-008 (Linux/stable) ✓ builds on stable
- FR-009 (unconnected receptacle returns error, no panic) ✓ `receptacle.rs`
- FR-010 (lifetime params in interface methods) ✓ `define_interface.rs`; doctest `component-macros/src/lib.rs`
- FR-011 (`query_interface!` macro; refs/Arc/ComponentRef) ✓ `crates/component-core/src/iunknown.rs`
- FR-012 (prelude module) ✓ `crates/component-core/src/prelude.rs`
- FR-013 (`new_default()` for Default fields) ✓ `crates/component-macros/src/define_component.rs`
- SC-001..006 ✓ (macro brevity, decoupled composition, constant-time query via TypeId map, doc/unit tests, benches compile, crate-level docs)

### Spec 002-registry-refcount-binding — Registry, Ref-Counting, Binding

All 20 FRs and 7 SCs Aligned.

- FR-001..008 (registry map/register/create-by-name/not-found/duplicate/list/unregister/thread-safe) ✓ `crates/component-core/src/registry.rs` (`RegistryError::{NotFound:160,AlreadyRegistered:100,FactoryFailed:23}`)
- FR-009..013 (atomic refcount via Arc, attach, Drop-based release, cross-thread, compile-time UAF prevention) ✓ `crates/component-core/src/component_ref.rs`
- FR-014..019 (first-party + third-party binding, enumerate by name, TypeId resolution + mismatch error, single wiring op) ✓ `crates/component-core/src/binding.rs` (TypeId-resolved wiring, `RegistryError::BindingFailed`)
- FR-020 (`register_simple`) ✓ `registry.rs:147`
- FR-018 (factory returns single `ComponentRef`) ✓ `registry.rs` `ComponentFactory::create`
- Factory panic isolation ✓ `catch_unwind` in `registry.rs`
- SC-001..007 ✓

### Spec 003-actor-channels — Actor Model with Channel Components

30 of 31 FRs and all 7 SCs Aligned; **FR-004 partially drifted** (see Drifted Items).

- FR-001..003, FR-005..006 (actor owns thread, same component model / `IUnknown`, sequential processing, introspection, panic caught + error callback) ✓ `crates/component-core/src/actor.rs` (message loop 647-699; `catch_unwind` 651/660/688; `IUnknown` 750-793)
- **FR-004** ⚠️ (see Drifted Items #1): the two literal clauses hold — activate-on-active returns `ActorError::AlreadyActive` (`actor.rs:589-602`), and double-*deactivation* is compile-time-prevented because `deactivate(self)` consumes the handle (`actor.rs:218`). BUT re-activation of the same `Actor` after a full activate→deactivate cycle **panics** at `actor.rs:610` / `actor.rs:617` (`.expect(...)` on an already-taken receiver/handler), violating the error-not-panic pattern this FR establishes and 005-FR-001's single-use intent.
- FR-028 (`on_idle` hook, default no-op returns `false`) ✓ `actor.rs:90-92`; used at `actor.rs:660-676`
- FR-029 (`signal_stop` non-blocking) ✓ `actor.rs:190-193`
- FR-030 (non-blocking `try_send` on handle + sender) ✓ `actor.rs:173-181`; channel sender `try_send`
- FR-007..012 (channels first-class w/ sender+receiver interfaces, SPSC, MPSC, lock-free queue, typed msgs, closure signal) ✓ `channel/spsc.rs`, `channel/mpsc.rs`, `channel/queue.rs`, `channel/mod.rs`
- FR-013..017 (SPSC reject 2nd sender/receiver, MPSC accept N senders / reject 2nd receiver, slot reuse on disconnect) ✓ `channel/spsc.rs:198,229`; `channel/mpsc.rs:495,516`; `ChannelError::BindingRejected`
- FR-018..020 (registerable, first+third-party binding, configurable capacity) ✓ `Actor::with_capacity` `actor.rs:453`
- FR-021..024 (ping-pong, producer-consumer, fan-in, tokio ping-pong examples) ✓ `examples/`
- FR-025 (`pipe()` + `pipe_mpsc()`) ✓ `actor.rs:834,884`
- FR-026 (`Actor::simple()`, default cap 1024) ✓ `actor.rs:408`
- FR-027 (channel `split()`) ✓ `spsc.rs`, `mpsc.rs`
- FR-031 (`register_for_unpark` on MPSC receiver, internal to poll loop) ✓ `mpsc.rs:297`; used at `actor.rs:679`
- SC-001..007 ✓

### Spec 004-channel-benchmarks — Channel Backend Benchmarks

All 17 FRs and 7 SCs Aligned.

- FR-001..004 (crossbeam bounded+unbounded, kanal, rtrb SPSC-only, tokio MPSC channel components) ✓ `channel/{crossbeam_bounded,crossbeam_unbounded,kanal_bounded,rtrb_spsc,tokio_mpsc}.rs`
- FR-005..007 (same `IUnknown`/ISender/IReceiver model, topology binding constraints, introspection) ✓ per-backend components
- FR-008..013 (throughput + latency benches, SPSC/MPSC groups, 2/4/8 producers, ≥2 message sizes, ≥2 capacities) ✓ `benches/channel_spsc_benchmark.rs`, `channel_mpsc_benchmark.rs`, `channel_latency_benchmark.rs`, `channel_throughput.rs`
- FR-014, FR-015 (unit tests + doc examples per backend) ✓
- FR-016 (comparable results; group IDs `{topology}_throughput_{type}/{backend}/{capacity}`) ✓ `benches/channel_spsc_benchmark.rs:64-70` — matches the 2026-08-07 backfilled wording (`spsc_throughput_u64/builtin/{64,1024,16384}`)
- FR-017 (backpressure implicitly exercised at small capacities) ✓ capacity-64 group present
- SC-001..007 ✓

### Spec 005-numa-aware-actors — NUMA-Aware Actor Pinning & Allocation

19 of 20 FRs and all 8 SCs Aligned; **FR-001 partially drifted** (single-use intent, see Drifted Items #1).

- **FR-001** ⚠️ "Actors are single-use; re-activation requires constructing a new Actor instance." The Edge Cases block (spec line 82) states "Actor is consumed on activation." The implementation does NOT consume: `Actor::activate(&self)` takes `&self` (`actor.rs:589`), and `deactivate()` resets `state` to `STATE_IDLE` (`actor.rs:229`). A second `activate()` therefore passes the CAS and then panics on the already-taken receiver/handler (`actor.rs:610/617`) instead of being compile-time-prevented (consuming `self`) or returning a typed error. CPU-affinity-configuration portion of FR-001 (`set_cpu_affinity`) ✓ `actor.rs:535`.
- FR-002..006 (pin before loop, no-affinity backward compat, validate CPU IDs, OS-error propagation, empty-set rejected) ✓ `actor.rs:622-626,631-639`; `crates/component-core/src/numa/cpuset.rs`; tests `actor.rs:1126-1148`
- FR-015..019 (`NumaAllocator` node-bound alloc, channel `new_numa()` first-touch delegation, handler first-touch, default policy when unset, fallback to default on failure) ✓ `numa/allocator.rs`; `spsc.rs`/`mpsc.rs` `new_numa` delegate (matches FR-016 documented behavior)
- FR-007..009 (`NumaTopology::discover()`, all online CPUs accounted, single-node fallback) ✓ `numa/topology.rs`
- FR-010..013, FR-020 (same-node + cross-node latency/throughput benches, labeled, numa-local vs default comparison) ✓ `benches/numa_latency_benchmark.rs`, `numa_throughput_benchmark.rs`
- FR-014 (NUMA pinning example) ✓ `examples/numa_pinning.rs`
- SC-001..008 ✓

### Spec 006-log-handler — Generic Log Handler

All 8 FRs and 5 SCs Aligned. Implemented in `crates/component-core/src/log.rs`.

- FR-001 (`LogLevel` enum ordered) ✓ `log.rs`
- FR-002 (`LogMessage` + debug/info/warn/error ctors) ✓ `log.rs`
- FR-003 (`LogHandler` impl `ActorHandler<LogMessage>`, stderr) ✓ `log.rs:291-307`
- FR-004 (`with_file`, append, buffered) ✓ `log.rs`
- FR-005 (`with_min_level` filtering) ✓ `log.rs:293-295`
- FR-006 (flush on `on_stop`) ✓ `log.rs:309-314`
- FR-007 (ISO-8601 timestamp + 5-char level tag) ✓ `log.rs:285-289,298`
- FR-008 (timestamp from `SystemTime`, no external deps) ✓ `log.rs:297`
- SC-001..005 ✓ (incl. `actor_log` example, `Default==new()`)

---

## Drifted Items

| # | Requirement | Spec vs Actual | Location | Severity | Resolution |
|---|-------------|----------------|----------|----------|------------|
| 1 | 005-FR-001 + 003-FR-004 (actor single-use / error-not-panic) | Spec: actor is single-use, "consumed on activation," lifecycle misuse reported via `Result::Err` not panic. Actual: `activate(&self)` does not consume `self`; re-activation after a deactivate cycle passes the CAS and panics via `.expect(...)` on the taken receiver/handler. | `crates/component-core/src/actor.rs:589` (`&self`), `:610` & `:617` (`.expect` panics), `:229` (deactivate resets state); spec `005/spec.md:82,90`, `003/spec.md:127` | minor (misuse path; not a happy-path bug) | **ALIGN** (code) — tracked in `align-tasks.md`. Specs already describe intended behavior; no spec edit. |
| 2 | Post-relocation path reference | `align-tasks.md` "Files to Modify" cites old path `components/component-framework/crates/component-core/src/actor.rs` after crate moved to `lib/`. | `.specify/sync/align-tasks.md:29` | trivial (doc) | Fixed this run (path updated to `lib/…`). |
| 3 | Post-relocation path reference | 2026-07-22 "Next Steps" commit command cites old `components/component-framework/specs/003-actor-channels` path. | `.specify/sync/apply-report.md:60` | trivial (doc) | Left as historical log entry (not rewritten); noted here. |

Item #1 is the only functional divergence. It is a defense-in-depth / API-consistency defect on a misuse path, not a happy-path failure: no test exercises re-activation, and correct single-use usage never hits it. It has been open since 2026-07-22 and was confirmed still open in the 2026-08-07 sweep.

---

## Unspecced Features

| Feature | Location | Suggested Spec |
|---------|----------|----------------|
| Additional Criterion benches beyond spec-named suites (`actor_latency`, `binding`, `component_ref`, `method_dispatch`, `query_interface`, `receptacle`, `registry`) | `crates/component-framework/benches/` | Covered in spirit by SC-005 (001) and the performance-accountability constitution rule; no dedicated FR names each bench. No action required. |

---

## Notes / Quirks

- **Hash tool src layout**: `scripts/spec-sync-hash.sh` documents hashing
  `<component-dir>/src/**`, but this crate has no top-level `src/` — sources
  live under `crates/{component-core,component-macros,component-framework}/src/`.
  The tool was run exactly as specified (`scripts/spec-sync-hash.sh
  lib/component-framework`) and the digest above
  (`8e73d91f…`) is whatever it printed; CI recomputes it identically, so the
  stamp stays consistent. (The tool folds in `components/interfaces/**` plus the
  component's own `specs/**`, which are the material inputs regardless of the
  `src/` naming.)
- **Digest volatility**: because the shared `components/interfaces/**` tree is
  folded into every component's digest, an unrelated concurrent edit there
  changes this component's hash. During this run the digest shifted from
  `033a7051…` to `8e73d91f…` when an interface source file
  (`components/interfaces/src/iipc.rs`, an untracked file) was modified
  mid-analysis; `8e73d91f…` is the value stamped and is stable on re-run. If
  the interfaces tree changes again before CI runs, the gate will recompute a
  different digest and flag staleness — expected behavior, not a spec drift.

## Recommendations

1. **Resolve align-task #1**: convert the `activate()` re-activation `.expect(...)`
   panics into a typed `Result::Err` (e.g., a new `ActorError::AlreadyConsumed`,
   or reuse `AlreadyActive` with updated docs), OR make `activate(self)` consume
   the `Actor` to enforce single-use at compile time (matching the spec's
   "consumed on activation" wording). Code change only — no spec edit.
2. No source changes are required for any other requirement; 148/149 are fully
   aligned with the six specs.
