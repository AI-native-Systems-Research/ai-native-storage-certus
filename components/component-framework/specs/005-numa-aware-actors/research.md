# Research: NUMA-Aware Actor Thread Pinning and Memory Allocation

**Feature**: 005-numa-aware-actors
**Date**: 2026-03-31

## 1. Thread CPU Affinity (Linux `sched_setaffinity`)

### Decision: Use `libc` crate directly for `sched_setaffinity`/`sched_getaffinity`

**Rationale**: The `libc` crate (v0.2.183, already in Cargo.lock) exposes all needed APIs. No new dependencies required, consistent with the "minimal dependencies" constitution mandate.

**Alternatives considered**:
- `nix` crate: Provides safe Rust `CpuSet` wrapper, but adds a substantial dependency tree. Rejected.
- `pthread_setaffinity_np`: Available in libc but requires a `pthread_t` handle. `sched_setaffinity` with `pid=0` (current thread) is simpler and sufficient since we set affinity from within the spawned thread.

**Key API details**:
- `libc::sched_setaffinity(pid: pid_t, cpusetsize: size_t, cpuset: *const cpu_set_t) -> c_int` — pid=0 targets calling thread
- `libc::sched_getaffinity(pid: pid_t, cpusetsize: size_t, cpuset: *mut cpu_set_t) -> c_int`
- `libc::cpu_set_t` — 128 bytes on x86-64 (1024 bits), supports up to `CPU_SETSIZE` (1024) CPUs
- Helper functions: `CPU_ZERO`, `CPU_SET`, `CPU_CLR`, `CPU_ISSET`, `CPU_COUNT` — all available as safe Rust functions in `libc`
- **Error codes**: `EINVAL` (no valid CPUs in mask), `ESRCH` (no such thread), `EPERM` (insufficient privileges), `EFAULT` (bad pointer)

**Implementation pattern**: Set affinity from within the actor's spawned thread, before entering the message loop. Call `sched_setaffinity(0, ...)` to pin the calling thread.

## 2. NUMA Topology Discovery

### Decision: Parse `/sys/devices/system/node/` using `std::fs`

**Rationale**: The sysfs interface is the standard Linux mechanism for NUMA topology. Parsing it requires only `std::fs` — no external crates needed.

**Alternatives considered**:
- `hwloc` bindings: Requires external C library (`libhwloc`). Too heavy for this project.
- `numactl` Rust crate: Requires `numactl-devel` headers. External C dependency.
- `/proc/cpuinfo` parsing: Less structured, doesn't directly expose NUMA relationships.

**Key files**:
| Path | Content | Format |
|------|---------|--------|
| `/sys/devices/system/node/online` | Online NUMA nodes | Range list: `0-1` |
| `/sys/devices/system/node/nodeN/cpulist` | CPUs on node N | Range list: `0-15,32-47` |
| `/sys/devices/system/node/nodeN/distance` | Distance to all nodes | Space-separated integers: `10 32` |
| `/sys/devices/system/cpu/online` | All online CPUs | Range list: `0-63` |

**Range list format**: Comma-separated ranges with `-` for inclusive bounds. Examples: `0-15,32-47`, `0-63`, `0`. Straightforward to parse with a helper function.

**Fallback**: If `/sys/devices/system/node/` does not exist (VMs without NUMA), report a single node containing all online CPUs from `/sys/devices/system/cpu/online` or `libc::sysconf(libc::_SC_NPROCESSORS_ONLN)`.

## 3. NUMA-Local Memory Allocation

### Decision: Use `mmap` + `mbind` syscall via `libc`

**Rationale**: `libnuma` (`numa_alloc_onnode`) is the standard approach, but it requires linking against `libnuma.so` and `numactl-devel` headers — an external C dependency. The kernel's `mbind()` syscall achieves the same result and is accessible via `libc::syscall(libc::SYS_mbind, ...)`. No new dependencies.

**Alternatives considered**:
- `libnuma` (`numa_alloc_onnode`): Standard, but requires `numactl-devel` install and FFI linking. Rejected for minimal-dependency compliance.
- `set_mempolicy()`: Sets thread-level default — too broad, affects all allocations. Not suitable for per-allocation control.
- Custom allocator trait only: Would not actually bind memory to a NUMA node. Must use `mbind` for real locality.

