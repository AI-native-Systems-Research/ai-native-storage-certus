# Data Model: NUMA-Aware Actor Thread Pinning and Memory Allocation

**Feature**: 005-numa-aware-actors
**Date**: 2026-03-31

## Entities

### CpuSet

A set of CPU core IDs representing a thread affinity mask.

| Field | Type | Description |
|-------|------|-------------|
| inner | `libc::cpu_set_t` | Underlying OS affinity bitmask (1024 bits on x86-64) |

**Validation rules**:
- Must contain at least one CPU (FR-006: empty set rejected with error)
- All CPU IDs must be < `CPU_SETSIZE` (1024)
- All CPU IDs must correspond to online CPUs on the system (FR-004)

**Builder API**:
- `CpuSet::new()` — empty set (invalid until at least one CPU added)
- `CpuSet::from_cpu(cpu_id)` — single CPU
- `CpuSet::from_cpus(iter)` — multiple CPUs
- `add(cpu_id)` — add a CPU
- `remove(cpu_id)` — remove a CPU
- `contains(cpu_id) -> bool` — test membership
- `count() -> usize` — number of CPUs in set
- `iter() -> impl Iterator<Item=usize>` — iterate over CPU IDs
- `as_raw() -> &libc::cpu_set_t` — access underlying type for syscalls

**Relationships**:
- Used by `Actor` (optional affinity configuration)
- Produced by `NumaNode::cpus()` (CPUs belonging to a node)

---

### NumaNode

A single NUMA node with its associated CPU set and memory region.

| Field | Type | Description |
|-------|------|-------------|
| id | `usize` | NUMA node ID (0-indexed) |
| cpus | `CpuSet` | CPUs belonging to this node |
| distances | `Vec<u32>` | NUMA distances to all other nodes (index = node ID) |

**Validation rules**:
- `id` must be a valid node from `/sys/devices/system/node/online`
- `cpus` must be non-empty (every node has at least one CPU)
- `distances[id]` is always the local distance (typically 10)

**Relationships**:
- Contained by `NumaTopology`
- Produces `CpuSet` for actor pinning

---

### NumaTopology

Runtime representation of the system's NUMA layout.

| Field | Type | Description |
|-------|------|-------------|
| nodes | `Vec<NumaNode>` | All NUMA nodes, indexed by node ID |

**Validation rules (FR-007, FR-008, FR-009)**:
- Every online CPU appears in exactly one node
- At least one node exists
- On non-NUMA systems, fallback: single node containing all online CPUs

**Query API**:
- `NumaTopology::discover() -> Result<Self>` — read sysfs and build topology
- `node_count() -> usize` — number of NUMA nodes
- `node(id) -> Option<&NumaNode>` — get node by ID
- `nodes() -> &[NumaNode]` — all nodes
- `node_for_cpu(cpu_id) -> Option<usize>` — find which node a CPU belongs to
- `online_cpus() -> CpuSet` — all online CPUs across all nodes

**State**: Immutable after construction. Topology is read once and assumed stable (per assumptions).

---

### NumaAllocator

A NUMA-local memory allocator that binds allocations to a specific NUMA node.

| Field | Type | Description |
|-------|------|-------------|
| node_id | `usize` | Target NUMA node for allocations |

**API**:
- `NumaAllocator::new(node_id) -> Self` — create allocator for a node
- `alloc(layout: Layout) -> Result<NonNull<u8>>` — allocate on target node (mmap + mbind)
- `dealloc(ptr: NonNull<u8>, layout: Layout)` — free (munmap)

**Implementation details**:
- Uses `mmap(MAP_PRIVATE | MAP_ANONYMOUS)` + `syscall(SYS_mbind, MPOL_BIND, nodemask)`
- Pages are touched after mbind to fault them onto the target node
- Allocations are page-aligned (mmap granularity)
- Fallback: if mbind fails, returns mmap'd memory with default policy (FR-019)

**Validation rules**:
- `node_id` must be valid (checked against topology)
- `layout.size()` must be > 0

**Relationships**:
- Used by `MpscChannel`/`SpscChannel` for NUMA-local ring buffer allocation
- Used by `Actor` for NUMA-local handler state allocation
- Configured via `NumaNode.id` from topology discovery

---

## Entity Relationships

```text
NumaTopology
  └── Vec<NumaNode>
        ├── id: usize
        ├── cpus: CpuSet ──────────── Actor.cpu_affinity: Option<CpuSet>
        └── distances: Vec<u32>

NumaAllocator(node_id) ──────────── MpscChannel/SpscChannel (ring buffer allocation)
                                    Actor (handler state allocation)

Actor (extended)
  ├── cpu_affinity: Option<CpuSet>  (mutable between activations)
  ├── numa_node: Option<usize>      (for NUMA-local channel allocation)
  └── channel: MpscChannel<M>       (optionally NUMA-allocated)
```

## State Transitions

### Actor with CPU Affinity

```text
                    set_cpu_affinity()
Idle ──────────────────────────────── Idle (affinity updated)
  │                                     │
  │ activate()                          │ activate()
  │ [apply affinity to thread]          │ [apply new affinity to thread]
  ▼                                     ▼
Running ──────────────────────────── Running
  │                                     │
  │ deactivate()                        │ deactivate()
  ▼                                     ▼
Idle ──────────────────────────────── Idle
```

- `set_cpu_affinity()` is only valid in `Idle` state (returns error if `Running`)
- `activate()` applies the current affinity (if set) to the newly spawned thread
- If affinity is `None`, thread starts with system default (backward compatible)
- On re-activation after affinity change, the new affinity applies to the new thread
