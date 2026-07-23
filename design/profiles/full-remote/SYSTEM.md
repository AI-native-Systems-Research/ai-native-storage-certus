# Certus System Architecture — `full-remote` Profile

This document describes the architecture of the Certus `full-remote` deployment: a distributed GPU KV-cache offloading cluster where multiple nodes cooperate via RDMA to serve cache lookups.

## 1. What is the `full-remote` Profile?

The `full-remote` profile extends the base `full` profile with **inter-node cache cooperation**. Each Certus node operates a local SPDK NVMe + DRAM cache and additionally participates in a peer mesh:

- **Discovers peers automatically** via the Zyre gossip/beacon layer (no static peer list)
- **Forwards local misses** to peers: the node broadcasts a key query and offers a landing region in its own RDMA-registered memory for a peer to write into
- **Serves peers that miss**: when a peer asks for a key this node holds, this node RDMA-**writes** the value directly into the peer's offered memory

This enables a cluster of Certus nodes to function as a shared distributed cache — a miss on one node may be a hit on another, avoiding expensive GPU recomputation.

### The push model (who writes into whose memory)

Remote transfer is **one-sided RDMA WRITE**, always performed by the node that *holds* the data:

- The **requester** (the node that missed) never reads a peer's memory. It registers a landing slot in its own memory and advertises `(endpoint, rkey, slot)` to peers. Its **responder** owns that registered memory and the RDMA accept loop.
- The **data-holder** (the node that has the value) connects out to the requester and RDMA-writes the value straight into the advertised slot. Its **initiator** performs that outbound push.

So on any single node the two RDMA components play opposite roles depending on the direction of a given transfer:

| Component | Role | Data direction |
|-----------|------|----------------|
| RemoteLookupRdmaResponder | passive accept side; registers this node's memory pool, runs the `rdma_cm` accept loop, hands out the pool `rkey` | data flows **into** this node (this node is the requester) |
| RemoteLookupRdmaInitiator | outbound push side; connects to a peer and RDMA-writes matching values into the peer's memory | data flows **out of** this node (this node is the data-holder) |

## 2. High-Level Data Flow

```
┌──────────────┐   gRPC/IPC    ┌─────────────────────────────────────────────┐
│  GPU Client  │◄─────────────►│              certus-server                   │
│  (vLLM)      │               │  (Dispatcher + Remote Cache Layer)            │
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

    ┌──────────────────────────────────────────────────────────────────────┐
    │  Remote Cache Layer                                                   │
    │                                                                      │
    │  ┌────────────────────┐   drives   ┌───────────────────────────────┐ │
    │  │ RemoteLookup       │───────────►│ RdmaInitiator (push out)      │ │
    │  │ (orchestrator:     │            │ RdmaResponder (accept in)     │ │
    │  │  key query + slot  │───────────►│ Zyre       (peer discovery)   │ │
    │  │  reservation)      │            └───────────────┬───────────────┘ │
    │  └─────────┬──────────┘                            │                 │
    │            │  KeyQuery (SHOUT) / RdmaRequest (WHISPER over Zyre)      │
    └────────────┼──────────────────────────────────────┼─────────────────┘
                 │              RDMA fabric              │
                 ▼                                       ▼
         ┌───────────────────────────────────────────────────────┐
         │  Peer Certus Nodes (same full-remote)                 │
         └───────────────────────────────────────────────────────┘
```

### Populate (PUT) Path

Same as `full` profile — **local-only**, no remote propagation:
1. Client sends key + CUDA IPC handle via gRPC
2. Dispatcher opens IPC handle, evicts DRAM if needed
3. `cudaMemcpy` (D2H): GPU → memory-tier slot
4. Entry registered in dispatch-map; acknowledgement sent to client
5. Background writer asynchronously persists to SSD (write-through)

Peers discover the entry only later, when they query for its key.

### Lookup (GET) Path — Warm

Same as `full` profile:
1. Client sends key + destination IPC handle via gRPC
2. Dispatch-map lookup returns MemoryTier pointer
3. `cudaMemcpyAsync` (H2D): memory-tier → GPU (via dedicated CUDA stream)
4. Stream handle returned; client synchronizes before accessing data

### Lookup (GET) Path — Cold (Local SSD)

Same as `full` profile:
1. Dispatch-map lookup returns BlockDevice offset
2. Pipelined NVMe reads into memory-tier slot (8-deep ring buffer, 2 CUDA streams)
3. Simultaneously streams chunks to GPU via async CUDA DMA
4. Entry promoted back to memory-tier for future warm access

### Lookup (GET) Path — Remote Miss (this node is the requester)