**Key API details**:
- `libc::mmap(NULL, size, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)` — allocate anonymous pages
- `libc::syscall(libc::SYS_mbind, ptr, size, MPOL_BIND, &nodemask, maxnode, flags)` — bind pages to node
- Constants available in libc: `MPOL_DEFAULT=0`, `MPOL_PREFERRED=1`, `MPOL_BIND=2`, `MPOL_INTERLEAVE=3`, `MPOL_LOCAL=4`
- `nodemask` format: bitmask of `unsigned long` words, one bit per node
- Pages are lazily faulted — must touch after `mbind` for physical allocation on the target node
- `munmap(ptr, size)` to free

**Implementation pattern**:
1. `mmap` anonymous memory (page-aligned)
2. `mbind` with `MPOL_BIND` and nodemask targeting a single node
3. Touch pages to fault them onto the target node
4. Wrap in a Rust `NumaAllocator` that implements `Drop` (calls `munmap`)
5. Fallback: if `mbind` fails (non-NUMA kernel), return the `mmap`'d memory with default policy

**Safety notes**:
- All `mmap`/`mbind`/`munmap` calls are `unsafe` — must be encapsulated in safe wrappers
- `NumaAllocator` must ensure alignment (mmap returns page-aligned) and proper cleanup
- Must validate node ID against topology before calling `mbind`

## 4. Rust Crate Dependencies

### Decision: No new external dependencies

**Rationale**: Everything required is achievable with:
- `libc` 0.2.183 (already in Cargo.lock as transitive dep; add as direct dep to `component-core`)
- `std::fs` for sysfs topology parsing
- `criterion` 0.5.1 (already present for benchmarks)

| Capability | Provided By | Status |
|------------|------------|--------|
| `sched_setaffinity` / `sched_getaffinity` | `libc` | Already in lockfile |
| `cpu_set_t` + helpers | `libc` | Already in lockfile |
| `mmap` / `munmap` | `libc` | Already in lockfile |
| `SYS_mbind` + `MPOL_*` | `libc` | Already in lockfile |
| `/sys/` topology parsing | `std::fs` | stdlib |
| Benchmarks | `criterion` 0.5.1 | Already in lockfile |

Only change: Add `libc = "0.2"` to `[dependencies]` in `crates/component-core/Cargo.toml`.

## 5. Criterion NUMA Benchmarks

### Decision: Use `BenchmarkGroup` with `BenchmarkId` for NUMA dimension, `iter_custom` for pre-pinned threads

**Rationale**: The existing channel benchmarks already use `BenchmarkGroup` + `BenchmarkId` for parameterization. NUMA adds another dimension (same-node vs cross-node). `iter_custom` allows pre-spawning and pinning threads outside the timed region for accurate measurement.

**Pattern**:
```
benchmark_group("numa_latency")
  BenchmarkId::new("spsc", "same_node")
  BenchmarkId::new("spsc", "cross_node")
  BenchmarkId::new("spsc", "same_node_numa_alloc")
  BenchmarkId::new("spsc", "cross_node_numa_alloc")
```

**Key considerations**:
- Detect topology at bench startup; skip cross-node if only 1 NUMA node
- Pin threads before starting timed region (use `iter_custom` with persistent pinned threads)
- For NUMA-local allocation comparison: allocate channel buffer on the same node as the consumer for "numa_alloc" variants, default allocation for baseline
- Use same `MSG_COUNT` and methodology as existing channel benchmarks for comparability
- Label results with NUMA configuration via `BenchmarkId` for Criterion HTML reports

## 6. Actor Integration Design

### Decision: Extend `Actor::new` with optional `CpuSet`, apply affinity inside spawned thread

**Key design points**:
- Add `CpuSet` type: wraps `libc::cpu_set_t`, provides safe builder API
- `Actor` gains optional `cpu_affinity: Option<CpuSet>` field (mutable between activations via setter)
- `activate()` passes the `CpuSet` into the spawned thread closure
- Thread calls `sched_setaffinity(0, ...)` as its first action, before entering the message loop
- If `sched_setaffinity` fails, the thread returns an error (propagated from `activate()`)
- If no affinity is set, thread starts with system default scheduling (full backward compatibility)
- `NumaTopology` is a standalone query — not tied to `Actor`, used by the user to pick CPUs

### NUMA-local memory for channels:
- `NumaAllocator` provides `alloc_on_node(size, node) -> *mut u8` and `free(ptr, size)`
- New channel constructor variant: `MpscChannel::new_numa(capacity, node)` allocates ring buffer on specified node
- Actor can be configured with `numa_node: Option<usize>` — when set, its channel uses NUMA-local allocation
- Fallback: if NUMA allocation fails, use default allocation (FR-019)
