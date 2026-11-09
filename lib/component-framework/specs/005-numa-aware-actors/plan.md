# Implementation Plan: NUMA-Aware Actor Thread Pinning and Memory Allocation

**Branch**: `005-numa-aware-actors` | **Date**: 2026-03-31 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/005-numa-aware-actors/spec.md`

## Summary

Add NUMA awareness to the actor framework: CPU thread pinning via `sched_setaffinity`, NUMA topology discovery by parsing `/sys/devices/system/node/`, NUMA-local memory allocation via `mmap` + `mbind`, NUMA-aware benchmarks comparing same-node vs cross-node latency/throughput, and a runnable example. Builds on top of features 001-004 with zero breaking changes. Uses only the `libc` crate (already in lockfile) — no new external dependencies.

## Technical Context

**Language/Version**: Rust stable (edition 2021, MSRV 1.75+)
**Primary Dependencies**: `libc` 0.2 (already in Cargo.lock; add as direct dependency), `criterion` 0.5.1 (existing)
**Storage**: N/A (in-memory constructs + sysfs reads)
**Testing**: `cargo test` for unit/integration/doc tests; Criterion for benchmarks
**Target Platform**: Linux only (uses Linux-specific syscalls: `sched_setaffinity`, `SYS_mbind`)
**Project Type**: Library (Rust crate)
**Performance Goals**: Demonstrate measurable latency difference between same-node and cross-node actor communication; NUMA-local allocation shows lower latency than default allocation for co-located actors
**Constraints**: No new external dependencies beyond `libc`. All `unsafe` code must be minimized, justified, and tested. Linux-only.
**Scale/Scope**: Extends `component-core` crate with ~4 new source files, 2 benchmark files, 1 example

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Code Correctness First | PASS | All unsafe code (sched_setaffinity, mmap, mbind) will be encapsulated in safe wrappers with validation. Doc tests on all public APIs. |
| II. Comprehensive Testing | PASS | Unit tests for CpuSet, NumaTopology, NumaNode, NumaAllocator. Integration tests for actor pinning. Doc tests on all public types/methods. |
| III. Performance Accountability | PASS | Criterion benchmarks for same-node vs cross-node latency and throughput. NUMA-local vs default allocation comparison. |
| IV. Documentation as Contract | PASS | All public types/functions will have doc comments with runnable examples. `cargo doc --no-deps` must pass. |
| V. Maintainability and Simplicity | PASS | Minimal public API surface. New `numa` module with clear single responsibility. No unnecessary abstractions. |
| Platform: Linux only | PASS | All APIs are Linux-specific (sched_setaffinity, sysfs, mbind). Consistent with constitution. |
| Toolchain: Stable Rust | PASS | No nightly features required. `libc` works on stable. |
| CI gate | PASS | All CI commands (fmt, clippy, test, doc) will be verified. |

**Post-Phase-1 re-check**: No violations. The design adds one direct dependency (`libc`) which is already resolved in the lockfile — justified for Linux syscall access. All unsafe code is encapsulated in safe public APIs with comprehensive tests.

## Project Structure

### Documentation (this feature)

```text
specs/005-numa-aware-actors/
├── spec.md              # Feature specification
├── plan.md              # This file
├── research.md          # Phase 0: Linux API research
├── data-model.md        # Phase 1: Entity definitions
├── quickstart.md        # Phase 1: Usage guide
├── contracts/
│   └── public-api.md    # Phase 1: API contracts
├── checklists/
│   └── requirements.md  # Spec quality checklist
└── tasks.md             # Phase 2: Task breakdown (via /speckit.tasks)
```

### Source Code (repository root)

```text
crates/component-core/
├── Cargo.toml                          # Add libc = "0.2" dependency
├── src/
│   ├── lib.rs                          # Add pub mod numa; export new types
│   ├── actor.rs                        # Extend Actor with cpu_affinity, numa_node
│   ├── numa/
│   │   ├── mod.rs                      # Module root: re-exports, NumaError
│   │   ├── cpuset.rs                   # CpuSet type (wraps libc::cpu_set_t)
│   │   ├── topology.rs                 # NumaTopology, NumaNode (sysfs parsing)
│   │   └── allocator.rs               # NumaAllocator (mmap + mbind)
│   └── channel/
│       ├── mpsc.rs                     # Add MpscChannel::new_numa()
│       ├── spsc.rs                     # Add SpscChannel::new_numa()
│       └── queue.rs                    # Add NUMA-aware ring buffer allocation
├── tests/
│   └── numa_integration.rs            # Integration tests for NUMA features

