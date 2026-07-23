# remote-lookup

**Crate**: `remote-lookup`
**Path**: `components/remote-lookup/`
**Version**: 0.1.0

## Description

Resolves cache keys that miss locally by fetching them from other Certus nodes. The dispatcher forwards entries not found in the local memory tier or on SSD to `batch_lookup`, which locates the value on a peer and pulls it in. Coordination runs over **zyre** (group SHOUT to discover who holds a key, directed WHISPER for the exchange); the value moves out of band over **RDMA** as a one-sided write into a landing slot the requester reserves in its own memory tier. The heavy RDMA work is delegated to the sibling `remote-lookup-rdma-initiator` / `remote-lookup-rdma-responder` components; this component owns the protocol and state machine. All mutable state lives on a single actor poll-loop thread that owns the zyre node; callers submit work over an MPSC channel and block on a per-operation one-shot. Feature `002-remote-lookup-rdma`.

## Component Definition

```
RemoteLookupComponent {
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
    fields: {
        submit_tx: OnceLock<MpscSender<ActorMsg>>,   // → actor
        actor: Mutex<Option<ActorHandle>>,           // joined on deactivate
        op_counter: AtomicU64,                        // op_id source
        config: OnceLock<LookupConfig>,
        peers_seen: Arc<AtomicUsize>,                 // ENTER−EXIT (test barrier)
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IRemoteLookup {
        fn initialize(&self, config: LookupConfig) -> Result<(), RemoteLookupError>;
        fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>>;
        fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>;
        fn leave_cluster(&self) -> Result<(), RemoteLookupError>;
    }
}
```
```

## Verified Properties

None. No formal verification model exists for this component.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `zyre` | `IZyre` | Yes | Discovery + signalling; the actor creates/joins the node. |
| `dispatch_map` | `IDispatchMap` | Yes | Classify/serve keys; publish fetched values. |
| `memory_tier` | `IMemoryTier` | Yes | Reserve landing slots for inbound RDMA writes. |
| `dispatcher` | `IDispatcher` | No | Promote disk-only keys before serving (US4). |
| `initiator` | `IRemoteLookupRdmaInitiator` | Yes | One-sided RDMA writes to a peer. |
| `responder` | `IRemoteLookupRdmaResponder` | Yes | Local pool registration + endpoint/rkey. |
| `responder_admin` | `IRemoteLookupRdmaResponderAdmin` | Yes | Responder lifecycle/config. |
| `logger` | `ILogger` | No | Diagnostic logging (skipped if unbound). |

## Key Types

- `RemoteLookupError` — `NotFound`, `TransportError(String)`.
- `LookupConfig` (public `interfaces` type) — `group`, `quorum_pct`, `phase1_timeout`, `op_deadline` (default 50 ms), `max_retry_rounds`, `max_keys_per_query`, `bind_ip`, `actor_cpu`, `discovery: Option<GossipConfig>` (`None` = UDP beacon, `Some` = gossip), `node_endpoint`. Derives `Default`.
- Wire protocol (`pub mod wire`): `WireMessage` (KeyQuery / KeyResponse / RdmaRequest / RdmaStatus / Unknown), `Avail`, `RdmaStatusCode`, `SlotDesc` — hand-rolled little-endian v1 framing.

## Key Design Decisions

- **Actor model**: one poll-loop thread owns the zyre node and all operation state (research Decision 1/2); `batch_lookup` submits over an MPSC channel and blocks on a per-op `std::sync::mpsc` one-shot.
- **Protocol**: SHOUT KEY_QUERY → peers classify each `(key, size)` (memory / disk / not-available; a size mismatch is a miss) → on a memory hit the requester reserves a private landing slot and whispers RDMA_REQUEST (its own responder endpoint + pool rkey + slot addr) → the holder RDMA-writes and whispers RDMA_STATUS.
- **Publish-on-success** (research Decision 5): a fetched value is published to dispatch-map only after RDMA_STATUS(Success); an `AlreadyExists` at a different size is treated as a key collision (unsatisfied, slot reclaimed, existing entry never evicted — see `knowledge/size-mismatch-handling.md`). D1 dropped (no dispatch-map change).
- **Single-flight**: a per-key in-flight index coalesces concurrent fetches of the same key onto one RDMA; followers complete when the fill resolves.
- **Retry (US5)** and **Phase-2 disk fallback (US4)**: a failed fetch re-targets an untried cached peer (memory preferred, then disk), bounded by `max_retry_rounds`; keys satisfiable only from disk are fetched once the memory phase stalls, with the serving peer promoting via `IDispatcher::promote_to_memory_tier`.
- **Completion/timeout (US6)**: an op finalizes when all keys are satisfied, when all expected peers have replied with nothing in flight, or at `op_deadline` (unsatisfied keys → `NotFound`; the client layer may recompute).
- **Teardown-before-reclaim (US7, SC-005)**: a landing slot exposed to a live peer is never reclaimed on a timeout (a late one-sided write could still land); it is orphaned and reclaimed on a late RDMA_STATUS, or when the peer exits after the responder confirms the queue pair is down (`ResponderCommand::Disconnect` → `DisconnectAck`).
- **CPU/DRAM only**: `batch_lookup` takes `(key, size)` pairs (no `IpcHandle`); on success the key becomes resident in the local memory tier and the dispatcher performs DRAM→GPU delivery.
- **SPDK coupling**: consumes `IDispatchMap`/`IDispatcher` (currently `spdk`-gated in `interfaces`), so the crate enables `interfaces/spdk` and is outside the workspace `default-members` (research Decision 10). The `rdma` feature only forwards to the initiator/responder crates.
- **Lifecycle contract**: wiring `dispatcher.remote_lookup` + `remote_lookup.dispatcher` forms an Arc cycle; the integrating mainline must `Receptacle::disconnect()` one side at teardown (research Decision 7).
- **Testing** (research Decision 8): a multi-node (N ≥ 4) in-process mesh over the **real** zyre component (gossip on TCP loopback) with the NIC and local state mocked (`remote_lookup::seams`); `#[ignore]` hardware loopback swaps in the real RDMA crates.