When a key is not found locally (neither DRAM nor SSD):
1. Dispatcher forwards the miss to the RemoteLookup orchestrator via `batch_lookup(&[(key, expected_size)])`
2. RemoteLookup reserves a landing slot in its own responder-registered memory and SHOUTs a `KeyQuery` to peers over Zyre
3. A peer that holds the key WHISPERs back; RemoteLookup WHISPERs an `RdmaRequest` carrying its `endpoint`, pool `rkey`, and slot descriptors
4. The holding peer connects to this node's responder and RDMA-**writes** the value directly into the reserved slot
5. The data (now in local memory) is optionally promoted into the memory-tier/dispatch-map for future warm access, then DMA'd to the client GPU

### Incoming Remote Request (this node is the data-holder)

1. This node receives a peer's `KeyQuery` over Zyre (`handle_key_query`) and, if it holds the key, receives the peer's `RdmaRequest` (`handle_rdma_request`) carrying the peer's endpoint, `rkey`, and slot descriptors
2. RemoteLookup dispatches an `InitiatorCmd::Serve` to its off-loop initiator worker
3. The RdmaInitiator connects to the requesting peer, resolves the value in the local memory-tier, and RDMA-**writes** it into the peer's advertised memory
4. Completion returns to the actor as `PushComplete`; no read-reference pinning protocol is involved (the value is copied out one-sided)

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

| Component | Interface(s) | Description |
|-----------|--------------|-------------|
| ZyreComponent | IZyre | Peer discovery factory (gossip/beacon); creates `IZyreNode` handles |
| RemoteLookupComponent | IRemoteLookup | Orchestrator: drives discovery, key queries, slot reservation, and push |
| RemoteLookupRdmaInitiatorComponent | IRemoteLookupRdmaInitiator | Outbound push / data-holder side: RDMA-writes values into peer memory |
| RemoteLookupRdmaResponderComponent | IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin | Passive accept / requester side: registers this node's pool, runs the accept loop |

### ZyreComponent (`components/zyre/`)

Peer discovery. `IZyre` is a **factory** — `ping()` and `create_node(NodeConfig) -> Box<dyn IZyreNode>`. All peer operations (`start/stop/join/leave/shout/whisper/recv/peers/…`) live on the single-threaded `IZyreNode` handle. Discovery is automatic: `NodeConfig` defaults to a UDP **beacon**; setting `gossip: Some(GossipConfig)` switches to ZeroMQ gossip over an explicit hub for cross-subnet clusters. `join`/`leave` operate on named **groups**, not individual peers — peers ENTER/EXIT the mesh automatically.

```rust
define_component! {
    pub ZyreComponent {
        version: "0.1.0",
        provides: [IZyre],
        receptacles: {},
    }
}
```

### RemoteLookupComponent (`components/remote-lookup/`)

The orchestrator for the remote path. Owns the Zyre node and the responder control channel on its actor thread, and drives the initiator via an off-loop worker.

```rust
define_component! {
    pub RemoteLookupComponent {
        version: "0.1.0",
        provides: [IRemoteLookup],
        receptacles: {
            zyre: IZyre,
            dispatch_map: IDispatchMap,
            memory_tier: IMemoryTier,
            dispatcher: IDispatcher,
            initiator: IRemoteLookupRdmaInitiator,
            responder: IRemoteLookupRdmaResponder,
            responder_admin: IRemoteLookupRdmaResponderAdmin,
            logger: ILogger,
        },
    }
}
```

**IRemoteLookup methods:**

| Method | Description |
|--------|-------------|
| `initialize(config)` | Brings the responder up, advertises the local RDMA endpoint via Zyre, spawns the initiator worker, and joins the discovery group |
| `batch_lookup(&[(key, size)])` | For each `(CacheKey, u32)` — `size` is the **expected value length in bytes**, not an address — query peers and land the value in local memory; returns per-entry results |
| `join_cluster(endpoint)` | Join a named Zyre group (supplementary to automatic discovery) |
| `leave_cluster()` | Leave the Zyre group |

**Error type:** `RemoteLookupError` — `NotFound`, `TransportError(String)`

> Note: `dispatcher.remote_lookup` and `remote_lookup.dispatcher` form a deliberate `Arc` cycle, broken explicitly at teardown.

### RemoteLookupRdmaInitiatorComponent (`components/remote-lookup-rdma-initiator/`)

The **outbound push / data-holder** side. Given a peer endpoint and a batch of `(key, remote-region)` pairs, it connects to the peer, resolves each key in the local memory-tier, and RDMA-writes matching values directly into the peer's memory. Driven from the "server" side by RemoteLookup.

```rust
define_component! {
    pub RemoteLookupRdmaInitiatorComponent {
        version: "0.1.0",
        provides: [IRemoteLookupRdmaInitiator],
        receptacles: { logger: ILogger, memory_tier: IMemoryTier },
    }
}
```

**IRemoteLookupRdmaInitiator methods:**

| Method | Description |
|--------|-------------|
| `push(endpoint, &[(key, RemoteRegion)])` | Resolve each key locally and RDMA-write matching values into the peer; returns per-item `PushStatus` |
| `connect(endpoint)` | Warm a connection off the hot path |
| `disconnect(endpoint)` / `disconnect_all()` | Tear down connections |
| `set_local_peer_id(PeerId)` | Identify this node to peers |

