# Component-Framework Drift Report

Generated: 2026-08-07T15:27:06Z
Project: component-framework (Certus)
Procedure: speckit-sync-analyze (spec-vs-implementation drift)
Scope: 6 specs (001–006) vs `crates/{component-core,component-macros,component-framework}`

## Summary Table

| Spec | Title | Reqs (FR+SC) | Aligned | Drifted | Not Impl | Unspecced |
|------|-------|-------------|---------|---------|----------|-----------|
| 001-com-component-framework | COM-style component framework | 19 | 19 | 0 | 0 | 2 |
| 002-registry-refcount-binding | Registry, refcount, binding | 27 | 27 | 0 | 0 | 1 |
| 003-actor-channels | Actor model with channels | 38 | 38 | 0 | 0 | 3 |
| 004-channel-benchmarks | Channel backend benchmarks | 24 | 23 | 1 | 0 | 1 |
| 005-numa-aware-actors | NUMA-aware actors | 28 | 28 | 0 | 0 | 3 |
| 006-log-handler | Generic log handler | 13 | 13 | 0 | 0 | 0 |
| **Total** | | **149** | **148** | **1** | **0** | **10** |

Overall the implementation is remarkably faithful to the specs. Several specs
were explicitly written to describe deliberate scope reductions (e.g. FR-005 in
001, FR-016/017 in 004/005), and the code matches those documented limitations
rather than diverging from them. Only one genuine (minor) drift was found.

## Detailed Findings

### 001-com-component-framework — CLEAN

All 13 FRs and 6 SCs Aligned.

- FR-001 `define_interface!` — `crates/component-macros/src/define_interface.rs:78` (trait + `Interface` impl for `dyn Name + Send + Sync`).
- FR-002 interface usable without impl crate — trait-only generation, `define_interface.rs:90`; validated by `tests/interface_definition.rs`.
- FR-003 `IUnknown` (query/version/enumerate ifaces/enumerate receptacles) — `crates/component-core/src/iunknown.rs:40-132`.
- FR-004 zero-or-more provides/receptacles — `define_component.rs:126-247` (both lists optional).
- FR-005 receptacle connect/disconnect; third-party disconnect NOT supported — `crates/component-core/src/receptacle.rs:84-117` (first-party `disconnect()` present); no `disconnect_receptacle_raw` exists, matching the spec's explicit statement.
- FR-006 compile-time type safety — typed `Receptacle<T>`, typed `query::<I>` (`iunknown.rs:189`).
- FR-007 compile-time macro errors — `define_interface.rs:41-69` (missing `&self`, `&mut self`, no methods); `define_component.rs:44-108`.
- FR-008 Linux + stable — no nightly features observed.
- FR-009 unconnected receptacle returns error not panic — `receptacle.rs:141-147` returns `NotConnected`.
- FR-010 lifetime params in interface methods — signatures forwarded verbatim (`define_interface.rs:81-88`); doc test at `component-macros/src/lib.rs:62-70`.
- FR-011 `query_interface!` for `&T`/`Arc<T>`/`ComponentRef` — `iunknown.rs:262-292` (`AsIUnknown` impls for `T`, `Arc<T>`, `ComponentRef`).
- FR-012 prelude single-import — `crates/component-core/src/prelude.rs:19-29`.
- FR-013 `new_default()` when fields impl Default — `define_component.rs:286-303`.
- SC-001..006 — under-20-line define+impl (doc test `lib.rs:100-131`); decoupled composition (`tests/receptacle_wiring.rs`); constant-time query via `TypeId` HashMap (`component.rs` InterfaceMap); doc+unit tests present; benches compile (`benches/query_interface.rs`, `method_dispatch.rs`, `receptacle.rs`).

### 002-registry-refcount-binding — CLEAN

All 20 FRs and 7 SCs Aligned. Implementation in `registry.rs`, `component_ref.rs`, `binding.rs`.

