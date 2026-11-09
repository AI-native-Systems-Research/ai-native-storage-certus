# Public API Contract: NUMA-Aware Actor Thread Pinning and Memory Allocation

**Feature**: 005-numa-aware-actors
**Date**: 2026-03-31

## Module: `component_core::numa`

### CpuSet

```text
CpuSet::new() -> CpuSet
CpuSet::from_cpu(cpu_id: usize) -> Result<CpuSet, NumaError>
CpuSet::from_cpus(cpus: impl IntoIterator<Item = usize>) -> Result<CpuSet, NumaError>
CpuSet::add(&mut self, cpu_id: usize) -> Result<(), NumaError>
CpuSet::remove(&mut self, cpu_id: usize)
CpuSet::contains(&self, cpu_id: usize) -> bool
CpuSet::count(&self) -> usize
CpuSet::is_empty(&self) -> bool
CpuSet::iter(&self) -> impl Iterator<Item = usize>
CpuSet::as_raw(&self) -> &libc::cpu_set_t
```

**Errors**:
- `NumaError::CpuOutOfRange { cpu: usize, max: usize }` — CPU ID >= CPU_SETSIZE
- `NumaError::EmptyCpuSet` — attempted operation requiring non-empty set on empty set

### NumaNode

```text
NumaNode::id(&self) -> usize
NumaNode::cpus(&self) -> &CpuSet
NumaNode::distances(&self) -> &[u32]
NumaNode::distance_to(&self, other_node: usize) -> Option<u32>
```

### NumaTopology

```text
NumaTopology::discover() -> Result<NumaTopology, NumaError>
NumaTopology::node_count(&self) -> usize
NumaTopology::node(&self, id: usize) -> Option<&NumaNode>
NumaTopology::nodes(&self) -> &[NumaNode]
NumaTopology::node_for_cpu(&self, cpu_id: usize) -> Option<usize>
NumaTopology::online_cpus(&self) -> CpuSet
```

**Errors**:
- `NumaError::TopologyUnavailable(String)` — sysfs read failed (falls back to single node)

### NumaAllocator

```text
NumaAllocator::new(node_id: usize) -> Self
NumaAllocator::alloc(&self, layout: Layout) -> Result<NonNull<u8>, NumaError>
NumaAllocator::dealloc(&self, ptr: NonNull<u8>, layout: Layout)
NumaAllocator::node_id(&self) -> usize
```

**Errors**:
- `NumaError::AllocationFailed(String)` — mmap or mbind failed
- Falls back to default allocation if mbind fails (FR-019)

### NumaError

```text
enum NumaError {
    CpuOutOfRange { cpu: usize, max: usize },
    CpuOffline(usize),
    EmptyCpuSet,
    InvalidNode(usize),
    TopologyUnavailable(String),
    AffinityFailed(String),
    AllocationFailed(String),
}
```

## Module: `component_core::actor` (Extended)

### Actor Extensions

```text
Actor::with_cpu_affinity(self, affinity: CpuSet) -> Self
Actor::with_numa_node(self, node: usize) -> Self
Actor::set_cpu_affinity(&self, affinity: CpuSet) -> Result<(), ActorError>
Actor::set_numa_node(&self, node: usize) -> Result<(), ActorError>
Actor::cpu_affinity(&self) -> Option<&CpuSet>
Actor::numa_node(&self) -> Option<usize>
```

**Constraints**:
- `set_cpu_affinity()` and `set_numa_node()` only valid when actor is idle (FR-001)
- Returns `ActorError::AlreadyActive` if actor is running
- `with_cpu_affinity()` and `with_numa_node()` are builder methods on the constructor chain

### activate() Behavior Change

When `cpu_affinity` is `Some(cpus)`:
1. Spawned thread calls `sched_setaffinity(0, cpuset)` before entering message loop
2. If affinity fails: thread returns `Err(ActorError::AffinityFailed(...))`
3. `activate()` propagates this error to caller

When `cpu_affinity` is `None`:
- No change from current behavior (FR-003)

## Module: `component_core::channel` (Extended)

### NUMA-Aware Channel Constructors

```text
MpscChannel::new_numa(capacity: usize, node: usize) -> Self
SpscChannel::new_numa(capacity: usize, node: usize) -> Self
```

When `node` is specified, the ring buffer backing memory is allocated via `NumaAllocator::alloc()` on the specified NUMA node (FR-016).

## Benchmark API

### NUMA Benchmark Functions (in `benches/`)

```text
benches/numa_latency_benchmark.rs
benches/numa_throughput_benchmark.rs
```

Benchmark groups:
- `numa_latency/spsc/{same_node, cross_node, same_node_numa_alloc, cross_node_numa_alloc}`
- `numa_throughput/spsc/{same_node, cross_node, same_node_numa_alloc, cross_node_numa_alloc}`

## Example

```text
examples/numa_pinning.rs
```

Demonstrates: topology discovery, actor pinning to specific nodes, message exchange, latency reporting. Prints warning on single-NUMA systems.
