# Data Model: Remote Lookup over Zyre + RDMA

**Feature**: 002-remote-lookup-rdma | **Phase**: 1 (design)

All state below lives on the **single actor thread** unless noted; callers interact only through
the submission channel + one-shot completion (research Decision 2). Types are internal to the
`remote-lookup` crate except where they mirror `interfaces` value types.

## Component declaration

```text
provides:    IRemoteLookup
receptacles: zyre: IZyre,
             dispatch_map: IDispatchMap,
             memory_tier: IMemoryTier,
             dispatcher: IDispatcher,                            // US4 disk fallback: promote_to_memory_tier
             initiator: IRemoteLookupRdmaInitiator,
             responder: IRemoteLookupRdmaResponder,
             responder_admin: IRemoteLookupRdmaResponderAdmin,   // lifecycle (bind/init/shutdown)
             logger: ILogger                                     // optional
fields:      config: LookupConfig,
             submit_tx: OnceLock<Sender<ActorMsg>>,              // caller -> actor
             actor: Mutex<Option<ActorHandle>>,                  // join handle + stop flag
             op_counter: AtomicU64                               // op_id source
```

Keeping remote-lookup SPDK-orthogonal: it depends only on the interface traits above (all
available without the `spdk` feature) — never on any implementation crate.

**`dispatcher` receptacle (US4)**: used only by the serving side to promote a disk-only key
(`promote_to_memory_tier` → re-`dm.lookup`, research Decision 7). Because `dispatcher-p2p` already
binds `remote_lookup`, this closes a strong `Arc` cycle; the mainline MUST `disconnect()` one
direction at teardown (Decision 7 caveat). The receptacle is optional at the type level — a
memory-only deployment that never serves disk hits can leave it unbound.

## Configuration — `LookupConfig` (FR-022)

Defined as a **public type in the `interfaces` crate** (like `DispatcherConfig`), derives `Default`,
and is supplied via `IRemoteLookup::initialize(LookupConfig)`. The `config` field above is populated
by `initialize`; `initialize` also spawns the actor and drives responder bring-up (FR-025). Built by
the `certus-server-yaml` `init_remote_lookup` hook with `..Default::default()` (YAML-robust).

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `group` | `String` | `"remote_lookup"` | zyre group joined on activation (FR-003) |
| `quorum_pct` | `u8` | 80 | % of group peers replied that triggers Phase-1→2 (FR-006a/010) |
| `phase1_timeout` | `Duration` | 20 ms | Phase-1 cap before falling through |
| `op_deadline` | `Duration` | 50 ms | block-before-`NotFound` bound (SC-002); configurable, derive from measurement (research Decision 9) |
| `max_retry_rounds` | `u32` | 2 | retry-round cap (FR-011) |
| `max_keys_per_query` | `usize` | 256 | KEY_QUERY split threshold (FR-005) |
| `bind_ip` | `String` | — | RoCE IPv4 handed to the responder admin |
| `actor_cpu` | `Option<usize>` | `None` | NUMA pin for the actor + responder loop |
| `discovery` | `Option<GossipConfig>` | `None` | peer-discovery mode → `NodeConfig.gossip`; `None` = UDP beacon; `Some` = gossip (cross-subnet clusters + the in-process test mesh over TCP loopback) |
| `node_endpoint` | `Option<String>` | `None` | this node's ZRE mailbox → `NodeConfig.endpoint`; required when `discovery` is `Some` |

## `Operation` (one `batch_lookup`, keyed by `op_id`)

| Field | Type | Notes |
|-------|------|-------|
| `op_id` | `u64` | correlation id echoed by peers |
| `keys` | `Vec<(CacheKey, u32)>` | the requested `(key, size)` entries, positional |
| `status` | `Vec<KeyStatus>` | per-key: `Unsatisfied` / `InProgress { peer }` / `Satisfied` / `Failed` |
| `replies` | `HashMap<PeerId, PeerReply>` | cached KEY_RESPONSEs for phase-2/retry (research D5) |
| `phase` | `Phase` | `Memory` → `DiskFallback` (US4, research Decision 7) |
| `retry_round` | `u32` | bounded by `max_retry_rounds` |
| `peers_expected` | `usize` | group size snapshot at SHOUT (for quorum) |
| `peers_replied` | `usize` | distinct peers whose KEY_RESPONSE arrived |
| `deadline` | `Instant` | now + `op_deadline` |
| `done` | `Sender<Vec<Result<(), RemoteLookupError>>>` | one-shot back to the blocked caller |

**State transitions (per key)**: `Unsatisfied` --(RDMA_REQUEST sent)--> `InProgress{peer}`
--(RDMA_STATUS Success)--> `Satisfied`; `InProgress` --(RDMA_STATUS fail | peer Exit)-->
`Unsatisfied` (retry-eligible); at finalization any non-`Satisfied` → `Failed` (`Err(NotFound)`).