- FR-001..007 registry map/register/create+config/not-found/duplicate/list/unregister — `registry.rs:93-213`.
- FR-008 thread-safe registry — `RwLock<HashMap>` (`registry.rs:57`); test `registry_is_send_sync`.
- FR-009..013 atomic refcount / attach / Drop release / cross-thread / compile-time UAF prevention — `component_ref.rs:41-101` (Arc-backed, `attach()`, `Deref`, `ref_count()`); tests at `component_ref.rs:136-216`.
- FR-014 first-party binding — `receptacle.rs` connect; FR-015..019 third-party `bind()` by name with TypeId verification — `binding.rs:100-138`; generated `connect_receptacle_raw` arms — `define_component.rs:252-282`.
- FR-018 factory returns single external `ComponentRef` — `registry.rs:182-205`; factory-panic containment via `catch_unwind` (`registry.rs:194`).
- FR-020 `register_simple` — `registry.rs:147-152`.
- SC-001..007 — met; concurrency test in `tests/registry.rs` / `tests/assembly.rs`.

### 003-actor-channels — CLEAN

All 31 FRs and 7 SCs Aligned. Implementation in `actor.rs`, `channel/{mod,spsc,mpsc}.rs`.

- FR-001/003/004 dedicated-thread actor, sequential handling, activate/deactivate with double-activate CAS guard — `actor.rs:589-602`, `activate`/`deactivate`/`Drop`.
- FR-002 actor conforms to component model (impls `IUnknown` directly for generics) — `actor.rs:750-793`.
- FR-005 introspection — `provided_interfaces`/`version`/`receptacles` (`actor.rs:772-782`).
- FR-006 panic caught + error callback, actor survives — `actor.rs:651-657`; test `panic_recovery_with_error_callback`.
- FR-028 idle hook returning did-work + no-op default — `ActorHandler::on_idle` (`actor.rs:90-92`); poll loop `actor.rs:659-683`.
- FR-029 non-blocking stop signal — `ActorHandle::signal_stop` (`actor.rs:190-193`).
- FR-030 non-blocking send — `ActorHandle::try_send` (`actor.rs:173`) + `Sender::try_send`/`MpscSender::try_send`.
- FR-007..012 channels as components, SPSC+MPSC, lock-free queues, typed messages, closure signalling — `channel/mod.rs`, `spsc.rs`, `mpsc.rs`, `queue.rs`.
- FR-013..017 binding enforcement — SPSC rejects 2nd sender/receiver via CAS (`spsc.rs:191-200`); MPSC allows many senders, rejects 2nd receiver (`mpsc.rs`); slot freed on `Drop` (`mod.rs:325-347`).
- FR-018/019 registry + first/third-party binding — `tests/actor_pipeline.rs`, `tests/assembly.rs`.
- FR-020 configurable capacity — `Actor::with_capacity` (`actor.rs:453`), `SpscChannel::new(capacity)`.
- FR-021..024 examples — `examples/actor_ping_pong.rs`, `examples/actor_pipeline.rs`, `examples/actor_fan_in.rs`, `examples/tokio_ping_pong.rs`.
- FR-025 `pipe()`/`pipe_mpsc()` — `actor.rs:834-896`.
- FR-026 `Actor::simple()` (default cap 1024, silent panic) — `actor.rs:408-410`.
- FR-027 `split()` — `spsc.rs:171`, `mpsc.rs:433`.
- FR-031 receiver `register_for_unpark` for actor park/unpark — `mpsc.rs:297`.
- SC-001..007 — met; backward-compat tests (001/002) pass, `no_affinity_backward_compatible`.

### 004-channel-benchmarks — 1 MINOR DRIFT

23 of 24 requirements Aligned.