### RemoteLookupRdmaResponderComponent (`components/remote-lookup-rdma-responder/`)

The **passive accept / requester** side. It belongs to the node that *wants* a value and offers local memory for a peer to write into. It manages only connections — it never touches the data.

```rust
define_component! {
    pub RemoteLookupRdmaResponderComponent {
        version: "0.1.0",
        provides: [IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin],
        receptacles: { logger: ILogger, memory_tier: IMemoryTier },
    }
}
```

**IRemoteLookupRdmaResponder methods:** `open_control_channel() -> ControlChannel`, `local_endpoint() -> Endpoint`, `local_region() -> LocalRegion` (pool-wide `rkey`).

**IRemoteLookupRdmaResponderAdmin methods:** `set_actor_cpu(cpu)`, `set_bind_ip(ip)`, `initialize()` (binds an ephemeral port, registers the whole memory-tier pool `REMOTE_WRITE`, starts the `rdma_cm` accept loop), `signal_stop()`, `shutdown()`.

The `ControlChannel` carries `ResponderCommand::Disconnect{node}` → `ResponderEvent::DisconnectAck{node}` (teardown before slot reclamation), plus `ConnectionEstablished` / `Error` events.

## 4. Wiring and Initialization Order

Per `apps/certus-server-yaml/profiles/full-remote.yaml`:

1. **LoggerComponent** — console/file logging
2. **SPDKEnvComponent** — DPDK/EAL init, VFIO device discovery
3. **GpuServicesComponent** — CUDA device init
4. **EvictionPolicyLruComponent** — shared LRU policy
5. **DispatchMapComponent** — key→location table (bound to eviction_policy)
6. **MemoryTierComponent** — mmap DRAM pool (bound to eviction_policy)
7. **ZyreComponent** — peer-discovery factory
8. **RemoteLookupRdmaInitiatorComponent** — bound to logger, memory_tier
9. **RemoteLookupRdmaResponderComponent** — bound to logger, memory_tier
10. **RemoteLookupComponent** — bound to zyre, dispatch_map, memory_tier, dispatcher, initiator, responder, responder_admin, logger; `init_hook` runs `initialize()`
11. **DispatcherComponent** — orchestrator (bound to dispatch_map, memory_tier, gpu_services, spdk_env, logger, remote_lookup)

During RemoteLookup's `initialize()`:
- The **responder** is brought up first: `responder_admin.set_bind_ip` / `set_actor_cpu` / `initialize()`, then `responder.local_endpoint()`, `responder.local_region().rkey`, `responder.open_control_channel()`
- The local RDMA endpoint is advertised in the Zyre ENTER header (`RDMA_ENDPOINT_HEADER`) so peers warm a connection on discovery
- An off-loop **initiator worker** thread is spawned (handles `Warm` → `initiator.connect` and `Serve` → `initiator.push`)
- The Zyre node is created, started, and joined to the configured group

## 5. Concurrency Model

Same as `full` profile, plus:

- **RemoteLookup actor thread** owns the Zyre node, the responder control channel, and the sole `InitiatorCmd` sender. Its poll loop consumes `ZyreEvent`s (ENTER/EXIT adjust the peer count) and RDMA control events.
- **Responder accept loop** accepts inbound `rdma_cm` connections from data-holding peers writing into this node's registered pool.
- **Initiator worker thread** performs outbound pushes off the actor's hot path.
- **One-sided RDMA WRITE, no read-reference pinning**: the data-holder copies the value out of its own memory-tier into the peer's slot; there is no cross-node pin held on the source entry for the duration of a remote transfer. Slot reclamation on the requester side is coordinated via the responder control channel (`Disconnect` → `DisconnectAck`) on peer EXIT.
- **No cross-node locking**: each node manages its own dispatch-map independently. Consistency is eventual — a populate on node A is not visible to node B until B queries for the key.

## 6. Key Design Decisions

1. **One-sided RDMA push for inter-node transfers**: the data-holder writes directly into the requester's memory with no CPU involvement on the requester's critical path.
2. **Cooperative caching, not replication**: nodes do not replicate data. A miss forwards a key query to peers; if no peer has it, the client must recompute.
3. **Requester offers memory; holder pushes**: the requester's responder registers the landing region and hands out the pool `rkey`; the holder's initiator performs the write. This keeps the data path one-sided and the responder data-agnostic.
4. **Automatic discovery via Zyre**: peers ENTER/EXIT the mesh automatically over UDP beacon (or ZeroMQ gossip across subnets). `join_cluster`/`leave_cluster` operate on named groups and are supplementary, not the primary discovery mechanism.
5. **Dispatcher-mediated resolution**: remote requests resolve against the local memory-tier/dispatch-map, reusing existing concurrency guarantees. The `dispatcher ↔ remote_lookup` `Arc` cycle is intentional and severed at teardown.
