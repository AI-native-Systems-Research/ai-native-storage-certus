The purpose of this component is to **push** locally-cached values to peer Certus
instances over an RDMA network. It is the server-side (data-holding) half of a
remote lookup: a peer that wants data sends its keys and the descriptors of the
memory it wants the data written into; this component looks the keys up in the
local memory tier and RDMA-writes matching values directly into the peer's
memory.

The control-plane messaging (transporting the keys and the requester's remote
memory descriptors, and the accept side of the RDMA connection) is handled by the
`remote-lookup` component over zyre. This component is invoked by `remote-lookup`
through the `IRemoteLookupRdmaInitiator` interface and is responsible only for the
outbound RDMA data path.

## Interface

`IRemoteLookupRdmaInitiator`:

- `push(endpoint, items)` where `items` is a list of `(CacheKey, RemoteRegion)`
  and `RemoteRegion` is the remote `(addr, rkey, length)`. Ensures a connection
  to `endpoint` (an `"ip:port"` string), looks each key up in the local memory
  tier via `IMemoryTier::peek`, and, if present with a matching size, RDMA-writes
  the value into the remote region. Returns one status per item:
  `Success` / `UnableToConnect` / `KeyNotFound` / `SizeMismatch`.
- `disconnect(endpoint)` — tear down a single host's connection. Used, e.g., when
  a host is known to have left the cluster. Note the many-to-one relationship
  between discovery-layer peers and hosts: only disconnect once the host (not
  merely one peer) is gone.
- `disconnect_all()` — tear down all connections.

## Connection table

Connections are held in a table keyed by the normalized `"ip:port"` endpoint. A
host absent from the table is *disconnected*; an entry is *connecting*,
*connected*, or *disconnecting*. Establishing an RDMA connection over RoCE with
CM was measured to take more than two seconds, so connections are established
lazily and reused across calls.

Calls are re-entrant: the connection table uses a brief lock to look up/insert a
per-host slot, and each slot has its own lock. Pushes to different hosts proceed
concurrently; pushes to the same host serialize on that host's slot (required
because a queue pair is not safe for concurrent use). If a cached connection's
queue pair is found in the error state — or an in-flight write fails — the
connection is torn down and rebuilt once, and the batch is retried.

## Memory registration

To RDMA-write from the memory-tier pool, the pool (base + size from
`IMemoryTier::pool_info`) is registered as an RDMA memory region once per
connection. Writes issue from the pointer returned by `IMemoryTier::peek`, which
lies within that registered region.

## Telemetry

Optional (feature `telemetry`), wired into the push path. When the feature is
disabled the collector is a zero-sized no-op, so call sites cost nothing. Metrics
tracked: outbound connections established / failed, reconnects (QP-error repairs),
disconnects, push batches and average push duration, per-item outcomes mirroring
`PushStatus` (success / key-not-found / size-mismatch / unable-to-connect), and
total bytes RDMA-written (with a throughput helper). Read them via
`RemoteLookupRdmaInitiatorComponent::telemetry()`. The `push_telemetry` Criterion
benchmark (`benches/`) measures the feature-on vs feature-off overhead against
the < 5% budget (spec-002 SC-004); see the README "Benchmark" section.

## Known limitations / follow-ups

- **Accept side lives in `remote-lookup`.** For `push`'s `rdma_connect` to
  succeed, the requesting node must run an `rdma_cm` listener and pre-register its
  receive memory with remote-write access, then communicate the endpoint and
  `RemoteRegion`s. That is the `remote-lookup` component's responsibility.
- **Eviction race.** `peek` returns a pointer/size without pinning the entry
  against eviction; an eviction + reallocation between `peek` and write
  completion could change the bytes (the pointer stays within the registered
  pool, so this is a data-freshness concern, not memory safety). Pinning
  (dispatch-map read reference or a memory-tier pin API) will be added when
  integrating with `remote-lookup`.

---

# Planned architecture: initiator / responder / shared registration

> **Status:** design agreed 2026-07-10; **not yet implemented.** The current code
> above is the *initiator half only*, under the older `remote-lookup-rdma-initiator`
> name. This section records the target shape so implementation can resume from
> the settled design rather than re-derive it. It spans three components plus the
> `remote-lookup` protocol; this component is the natural home because it becomes
> the initiator and the responder is its sibling.