Aligned:
- FR-001..004 backends: crossbeam bounded+unbounded, kanal, rtrb (SPSC-only), tokio MPSC — `channel/{crossbeam_bounded,crossbeam_unbounded,kanal_bounded,rtrb_spsc,tokio_mpsc}.rs`.
- FR-005/007 same component model + introspection — each backend impls `IUnknown` with ISender/IReceiver.
- FR-006 binding constraints per topology — `sender()`/`receiver()` CAS guards in each backend.
- FR-008/010/011/012/013 throughput suite, SPSC+MPSC groups, 2/4/8 producers, message sizes (u64 + Vec<u8>), multiple capacities — `benches/channel_spsc_benchmark.rs`, `benches/channel_mpsc_benchmark.rs` (`for producers in [2u64,4,8]`).
- FR-009 latency (single-thread send+recv, spec-acknowledged current impl) — `benches/channel_latency_benchmark.rs`.
- FR-014/015 unit tests + doc examples on backends; native construction API rather than `split()` — matches spec text.
- FR-017 backpressure via small-capacity bounded runs (spec-acknowledged, no dedicated bench) — Aligned per spec wording.
- SC-001..007 — met.

**Drifted (minor):**
- **FR-016** — benchmark group-ID naming. Spec states the pattern
  `{topology}_throughput_{type}/{backend}/{capacity}` with example
  `spsc_throughput_u64/built_in/capacity_1024`. Code produces group
  `spsc_throughput_u64` with `BenchmarkId::new("builtin", capacity)`, i.e.
  `spsc_throughput_u64/builtin/1024` — backend token is `builtin` (not
  `built_in`) and the capacity is a bare number (not `capacity_1024`).
  Severity: minor (cosmetic; results are still self-describing and comparable).
  Location: `crates/component-framework/benches/channel_spsc_benchmark.rs:64-70`
  (and analogous IDs in `channel_mpsc_benchmark.rs`).

### 005-numa-aware-actors — CLEAN

All 20 FRs and 8 SCs Aligned. Implementation in `numa/{mod,cpuset,topology,allocator}.rs`, `actor.rs`.

- FR-001 `set_cpu_affinity()` / `with_cpu_affinity()`, single-use actor — `actor.rs:511-541`.
- FR-002 pin before message loop — thread applies affinity before `on_start` (`actor.rs:632-641`).
- FR-003 no-affinity backward compat — `no_affinity_backward_compatible` test; default `None`.
- FR-004/005/006 validate CPU IDs before spawn / OS-reject error / empty-set error — `numa::validate_cpus` (`actor.rs:623-626`; `cpuset.rs:355`), `set_thread_affinity` (`cpuset.rs:300`), `CpuSet` empty rejection.
- FR-007/008/009 topology discovery, all online CPUs accounted, single-node fallback — `topology.rs:135` `discover()`, `node()`/`node_count()`/`nodes()`.
- FR-015 NUMA-local allocator (mmap+mbind MPOL_BIND) — `allocator.rs:80-140`.
- FR-016/017 channel buffers + handler state via first-touch (`new_numa` delegates to `new`; mbind deliberately not used for channel buffers) — `spsc.rs:147`, `mpsc.rs:409`; matches spec.
- FR-018/019 default policy when unspecified + allocator fallback — `allocator.rs` ignores mbind failure.
- FR-010..013/020 benchmarks: same-node/cross-node latency+throughput, labeled, plus `spsc_numa_alloc` vs `spsc` comparison — `benches/numa_latency_benchmark.rs`, `benches/numa_throughput_benchmark.rs`.
- FR-014 example — `examples/numa_pinning.rs`.
- SC-001..008 — met (SC-008 covered by `spsc_numa_alloc` same_node vs cross_node groups).

### 006-log-handler — CLEAN

All 8 FRs and 5 SCs Aligned. Implementation in `crates/component-core/src/log.rs`.