**Finalization (FR-012)** at first of: all `Satisfied`; no cached peer holds any remaining key and
no more replies expected; `retry_round == max_retry_rounds`; or `deadline` elapsed. On finalize:
discard any unpublished landing slots (`memory_tier.remove`), send `done`, drop the `Operation`.

## `PeerReply` (cached KEY_RESPONSE from one peer)

| Field | Type | Notes |
|-------|------|-------|
| `peer` | `PeerId` | zyre uuid of the responder |
| `endpoint` | `Endpoint` | the peer's responder `{ip, port}` to connect the initiator to |
| `rkey` | `u32` | that peer's pool-wide rkey (advertised) — unused by requester; kept for symmetry |
| `memory` | `HashMap<CacheKey, u32>` | keys the peer holds in memory (with stored size) |
| `disk` | `HashMap<CacheKey, u32>` | keys the peer holds on disk (used by US4 disk fallback) |

Dropped when the peer emits a zyre `Exit` (FR-013).

## `LandingSlot` (private reserve → publish-on-success)

A slot in the requester's own responder-registered memory-tier pool that receives one
RDMA-written value. It is reserved **privately** for the fill's duration and is **not published to
dispatch-map until the transfer succeeds** (research Decision 5) — so a failed or peer-interrupted
fill leaves no dispatch-map entry to race on. This drops dependency D1 entirely (no dispatch-map
change).

| Field | Type | Notes |
|-------|------|-------|
| `key` | `CacheKey` | |
| `addr` | `u64` | DRAM slot address inside the requester's responder-registered pool |
| `len` | `u32` | value length (== requested size) |
| `peer` | `PeerId` | peer currently serving it (for teardown-before-reclaim) |

**Reserve** (before RDMA): `memory_tier.insert(key, size)` yields `addr` inside the pool the local
responder registered `REMOTE_WRITE` at startup. **No dispatch-map entry is created yet.** The slot
is advertised to the serving peer as `RemoteRegion { addr, rkey: <pool rkey>, length: len }`, where
`rkey` is the single value cached from `responder.local_region()` at startup (FR-007).

**Publish** (on RDMA_STATUS Success): `dispatch_map.create_memory_tier_entry(key, addr, len)` (sets
`write_ref=1` — the fill is already complete in DRAM) → `dispatch_map.release_write(key)`
(`write_ref=0`, entry becomes readable). A concurrent fetch that raced a just-published entry sees
`create_memory_tier_entry → AlreadyExists` and treats it as success (self-heal).

**Discard** (on failure / peer Exit mid-fill): `memory_tier.remove(key)` only — nothing was ever
published to dispatch-map, so there is no `dispatch_map.remove` and no blocked-reader wakeup race.

**Single-flight** (SC-008) is enforced **in the actor**, not by a dispatch-map placeholder: a
per-key in-flight index (`HashMap<CacheKey, InFlight { serving_op, followers: Vec<op_id> }>`)
coalesces a second same-key `batch_lookup` into a *follower* of the in-flight operation — it blocks
on the same fill and never issues a duplicate query/RDMA. Physical reclaim of a slot that was
exposed to a peer which then departed waits for `ResponderEvent::DisconnectAck` (FR-014, SC-005).

## `WireMessage` (framed; research Decision 3)

Header (all): `[version: u8 = 1][msg_type: u8][op_id: u64 LE]`.

| `msg_type` | Message | Payload |
|-----------|---------|---------|
| 1 | `KeyQuery` (SHOUT) | `count: u32`, then `count × (key: u64, size: u32)` |
| 2 | `KeyResponse` (whisper) | requester `endpoint`, `count: u32`, then `count × (key: u64, size: u32, avail: u8)` where avail ∈ {0 none, 1 memory, 2 disk} |
| 3 | `RdmaRequest` (whisper) | requester `endpoint`, `rkey: u32`, `count: u32`, then `count × (key: u64, addr: u64, length: u32)` |
| 4 | `RdmaStatus` (whisper) | `count: u32`, then `count × (key: u64, status: u8)` where status ∈ {0 success, 1 unable-to-connect, 2 key-no-longer-available} |

`endpoint` encodes as `ip_len: u16`, `ip: utf8`, `port: u16`. Unknown `msg_type` → logged + ignored
(FR-018). A `KeyResponse`/`RdmaStatus` with an unknown `op_id` → discarded (FR-019).

## Actor control messages — `ActorMsg`

| Variant | From | Meaning |
|---------|------|---------|
| `Submit(OperationRequest)` | `batch_lookup` caller | start a new operation |
| `Join(String)` / `Leave` | `join_cluster`/`leave_cluster` | gossip bind/connect the zyre node |
| `Shutdown` | deactivate | stop the loop, drain, join |

Inbound zyre `ZyreEvent`s (Shout/Whisper/Exit/Enter) and responder `ResponderEvent`s
(`DisconnectAck`, errors) are polled inside the loop (research Decision 1), not sent as `ActorMsg`.