crates/component-framework/
├── benches/
│   ├── numa_latency_benchmark.rs       # Same-node vs cross-node latency
│   └── numa_throughput_benchmark.rs    # Same-node vs cross-node throughput
├── examples/
│   └── numa_pinning.rs                 # NUMA pinning example
```

**Structure Decision**: Extends the existing `component-core` crate with a new `numa` submodule. All NUMA-related types live in `crate::numa`. Actor extensions are minimal additions to the existing `actor.rs`. Benchmarks and example go in `component-framework` (same pattern as existing benchmarks).

## Architecture

### Module Dependency Graph

```text
numa::cpuset      ← numa::topology (uses CpuSet)
                  ← actor (uses CpuSet for affinity)

numa::topology    ← benchmarks (discover topology for CPU selection)
                  ← example (discover and display topology)

numa::allocator   ← channel::mpsc (NUMA-local ring buffer)
                  ← channel::spsc (NUMA-local ring buffer)
                  ← actor (NUMA-local handler allocation)

actor             ← benchmarks (create pinned actors)
                  ← example (demonstrate pinning)
```

### Thread Pinning Flow

```text
User code                    Actor                        OS
─────────                    ─────                        ──
actor.set_cpu_affinity(cpus)
  │
  └─► store in actor.cpu_affinity
       (validate: actor must be idle)

actor.activate()
  │
  └─► spawn thread ──────────► thread::spawn(move || {
                                 if let Some(cpus) = &affinity {
                                   sched_setaffinity(0, cpus) ──► kernel sets affinity
                                   if error → return Err(...)
                                 }
                                 enter_message_loop()
                               })
```

### NUMA-Local Allocation Flow

```text
MpscChannel::new_numa(cap, node)
  │
  ├─► NumaAllocator::new(node)
  │
  ├─► allocator.alloc(layout_for_ring_buffer)
  │     ├─► mmap(MAP_ANONYMOUS)
  │     ├─► syscall(SYS_mbind, MPOL_BIND, nodemask)
  │     ├─► touch pages (fault onto target node)
  │     └─► return NonNull<u8>
  │
  └─► construct MpscRingBuffer from raw allocation
```

### Key Design Decisions

1. **`libc` only, no `libnuma`**: All NUMA operations (affinity, topology, allocation) use `libc` syscalls and `std::fs` sysfs parsing. No external C library dependency.

2. **Separate `numa` module**: Clean separation of concerns. NUMA types are independent of actor/channel and can be used standalone.

3. **Builder pattern for Actor affinity**: `Actor::new(...).with_cpu_affinity(cpus)` for construction, `set_cpu_affinity()` for mutation between activations. Both produce clear errors.

4. **Affinity applied inside spawned thread**: `sched_setaffinity(0, ...)` targets the calling thread. Applied before the message loop starts. Errors propagated back via channel to `activate()`.

5. **NUMA-local allocation via mmap+mbind**: Allocates anonymous pages then binds them to a specific node. Fallback to default policy if mbind fails (FR-019).

6. **`new_numa()` channel constructors**: Separate from `new()` to maintain backward compatibility. Default constructors are unchanged (FR-003, FR-018).

7. **Topology as read-once immutable**: `NumaTopology::discover()` reads sysfs once. No hot-plug support (per assumptions).

## Complexity Tracking

No constitution violations to justify. The design adds:
- 1 direct dependency (`libc` — already in lockfile)
- 4 new source files in a single new submodule
- Minimal unsafe code (3 syscall wrappers: `sched_setaffinity`, `mmap+mbind`, `munmap`)
- Zero breaking changes to existing APIs
