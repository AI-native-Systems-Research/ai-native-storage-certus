# Cutover: remote-lookup onto the transfer endpoint (T065, T066) — DEFERRED

**Status**: designed and decided, **not started**. Deferred on 2026-10-22 so that explicit
cache transfer through llm-d and vLLM comes first; this cleans up remote-lookup afterwards.
T067 (the connector's shared-memory message) is a separate task and not covered here.

Decisions 1–5 below were accepted as recommended. Everything else is the design they imply,
written so the work can be picked up cold.

## What is swapped

remote-lookup uses two components today, both deleted by T066:

| component | used for | calls |
|---|---|---|
| `remote-lookup-rdma-initiator` (serving side) | writing values into a requester's landing slots; warming a connection when a peer joins; stamping our zyre id into the connect | `push_async`, `connect`, `set_local_peer_id` |
| `remote-lookup-rdma-responder` (requesting side) | binding an `rdma_cm` port; registering the whole memory-tier pool once; tearing a peer down to reclaim abandoned slots | `set_bind_ip`, `set_actor_cpu`, `initialize`, `local_endpoint`, `local_region().rkey`, `open_control_channel` (Disconnect / DisconnectAck) |

Never called by remote-lookup: `push` (blocking), `disconnect`, `disconnect_all`,
`signal_stop`, `shutdown`.

`certus-transfer-endpoint` does all of it as one component:

| today | after |
|---|---|
| `initiator.push_async(endpoint, [(key, RemoteRegion)], cb)` | `transfer.push_async(endpoint, same items, cb)` — unchanged shape (FR-019); the callback receives `FetchResult` = per-key `EntryOutcome` |
| `responder.local_region().rkey` | `transfer.landing_region()?.rkey`. remote-lookup already whispers raw pool addresses plus one pool-wide rkey, which is exactly what a `LandingRegion` describes |
| `responder.local_endpoint()` (ephemeral port) | the configured `"host:port"` (decision 1) |
| `initiator.connect(endpoint)` on zyre ENTER | `transfer.connect(endpoint)` — new method (decision 3) |
| `Disconnect { node }` + 500 ms wait for `DisconnectAck`, keyed by zyre id, silently abandoned on timeout | `transfer.disconnect("host:port")`, which returns only once no write from that peer can still land (FR-017) |
| `initiator.set_local_peer_id(uuid)` | gone: the endpoint names peers by `host:port`, carried in its own connect private data (D24) |
| `responder_admin.{set_bind_ip, set_actor_cpu, initialize}` | `transfer_admin.configure(EndpointConfig { local, actor_cpu, .. })` then `initialize()` |

## Decisions

1. **The listen port is configured.** `LookupConfig` gains `rdma_port: u16`, set from
   `CERTUS_RDMA_PORT`, default 7481, documented beside `CERTUS_RDMA_BIND_IP` in
   `full-remote.yaml`. (Not 4791, RoCEv2's UDP port, which an `rdma_cm` port number is easily
   confused with; not 7471, `tools/rdma-test`'s default, which runs on the same nodes.) Several
   instances on one node need one port each; the k8s deployment sets it per pod. The
   endpoint's rule stands: a peer is `host:port`, port 0 is refused (FR-021, D16).
2. **A blank `CERTUS_RDMA_BIND_IP` still auto-detects.** The "first active RDMA device's
   IPv4" helper (`first_active_rdma_ipv4`, responder `rdma.rs:76,240-244`) moves into
   certus-transfer-endpoint's verbs code as a public function, behind `rdma`.
   `init_remote_lookup` calls it when the variable is blank and passes the result into
   `EndpointConfig::local`. `EndpointConfig` itself never accepts an empty host.
3. **`ITransferEndpoint::connect(peer)` is added**: start a connect if there is no link or
   attempt, return at once (`ConnectionTable::ensure`). Warming keeps today's first-request
   latency. Interface change → every implementor (one) and a re-stamp of every component.
4. **remote-lookup owns the endpoint's lifecycle**, as it does the responder's today:
   receptacles `transfer: ITransferEndpoint` and `transfer_admin: ITransferEndpointAdmin`
   replace `initiator`, `responder`, `responder_admin`. The `full-remote` profile wires the
   endpoint's own receptacles: `dispatch_map`, `memory_tier`, `logger`, and with `spdk`
   `dispatcher`. remote-lookup's `shutdown` now also calls `transfer_admin.shutdown()` (the
   responder was never shut down).
5. **remote-lookup keeps promoting disk keys before it serves.** `serve_rdma_request`
   batch-promotes BlockDevice keys with `promote_to_memory_tier` and pushes from memory, so
   the endpoint's own `read_to_bounce` path is not exercised by remote-lookup. Changing that
   (serve from disk without promoting, as the endpoint does for `fetch`) is a separate,
   measured change; it is dispatcher align-task RH-03.

## The boundary mapping (`server.rs`)

`map_push_status(PushStatus)` becomes `map_outcome(EntryOutcome)`:

| `EntryOutcome` | wire `RdmaStatusCode` | why |
|---|---|---|
| `HitMemoryTier`, `HitBlockTier` | `Success` | the write completed before the callback |
| `Miss` | `KeyNoLongerAvailable` | absent or wrong size — today's `KeyNotFound` / `SizeMismatch` |
| `Failed` | `UnableToConnect` | the only wire status meaning "not delivered"; it covers a lost link, a refused or flushed write, and a failed lookup |

A `push_async` returning `Err` drops its callback; `ServeReport`'s `Drop` already reports
`UnableToConnect` for that, so it stays.

## Teardown (`actor.rs`)

- The actor learns each peer's `host:port` from its zyre ENTER header (`rl_rdma_ep`) and
  keeps `HashMap<PeerId, String>`; EXIT removes the entry *after* using it.
- `teardown_peer(peer)` becomes `transfer.disconnect(&endpoint_of(peer))`. It blocks until
  the drain finishes (normally microseconds; bounded by the endpoint's 5 s drain stall), so
  the 500 ms poll loop and `DISCONNECT_ACK_TIMEOUT` are deleted — and with them the silent
  give-up, which today can reclaim a slot a late write may still land in.
- A peer whose ENTER carried no endpoint has never been exposed to a slot (we only whisper
  to peers we can name), so it needs no teardown; assert that.
- `connection_teardown_timeout` (`CERTUS_RL_TEARDOWN_MS`) keeps its meaning: how long an
  orphaned slot waits for a late status before the peer is torn down.

## Interfaces (T066)

Removed from `components/interfaces`: `iremote_lookup_rdma_initiator.rs`,
`iremote_lookup_rdma_responder.rs` — `IRemoteLookupRdmaInitiator`,
`IRemoteLookupRdmaResponder`, `IRemoteLookupRdmaResponderAdmin`, `PushStatus`,
`PushCompletion`, `ControlChannel`, `ResponderCommand`, `ResponderEvent`, the responder's
`Endpoint`, `LocalRegion`, their error types, and their re-exports. `RemoteRegion` moves into
`itransfer_endpoint.rs` (its existing comment anticipates this) and stays re-exported under the
same name, so its users only change an import path.

Added: `ITransferEndpoint::connect` (decision 3). `LookupConfig` gains `rdma_port`.

## Files

| area | change |
|---|---|
| `components/remote-lookup` | receptacles; `initialize` (configure + initialize the endpoint, cache `landing_region().rkey` and the local `host:port`); `server.rs` mapping; `worker.rs` warm via `connect`; `actor.rs` teardown and the peer→endpoint map; `seams.rs`: `MockInitiator` + `MockResponder` + `MockResponderAdmin` → one `MockTransferEndpoint` (push semantics as today, a `disconnects()` log, `landing_region` over the world's pool); `tests/mesh.rs` wiring; `tests/mesh_rdma.rs` over two real endpoints; `Cargo.toml` `rdma` feature → `certus-transfer-endpoint/rdma` |
| `components/certus-transfer-endpoint` | `connect`; the auto-detect helper; README and CLAUDE.md lose their "unlike the initiator/responder" contrasts |
| `apps/certus-server-yaml` | `full-remote.yaml` (components, wiring, `init_order`: `certus_transfer_endpoint` after `memory_tier`/`dispatch_map`, before `remote_lookup`); `hooks.rs` (`CERTUS_RDMA_PORT`, auto-detect); `Cargo.toml` (`rdma = ["dep:certus-transfer-endpoint", "certus-transfer-endpoint/rdma", "dep:zyre"]`); `build.rs` refuses `full-remote` without `rdma` (decision 7 below) |
| workspace | `Cargo.toml` members, default-members comments, path deps; delete both crate directories and their `.specify/` |
| build and deploy | the feature string `spdk,rdma,remote-lookup-rdma-initiator/rdma,remote-lookup-rdma-responder/rdma` in `scripts/build-certus-full-remote-spdk.sh:10`, `deploy/k8s/Dockerfile:64-73`, `deploy/k8s/build-image-and-push.sh:54` becomes `spdk,rdma`; `scripts/test-full-remote-multinode.sh` gains `CERTUS_RDMA_PORT` |
| docs | `design/profiles/full-remote/*` (SYSTEM.md, deployment, .puml/.svg, hit and put flows), `design/profiles/full{,-optimized}/SYSTEM.md`, `knowledge/components/{remote-lookup,interfaces}.md`, delete `knowledge/components/remote-lookup-rdma-{initiator,responder}.md`, `knowledge/size-mismatch-handling.md`; remote-lookup's `specs/002-remote-lookup-rdma` (amend, revise in place) |
| specs here | spec.md FR-019/FR-020 and SC-002/SC-008 become met rather than deferred; tasks.md T065/T066 ticked |

## Other consequences, accepted

6. **A teardown has a wider blast radius.** One connection carries both directions (D22), so
   reclaiming an orphaned slot from peer P also aborts the pushes this node is serving *to*
   P on that connection. They fail loudly (`UnableToConnect`) and P's lookup retries. Today the
   two directions are separate connections and a teardown touches only one.
7. **A server built without `rdma` must not run remote lookup.** Without `rdma` the endpoint
   is its in-process mock, which would report plausible numbers from no fabric (Principle XI).
   `build.rs` refuses the `full-remote` profile unless `rdma` is on, as it already refuses
   block-device profiles without their features.
8. **Promotion stays a peer-triggered cost** (decision 5): serving a remote lookup still
   evicts the server's own DRAM to make room, exactly as today. The endpoint's non-promoting
   path is the fix, deliberately not taken here.

## Validation

1. remote-lookup unit and `tests/mesh.rs` over `MockTransferEndpoint`: every existing test,
   including `stuck_orphan_is_force_reclaimed_after_teardown_timeout` (now asserting a
   `disconnect(host:port)` of the right peer) and `warms_connections_to_discovered_peers`
   (asserting `connect`).
2. A new test: an orphan's slot is not reclaimed until `disconnect` has returned — the
   ordering the old 500 ms give-up broke.
3. `tests/mesh_rdma.rs` over two real endpoints on node2's NIC, loopback.
4. `scripts/test-full-remote-multinode.sh` node2 ↔ node5 (needs llm-d and certus stopped on
   node5); compare remote-hit counts and bytes with the last pre-cutover run.
5. Full workspace suite with the gdrcopy `LD_LIBRARY_PATH`, exit code checked; then re-stamp
   every component (interfaces changes).

## Order of work

1. `interfaces`: `connect`, `rdma_port`; `RemoteRegion` moved (old modules still present).
2. certus-transfer-endpoint: `connect`, auto-detect helper.
3. remote-lookup re-pointed (T065), tests green over the mock, then `mesh_rdma` on hardware.
4. certus-server-yaml profile, hook, `build.rs` refusal; multinode run.
5. T066: delete both crates and the three interfaces; scripts, deploy, docs.
6. Re-stamp every component last.