- FR-001 `LogLevel` ordered enum — `log.rs:39-49`.
- FR-002 `LogMessage` + `debug/info/warn/error` ctors — `log.rs:82-145`.
- FR-003 `LogHandler: ActorHandler<LogMessage>` to stderr — `log.rs:291-307`.
- FR-004 `with_file()` append + buffered — `log.rs:229-235`.
- FR-005 `with_min_level()` filtering — `log.rs:247-250`, `log.rs:293-295`.
- FR-006 flush on `on_stop` — `log.rs:309-314`.
- FR-007 ISO-8601 timestamp + 5-char padded level — `log.rs:259-289`, `LogLevel::Display`.
- FR-008 timestamp from `SystemTime`, no external deps — `log.rs:260`.
- SC-001..005 — met; `LogHandler::default()==new()` test; example `examples/actor_log.rs`.

## Unspecced Code

| Item | Location | Spec | Note |
|------|----------|------|------|
| `ReceptacleInfo.interface_name` field | `interface.rs` / used in `define_component.rs:243` | 001 | Extra metadata beyond FR-003 introspection; benign, used by `bind()`. |
| `ComponentRef::ref_count()` | `component_ref.rs:72` | 002 | Public testing/debug helper not called out in any FR. |
| `ActorHandler::on_start` / `on_stop` | `actor.rs:76-81` | 003 | Public lifecycle hooks; only `on_idle` (FR-028) and `handle` are specced in 003 (`on_stop` is relied on by 006 FR-006 but not defined in 003). |
| `Actor::with_capacity()` | `actor.rs:453` | 003 | Public ctor; capacity config is FR-020 but the explicit-capacity ctor variant is not named in the spec. |
| `SpscChannel::with_default_capacity()` | `spsc.rs:119` | 003 | Convenience ctor not specced. |
| Channel park/unpark backoff (`escalate`, SPIN/YIELD/PARK limits, `force_closed`) | `channel/mod.rs:33-70,183` | 003 | Internal blocking strategy; not surfaced in spec (reasonable impl detail). |
| `NumaTopology::node_for_cpu()`, `nodes()` | `topology.rs:194,211` | 005 | Extra topology introspection beyond FR-007/008. |
| `parse_range_list()` (pub) | `topology.rs` (re-exported `numa/mod.rs:35`) | 005 | Public sysfs range-parser helper, not specced. |
| `get_thread_affinity()` | `cpuset.rs:333` | 005 | Public affinity read-back not required by any FR. |
| `NumaAllocator` exposed but unused by channels/handlers | `allocator.rs` | 005 | Satisfies FR-015 but is not wired into the first-touch paths (FR-016/017); standalone public API. |

## Conflicts / Nonexistent References

None found. The 6 spec.md files do not reference concrete source files, proof
artifacts, or directories that are absent from the tree. All named examples
(`actor_ping_pong`, `actor_pipeline`, `actor_fan_in`, `tokio_ping_pong`,
`numa_pinning`, `actor_log`) and all declared benches exist.

## Recommendations

1. **FR-016 (004)** — Either (a) update the benchmark `BenchmarkId`s to emit
   `built_in` and `capacity_1024` to match the spec's stated pattern, or
   (b) relax the spec's example to the actual `builtin`/`<capacity>` tokens.
   Cosmetic; pick whichever keeps downstream result-parsing tooling stable.
2. **on_start/on_stop (003)** — Add an FR (or a note under FR-028) documenting
   the `on_start`/`on_stop` lifecycle hooks, since spec 006 already depends on
   `on_stop`. This removes a cross-spec implicit dependency.
3. **NumaAllocator (005)** — Clarify in the spec that `NumaAllocator`
   (mbind-based, FR-015) is provided as a standalone allocator while channel
   buffers/handler state use first-touch (FR-016/017); optionally note it is
   not auto-wired, to preempt "why isn't the allocator used?" questions.
4. Remaining unspecced items are benign convenience/introspection helpers and
   internal implementation details; backfilling them into specs is optional.