## Component split

Three components, with all ibverbs/rdma_cm confined to two of them and **none** in
`remote-lookup`:

- **`remote-request-rdma-initiator`** (rename of this component). Active/connect
  side: given `{endpoint, rkey, [(addr,len)…]}`, connects out and RDMA-writes
  values from the local tier. The current `push`/`disconnect`/`disconnect_all`
  interface is already initiator-shaped, so the rename carries no interface churn.
- **`remote-lookup-rdma-responder`** (new). Passive/accept side: binds an
  ephemeral port, exposes *locked* landing slots in the local tier, returns
  `{ip, port, rkey, [(addr,len)…]}`, and accepts the initiator's connection. The
  `run_responder` scaffolding in `loopback_test.rs` is a working prototype to
  promote; its test-only accept-side FFI (`rdma_bind_addr`/`listen`/`accept`)
  becomes this component's real FFI, and the loopback test becomes a genuine
  initiator+responder integration test.
- **A tier-registration owner** (`memory_tier` or a close sibling). Registers the
  whole memory tier as one RDMA MR at startup and exposes
  `{context, PD, base_addr, lkey, rkey}` (see constraints below). Both initiator
  and responder borrow it; **neither registers memory per request.** This is the
  linchpin that keeps the responder small — "prepare landing" becomes "pick a tier
  slot, return its addr + the tier rkey," with no `ibv_reg_mr` on the hot path.

`remote-lookup` stays control-plane only (zyre membership, source selection,
moving descriptors) and touches no verbs. It wires receptacles to both RDMA
components on each side.

## Role inversion

Data roles and RDMA connection roles are opposite. The side that *receives* the
data is the side that *binds and listens*, because an RDMA write is one-sided and
the writer needs the destination `addr`+`rkey` up front.

| Instance | Data role | RDMA connection role | CM calls |
|----------|-----------|----------------------|----------|
| Requesting (wants the value) | client | **responder** (accept) | `rdma_bind_addr` → `rdma_listen` → `rdma_accept` |
| Serving (holds the value) | server | **initiator** (connect) | `rdma_resolve_addr` → `rdma_resolve_route` → `rdma_connect` |

## Ephemeral port

One Certus instance runs **per NUMA domain**, so multiple instances share a host.
A zyre "node" is therefore an *instance*, not a host: several nodes share one host
IP. The responder binds `port 0` (rdma_cm assigns an ephemeral port), reads it back
via `rdma_get_src_port`, and advertises it in its request message. The port is what
distinguishes co-resident instances on a shared NIC; do **not** use a well-known
port (co-resident responders would collide). The loopback test hard-codes a port
only because it has no control plane to negotiate one.

## Protocol (shout → whisper)

1. A local request misses both memory and disk tiers for a subset of keys.
2. remote-lookup zyre-**shouts** the missing keys. Each peer replies with the keys
   it holds and, per key, the **tier** (memory/disk) and **length**.
3. remote-lookup picks a source per key (prefer memory-tier holders; multiple peers
   may hold a key → they are failover candidates), allocates **locked** landing
   slots in the local tier, and zyre-**whispers** each chosen peer
   `{local-ip, ephemeral-port, rkey, [(addr,len)…]}`.
4. Each peer validates and RDMA-writes, then whispers back a **status vector** (one
   entry per requested item): `Success | UnableToConnect | KeyNotFound | LengthMismatch`.
5. The requester unlocks filled slots; for each failure it retries another holder
   or drops the incomplete tier entry.

The status vector *is* the completion signal — the requester never observes an
RDMA completion of its own, so `RDMA_WRITE_WITH_IMM` is unnecessary.

## Invariants (load-bearing — get these wrong and you corrupt memory)

- **Validate before writing.** The serving initiator checks *key-still-present* and
  *value length ≤ advertised slot length* **before** posting any RDMA write.
  Because the whole tier is a single MR, the NIC only bounds-checks at *tier*
  granularity — an over-length write to one slot would silently corrupt its
  neighbor. This software length check is the **sole** enforcement of per-slot
  boundaries.
