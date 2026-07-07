# Certus System Architecture — `full-remote` Profile

This document describes the architecture of the Certus `full-remote` deployment: a distributed GPU KV-cache offloading cluster where multiple nodes cooperate via RDMA to serve cache lookups.

## 1. What is the `full-remote` Profile?

The `full-remote` profile extends the base `full` profile with **inter-node cache cooperation**. Each Certus node operates a local SPDK NVMe + DRAM cache and additionally:

- **Forwards local misses** to peer nodes via the RemoteLookup component
- **Serves incoming requests** from peers via the RemoteRequestHandler (RDMA-based)

This enables a cluster of Certus nodes to function as a shared distributed cache — a miss on one node may be a hit on another, avoiding expensive GPU recomputation.

## 2. High-Level Data Flow

```
┌──────────────┐   gRPC/IPC    ┌─────────────────────────────────────────────┐
│  GPU Client  │◄─────────────►│              certus-server                   │
│  (vLLM)      │               │  (Dispatcher + Remote Request Handler)       │
└──────────────┘               └──────────┬───────────────────────────────────┘
                                          │
              ┌───────────────────────────┬┴──────────────────────┐
              ▼                           ▼                        ▼
    ┌──────────────────┐     ┌───────────────────┐    ┌────────────────┐
    │  Memory-Tier     │     │  Dispatch Map     │    │  GPU Services  │
    │  (DRAM Pool)     │     │  (Index + Refs)   │    │  (CUDA DMA)    │
    └────────┬─────────┘     └───────────────────┘    └────────────────┘
             │
             ▼
    ┌──────────────────┐     ┌───────────────────┐
    │  Block Device    │────►│  Extent Manager   │
    │  (SPDK NVMe)     │     │  (Space Alloc)    │
    └──────────────────┘     └───────────────────┘

    ┌──────────────────────────────────────────────────────────────┐
    │  Remote Cache Layer                                           │
    │                                                              │
    │  ┌─────────────────────┐      ┌────────────────────────────┐ │
    │  │ RemoteLookup        │      │ RemoteRequestHandler       │ │
    │  │ (client: forward    │      │ (server: handle incoming   │ │
    │  │  misses to peers)   │      │  RDMA requests from peers) │ │
    │  └─────────┬───────────┘      └──────────────┬─────────────┘ │
    │            │                                 │               │
    └────────────┼─────────────────────────────────┼───────────────┘
                 │          RDMA fabric            │
                 ▼                                 ▼
         ┌───────────────────────────────────────────┐
         │  Peer Certus Nodes (same full-remote)     │
         └───────────────────────────────────────────┘
```

### Populate (PUT) Path

Same as `full` profile:
1. Client sends key + CUDA IPC handle via gRPC
2. Dispatcher opens IPC handle, evicts DRAM if needed
3. `cudaMemcpy` (D2H): GPU → memory-tier slot
4. Entry registered in dispatch-map; acknowledgement sent to client
5. Background writer asynchronously persists to SSD (write-through)

### Lookup (GET) Path — Warm

Same as `full` profile:
1. Client sends key + destination IPC handle via gRPC
2. Dispatch-map lookup returns MemoryTier pointer
3. `cudaMemcpyAsync` (H2D): memory-tier → GPU (via dedicated CUDA stream)
4. Stream handle returned; client synchronizes before accessing data

### Lookup (GET) Path — Cold (Local SSD)

Same as `full` profile:
1. Dispatch-map lookup returns BlockDevice offset
2. Pipelined NVMe reads into memory-tier slot (8-deep ring buffer)
3. Simultaneously streams chunks to GPU via async CUDA DMA
4. Entry promoted back to memory-tier for future warm access

### Lookup (GET) Path — Remote Miss

When a key is not found locally (neither DRAM nor SSD):
1. Dispatcher forwards the miss to RemoteLookup component
2. RemoteLookup queries peer Certus nodes via RDMA
3. If a peer has the entry, data is transferred via RDMA Write into local memory
4. Entry is optionally cached locally for future access

### Incoming Remote Request (Peer → This Node)

1. RemoteRequestHandler receives RDMA request from a peer
2. Resolves the key against the local dispatcher (acquires read reference)
3. Returns a zero-copy LookupRef (pointer into memory-tier)
4. RDMA Write sends data to the requesting peer
5. Read reference released after RDMA Write completes

## 3. Components in This Profile

### Standard Components (same as `full`)

| Component | Interface | Description |
|-----------|-----------|-------------|
| SPDKEnvComponent | ISPDKEnv | DPDK/EAL init, VFIO device discovery |
| LoggerComponent | ILogger | Console/file logging |
| GpuServicesComponent | IGpuServices | CUDA DMA, stream management |
| EvictionPolicyLruComponent | IEvictionPolicy | LRU eviction policy |
| DispatchMapComponent | IDispatchMap | Key→location index with reference counting |
| MemoryTierComponent | IMemoryTier | mmap'd DRAM pool with LRU |
| BlockDeviceSpdkNvme | IBlockDevice, IBlockDeviceAdmin | SPDK userspace NVMe driver |
| ExtentManager | IExtentManager | Crash-consistent space allocator |
| DispatcherComponent | IDispatcher | Central orchestrator |