- **Non-success ⇒ zero bytes written.** A `KeyNotFound`/`LengthMismatch` status must
  guarantee nothing was written to that slot; this is what makes failover and
  retries idempotent and safe.
- **Reply after completions.** A peer sends its status vector only *after* all its
  RDMA write completions for that whisper are reaped. This is what makes
  "free-on-reply" safe on the requester (below).
- **Free landing slots on exactly two triggers — never a third:**
  1. *Whisper reply received* → R is done writing (per the invariant above); unlock
     successes, discard failures. Per-operation, no QP teardown, no disturbance to
     other concurrent fills sharing the connection.
  2. *Zyre EXIT for R* → tear down the shared per-node QP to R, **then** reclaim
     *all* slots pending on R. Collateral-free precisely because every operation on
     that QP is already doomed by the EXIT.

  **Never** free a slot on a per-operation timeout while R is still a live group
  member. That single case both reintroduces the late-write use-after-free *and*
  is the only thing that would force a mid-flight teardown of a shared QP.
- **QP-teardown-before-reclaim.** On give-up, destroy (or transition to ERROR) the
  responder-side QP to R **before** reclaiming its slots. An RDMA RC write needs a
  live QP; a destroyed QP NAKs R's late writes so they cannot land. This is the hard
  barrier that makes reclamation safe even when the departure was a zyre false
  positive and R is still alive.
- **Per-node connection state machine** (`Active → Draining → Dead`). QPs are
  per-remote-node and shared across concurrent fills. On EXIT, enter `Draining` and
  refuse new fills to R (route them to another holder) so nothing races a fresh push
  onto a QP being destroyed. Mirrors the initiator's existing `ConnectionTable`.

### Identity correlation: mapping a zyre node to its QP

The RDMA connection is established out-of-band over the RoCE fabric via `rdma_cm`,
entirely separate from zyre. On `CONNECT_REQUEST` the responder sees only the
initiator's RDMA **source IP**, which cannot identify the peer — co-resident serving
instances share a host IP and differ only by an arbitrary source port. So there is
no way to tie an inbound QP to a zyre `PeerId` unless the identity is carried in the
handshake.

**Fix: the serving initiator stamps its own zyre UUID into the `rdma_cm` connect
`private_data`.** That blob (≥56 bytes on RC/RoCE — ample for a 16-byte UUID) arrives
verbatim on the responder's `CONNECT_REQUEST`. The responder reads it and keys its
per-node connection table (the `Active → Draining → Dead` state machine) by
`PeerId`. On zyre EXIT, `remote-lookup` calls `disconnect(node: PeerId)`; the
responder resolves `PeerId → cm_id/QP` in *its own* table and tears it down.

Consequences:
- **`remote-lookup` never holds the mapping and never touches a QP.** It deals only
  in `PeerId`s (which it has from zyre); the correlation lives inside the responder,
  populated from `private_data`. This is what keeps verbs and connection handles out
  of `remote-lookup`.
- **New requirement on the initiator:** it must know its *own* local zyre `PeerId` to
  stamp it — supplied once at init/config (stable for the process), not per-push. Its
  local `remote-lookup` (which owns zyre and handles the inbound whisper) is the
  natural source.
- **Stamping is mandatory.** A connection arriving with absent/garbage `private_data`
  yields `Event::ConnectionEstablished { node: None }`, so `disconnect(PeerId)` can't
  find it — it is reclaimable only via the wedged-node backstop or `shutdown`'s
  `disconnect_all`. "All initiators MUST stamp their UUID" is a protocol invariant.

### Wedged-but-alive backstop

If R stops replying but its zyre heartbeat persists, do **not** free slots early.
Either accept that slots stay locked until zyre expires R (bounded by zyre's expiry,
tens of seconds), or escalate the local stall to a *node-level* teardown
(teardown-before-reclaim, triggered locally). Per-operation memory windows are the
only way to revoke at *slot* granularity without a node teardown — not needed given
the two-trigger rule.

## Shared tier registration constraints

- **Shared MR ⇒ shared PD ⇒ shared device context.** Trivial on a single NIC (one
  context). For NIC-per-NUMA, the registrar must key PD/MR **by device** and hand
  out a region *handle*, not a bare global rkey, because rdma_cm picks a
  connection's context by IP routing and it must match the PD the MR was registered
  on.