### Remote-Specific Components

| Component | Interface | Description |
|-----------|-----------|-------------|
| RemoteLookupComponent | IRemoteLookup | Client-side: forwards local misses to peer nodes |
| RemoteRequestHandlerComponent | IRemoteRequestHandler | Server-side: handles incoming RDMA lookup requests |

### RemoteLookupComponent (`components/remote-lookup/`)

Forwards cache misses to peer Certus nodes in the cluster.

```rust
define_component! {
    pub RemoteLookupComponent {
        version: "0.1.0",
        provides: [IRemoteLookup],
        receptacles: { logger: ILogger },
    }
}
```

**IRemoteLookup methods:**

| Method | Description |
|--------|-------------|
| `batch_lookup(entries)` | Query peer nodes for (key, IpcHandle) pairs; returns per-entry results |
| `join_cluster(endpoint)` | Connect to the cluster at the given address |
| `leave_cluster()` | Disconnect from remote peers |

**Error type:** `RemoteLookupError` — `NotFound`, `TransportError(String)`

### RemoteRequestHandlerComponent (`components/remote-request-handler/`)

Handles incoming RDMA-based cache lookup requests from peer Certus nodes. Resolves keys against the local dispatcher and provides zero-copy data references for RDMA Write.

```rust
define_component! {
    pub RemoteRequestHandlerComponent {
        version: "0.1.0",
        provides: [IRemoteRequestHandler],
        receptacles: {
            logger: ILogger,
            dispatcher: IDispatcher,
        },
    }
}
```

**IRemoteRequestHandler methods:**

| Method | Description |
|--------|-------------|
| `handle_lookup(key)` | Resolve key locally, return zero-copy LookupRef |
| `handle_check(key)` | Check existence without acquiring a reference |
| `handle_batch_lookup(keys)` | Batch resolve, return LookupRefs (each must be released) |
| `release_lookup(key)` | Release read reference after RDMA Write completes |

**Key type — LookupRef:**
```rust
pub struct LookupRef {
    pub ptr: *const u8,    // Pointer into memory-tier pool
    pub size: u32,         // Data size in bytes
    pub key: CacheKey,     // For release_lookup
}
```

The LookupRef holds a read reference in the dispatch-map — the entry cannot be evicted until `release_lookup` is called.

**Error type:** `RemoteRequestHandlerError` — `InvalidRequest`, `KeyNotFound`, `DispatchError`, `NotInitialized`

## 4. Wiring and Initialization Order

1. **LoggerComponent** — console/file logging
2. **SPDKEnvComponent** — DPDK/EAL init, VFIO device discovery
3. **GpuServicesComponent** — CUDA device init
4. **EvictionPolicyLruComponent** — shared LRU policy
5. **DispatchMapComponent** — key→location table (bound to eviction_policy)
6. **MemoryTierComponent** — mmap DRAM pool (bound to eviction_policy)
7. **RemoteLookupComponent** — cluster client (bound to logger)
8. **DispatcherComponent** — orchestrator (bound to dispatch_map, memory_tier, gpu, spdk_env, remote_lookup)
9. **RemoteRequestHandlerComponent** — RDMA server (bound to dispatcher, logger)

Note: RemoteRequestHandler is initialized **after** the dispatcher because it needs a fully-initialized dispatcher to resolve incoming lookups.

## 5. Concurrency Model

Same as `full` profile, plus:

- **RemoteRequestHandler** operates an async RDMA listener that accepts connections from peer nodes. Each connection runs as a session with its own state machine.
- **LookupRef lifetime**: A read reference is held from `handle_lookup` until `release_lookup`. This pins the memory-tier entry (prevents eviction) during the RDMA Write window. Failure to release blocks eviction.
- **No cross-node locking**: Each node manages its own dispatch-map independently. Consistency is eventual — a populate on node A is not immediately visible to lookups forwarded from node B until the cluster state propagates.

## 6. Key Design Decisions

1. **RDMA for inter-node transfers**: Zero-copy data movement between nodes without CPU involvement on the critical path.
2. **Cooperative caching, not replication**: Nodes do not replicate data. A miss on one node forwards to peers; if no peer has it, the client must recompute.
3. **Read-reference pinning for RDMA**: The LookupRef/release_lookup pattern ensures memory-tier entries are not evicted while RDMA Write is in flight.
4. **Dispatcher-mediated resolution**: Remote requests go through the full dispatcher path (dispatch-map lookup + reference management), reusing all existing concurrency guarantees.
5. **Cluster membership is explicit**: Nodes join/leave via `join_cluster`/`leave_cluster` — no automatic discovery.