- **Never pin a device by name.** Bind/connect by IP and let routing imply the
  device. Same code path for single-NIC and NIC-per-NUMA; only the advertised IP
  changes.
- **rkey blast radius.** One tier-wide MR means the rkey handed to a peer authorizes
  writes *anywhere* in the tier. Acceptable on a trusted fabric; memory windows
  (`ibv_bind_mw`) could scope per-request rkeys later — and would also supply the
  per-operation revocation the backstop above otherwise lacks.

### Measured registration cost (single-MR decision input)

Today the responder registers the whole pool **once** at `initialize`, while the
initiator re-registers the whole pool **per connection** (`connection.rs`
`RealTransport::connect` → `register_existing_mr`). Whether to instead **share one
MR/PD** between responder and initiator (and across the initiator's connections)
hinges on how costly a full-pool `ibv_reg_mr` is. Measured on this platform
(mlx5, RoCE; `LOCAL_WRITE|REMOTE_WRITE`; median of 7; harness
`tests/mr_registration_bench.rs`, `CERTUS_MR_BENCH_HUGE=1` for the hugepage row):

| pool | 4 KiB pages | 1 GiB hugepages |
|------|-------------|-----------------|
| 16 MiB | 0.74 ms | — |
| 64 MiB | 2.9 ms | — |
| 256 MiB | 10.6 ms | — |
| 1 GiB | 37.8 ms | **3.5 ms** |
| 2 GiB | 49.6 ms | **7.0 ms** |
| 4 GiB | 97 ms | **14.0 ms** |

Registration is **linear in pool size** in both modes: ≈40 µs/MiB (≈38 ms/GiB) on
4 KiB pages, ≈3.4 µs/MiB (≈3.5 ms/GiB) on 1 GiB hugepages. Hugepages cut it ~10×
but — notably — do **not** make it free: the cost is not purely CPU-PTE-count
bound (a 1 GiB pool is one CPU PTE), it is the kernel pinning the range plus the
NIC populating its address-translation table, both linear in region size.

**Conclusion:** with a hugepage-backed pool (the realistic production case, and
what this box uses — 1 GiB hugepages) the per-connection re-registration is ~3.5
ms/GiB. warm-at-discovery moves it off the serve hot path but does not remove it,
and it recurs on every reconnect. So sharing one MR/PD (initiator reuses the
responder's already-registered region — its PD + `lkey` — instead of calling
`ibv_reg_mr`) is:
- **not worth the coupling for small/moderate pools** (≤ a few GiB): a few ms
  per peer, hidden by warming, versus a raw-`ibv_pd` handle crossing the
  component boundary + coupled MR/PD teardown lifetimes + NIC-per-NUMA handling
  (see "Shared tier registration constraints" above);
- **worth it for large pools** (tens of GiB → hundreds of ms/connection, ×
  mesh fan-out × reconnects).

Break-even is therefore a **pool-size × mesh-size** judgement, not an
unconditional win. Tracked as a gated task (remote-lookup 002 T040), currently
**recommend deferring** unless deployments use very large per-node pools.

## Keyspace

- **Namespace keys** by model/tenant id — avoids cross-model key collisions (two
  models generating the same key for different values) and provides multi-tenant
  privacy isolation.
- **Encode value length in the key.** A differing length is then simply a different
  key, collapsing `LengthMismatch` into `KeyNotFound`, and the requester can size
  landing slots directly from the key (making the shout's length a redundant
  cross-check). **Keep the server-side `≤` bound check regardless:** with length in
  the key it is a should-never-fire assertion, but it is the cheap last line that
  turns a key-derivation bug or hash collision into a clean error instead of a
  neighbor-corrupting overrun. KV-cache value length is generally fixed per model,
  so `LengthMismatch` is not an expected runtime event — it exists for memory
  safety, not graceful degradation.

## NUMA locality (performance, not correctness)

With a single shared NIC, an instance pinned to NUMA node N registers tier memory on
node N while the NIC may sit on another node's PCIe root; writes DMA across the
inter-socket link (UPI/Infinity Fabric). Correct and transparent, but single-NIC
data-path numbers *include* this cross-NUMA hop and should not be read as the
ceiling — expect it to shrink once instances bind NUMA-local NICs.
