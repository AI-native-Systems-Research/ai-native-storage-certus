# Research: Remote Lookup over Zyre + RDMA

**Feature**: 002-remote-lookup-rdma | **Phase**: 0 (outline & research)

This resolves the technical unknowns and cross-component dependencies for turning the
`remote-lookup` placeholder into the real zyre + RDMA cache-fill client/server. The RDMA
transport prerequisites are already built and validated on this branch (see
[Prerequisites](#prerequisites-already-satisfied)).

## Prerequisites (already satisfied on this branch)

- **`IRemoteLookup::batch_lookup`** takes `&[(CacheKey, u32 /* size */)]` (the `IpcHandle`
  parameter is gone); `Ok(())` means the key is resident in the local memory tier.
- **Responder is the registrar**: `remote-lookup-rdma-responder` registers the whole memory-tier
  pool (`REMOTE_WRITE`) and exposes `IRemoteLookupRdmaResponder::local_region() -> LocalRegion {
  addr, rkey, length }`.
- **`IMemoryTier`** is available without the `spdk` feature; both RDMA crates gate their real
  path behind an `rdma` cargo feature (mock seams by default).

## Decision 1 — Actor event-loop multiplexing

**Decision**: One actor thread runs a **poll loop** that, each iteration, (1) drains new
`batch_lookup` submissions from a caller→actor channel, (2) drains inbound zyre events via
`IZyreNode::try_recv()`, (3) drains responder control events (`ResponderEvent`) via `try_recv`,
(4) advances per-operation deadlines/phase transitions, then (5) sleeps for a short bounded
interval (≈500 µs–1 ms) only when fully idle.

**Rationale**: `IZyreNode` exposes `try_recv()` (non-blocking) but **no pollable fd**, so a
`select`/`epoll` over {zyre, submissions, timers} is not available. A bounded poll loop is the
simplest correct multiplexer. This is the **control plane** only; end-to-end latency is dominated
by the one-sided RDMA transfer, so a sub-millisecond poll tick is negligible (SC-001).

**Alternatives rejected**: (a) blocking `IZyreNode::recv()` — can't also service submissions and
deadlines; (b) epoll over a zyre socket fd — not exposed by the `IZyre` wrapper and would couple
us to czmq internals.

## Decision 2 — Caller ↔ actor handoff

**Decision**: `batch_lookup` (called on dispatcher threads) packages the entries into an
`OperationRequest { op_id, entries, done: Sender<Vec<Result<(), RemoteLookupError>>> }` and sends
it on an MPSC submission channel to the actor, then **blocks** on the paired one-shot receiver
until the actor finalizes that operation. The actor owns all per-operation state; callers never
touch it. Multiple concurrent `batch_lookup`s are independent operations keyed by `op_id`
(FR-020).

**Rationale**: Keeps all mutable operation state single-threaded (no locks on the hot correlation
path), while `batch_lookup` stays synchronous to its caller as the interface requires. Uses
`component_core::channel` MPSC/SPSC primitives already used elsewhere.

**Alternatives rejected**: shared-memory op table with locks (reintroduces cross-thread races the
actor model removes).

## Decision 3 — Wire message encoding

**Decision**: Hand-rolled, little-endian, explicitly-framed encoding in a `wire` module — no
`serde`/`bincode`. Every message is `[version: u8][msg_type: u8][op_id: u64]` followed by a
type-specific payload of length-prefixed records (`key: u64`, `size: u32`, availability tag: u8,
`RemoteRegion { addr: u64, rkey: u32, length: u32 }`, endpoint string as `len: u16` + UTF-8 bytes).
Unknown `msg_type` is logged and ignored (FR-018).

**Rationale**: Messages are small and fixed-shape; a hand-rolled codec honors the constitution's
minimal-dependency principle, gives full control of the versioned frame, and avoids a serde
dependency in a control-plane component. Encode/decode are unit-testable in isolation.

**Alternatives rejected**: `bincode`/`serde` (extra deps, less control over the explicit version
byte); protobuf (heavy, and the sibling initiator's old protobuf design was already removed).

## Decision 4 — Operation identity and correlation

**Decision**: `op_id: u64` from a process-wide `AtomicU64` counter. KEY_RESPONSE and RDMA_STATUS
echo the `op_id`; a reply whose `op_id` is not in the active-operation map is discarded (FR-019).
A batch exceeding the configured max keys per KEY_QUERY is split into multiple SHOUTs under one
`op_id` (FR-005); responses aggregate by `op_id`.

## Decision 5 — Publish-on-success landing slots + actor-side single-flight

**Decision**: A landing slot is reserved **privately** in the requester's own
responder-registered memory-tier pool for the fill's duration and is **published to dispatch-map
only after the RDMA transfer succeeds** — never held there as a write-locked placeholder.

- **Reserve** (before RDMA): `memory_tier.insert(key, size)` yields a pool `addr`. No dispatch-map
  entry is created. The slot is advertised to the peer as `RemoteRegion { addr, rkey, length }`.
- **Publish** (on RDMA_STATUS Success): `dm.create_memory_tier_entry(key, addr, len)` (sets
  `write_ref=1`; the DRAM is already fully filled) → `dm.release_write(key)` (readable). A racing
  same-key publish sees `AlreadyExists` and treats it as success (self-heal).
- **Discard** (on failure / peer Exit mid-fill): `memory_tier.remove(key)` only — dispatch-map was
  never touched, so there is no `dm.remove` and no blocked-reader wakeup race.

**Single-flight (SC-008)** lives in the **actor**, not in dispatch-map: a per-key in-flight index
(`HashMap<CacheKey, InFlight { serving_op, followers }>`) coalesces a second same-key
`batch_lookup` into a follower of the in-flight operation — it blocks on the same fill and issues
no duplicate query/RDMA. A local reader that hits dispatch-map directly during the sub-millisecond
fill sees `NotExist` and bounces back through the dispatcher → actor, where it is deduped as a
follower (it blocks on the fill; it never recomputes).

**Dependency (D1) — NOT NEEDED (resolved 2026-07-13).** The earlier design held a write-locked
dispatch-map placeholder across the fill, which required dispatch-map to (a) `notify_all` on
`remove` and (b) atomically abort a write-held placeholder (`abort_fill`) — plus a pending/valid
flag so blocked readers never observed uncommitted bytes. Publish-on-success removes that entire
requirement: the entry only ever exists in dispatch-map *fully filled*, and the failure/peer-exit
path never creates one, so there is no removal-vs-blocked-reader race to fix. **No dispatch-map
code change is made for this feature.** (`define_interface!` discards default method bodies, so an
`abort_fill` method would also have forced updating ~8 `IDispatchMap` impls; avoided.)

## Decision 6 — Server-side identity stamping (initiator local PeerId)

**Decision**: The serving node's **initiator stamps its own zyre UUID into the `rdma_cm` connect
`private_data`** so the requester's responder can key its connection table by `PeerId` (needed for
teardown-before-reclaim, FR-014). The initiator must therefore know its own local `PeerId`.

**Dependency (D2) — CONFIRMED (verified 2026-07-13)**: `remote-lookup-rdma-initiator`'s
`rdma::client_connect` hard-codes `conn_param.private_data = null, private_data_len = 0`, and there
is **no local-`PeerId` concept anywhere** in the crate. Because the responder correlates peers by
reading that `private_data` on `CONNECT_REQUEST`, every connect currently lands as `node: None`
(unidentified) — so teardown-before-reclaim keyed by `PeerId` (FR-014) cannot target a specific
peer. **Required initiator change**: an admin `set_local_peer_id(PeerId)` (supplied by
`remote-lookup` from its own zyre uuid) plus stamping that id into `conn_param.private_data` inside
`client_connect`. Small, gated behind the initiator's `rdma` feature. MVP prerequisite for identity
correlation + teardown (FR-014).

## Decision 7 — Scope: memory + disk hits + retry (US4/US5 folded in; D3 resolved)

**Decision**: The feature implements **memory-tier hits, disk-tier fallback, and multi-round
retry** — User Stories 1 (remote memory hit), 2 (answer queries), 3 (serve RDMA), 4 (disk
fallback), 5 (retry), 6 (completion/timeout), and 7 (peer-departure safety). Delivery is still
incremental (memory path first), but US4 and US5 are **in scope**, not a separate later increment.

**US5 (retry)** was only ever deferred for increment-sizing; it re-targets a still-unsatisfied key
to an **already-cached** alternate peer and reissues an RDMA_REQUEST entirely inside the actor
(reusing `PeerReply` + the same `push` path). No cross-component dependency; it depends only on the
Operation state machine + finalization.

**US4 (disk fallback)** needs a serving node to promote a disk-only entry into its RDMA-registered
memory pool before pushing. Investigation found this capability **already exists on `IDispatcher`**:
`fn promote_to_memory_tier(&self, keys: &[CacheKey])` (`interfaces/src/idispatcher.rs:657`),
implemented by **both** `dispatcher` (`lib.rs:2361`) and `dispatcher-p2p` (`lib.rs:2329`). It reads
each `BlockDevice{offset}` entry from SSD into a fresh memory-tier slot (via
`pipelined_ssd_to_dram_only`) and re-registers it as `MemoryTier{pointer,size}` in the dispatch-map;
it is synchronous, best-effort (per-key errors logged, not propagated), and idempotent for keys
already resident.

**Dependency (D3) — RESOLVED (no interface or dispatcher code change).** The serving path is:
1. On an RDMA_REQUEST naming a key that `dm.lookup` reports as `BlockDevice{..}`, call
   `dispatcher.promote_to_memory_tier(&[key])` (batch all such keys in the request).
2. Re-`dm.lookup(key)`: if now `MemoryTier{ptr,size}`, pin + `push` as for a memory hit; if still
   not memory-resident (promotion failed/evicted), whisper `RDMA_STATUS(KeyNoLongerAvailable)`
   (FR-017). No new return value is needed — the dispatch-map is the source of truth for the
   promoted pointer.
3. remote-lookup gains one receptacle, `dispatcher: IDispatcher`. It already has `dispatch_map` +
   `memory_tier`, so no other new plumbing.

**D3 caveat — Arc reference cycle + teardown contract.** `Receptacle` holds a strong `Arc<T>`
(`component-core/src/receptacle.rs:44`) and `dispatcher-p2p` already binds `remote_lookup`
(`dispatcher-p2p/src/lib.rs:145`). Adding `remote-lookup → dispatcher` closes a strong cycle
(`dispatcher-p2p ⇄ remote-lookup`) that would leak both components at teardown. Mitigation: the
mainline MUST call `Receptacle::disconnect()` (`receptacle.rs:110`) on one direction of the pair at
deactivate/shutdown. This is a wiring/lifecycle contract for the integrating app, not a dispatcher
code change. remote-lookup does **not** take a raw `IBlockDevice` receptacle (that would duplicate
drive ownership + DMA/pipeline plumbing the dispatcher already owns).

## Decision 8 — Testing strategy

**Decision**: Two layers. The default layer uses **real zyre** for transport/discovery and mocks
only the NIC; the hardware layer adds real RDMA.

- **Default (no NIC) — multi-node real-zyre mesh**: a reusable `TestMesh` fixture spawns **N ≥ 4**
  full remote-lookup instances in one process, each wired to the **real `zyre` component** and
  discovering the others via **gossip over TCP loopback** (`tcp://127.0.0.1:…`, one hub `bind`, the
  rest `connect` — no UDP multicast, no runner interference). Only the RDMA `initiator`/`responder`
  receptacles and the local-state receptacles (`memory_tier`/`dispatch_map`/`dispatcher`) are mocks:
  channel-backed RDMA seams and a **scriptable** per-node holdings table (memory/disk/absent per
  key, plus scripted eviction between a KEY_RESPONSE and the later RDMA_REQUEST). This exercises the
  full protocol (SHOUT → KEY_RESPONSE → RDMA_REQUEST → RDMA_STATUS), single-flight,
  completion/timeout, peer-exit teardown, and — critically — the **client+server dual role under
  concurrency**: all nodes run `batch_lookup` while simultaneously answering peers, which a
  two-instance ping-pong cannot reach.
- **Hardware (`rdma` feature, `#[ignore]`)**: the same multi-node fixture over real localhost zyre +
  **single-host RDMA loopback** (distinct ephemeral ports, proven by the initiator/responder
  loopback tests), replacing the mock RDMA seams with the real crates.

**Determinism**: real zyre whisper/discovery timing is nondeterministic, so tests assert
**timing-robust invariants** — every satisfiable key ends `Satisfied` or (on quorum/timeout)
`NotFound`; no RDMA_REQUEST is ever sent to a peer that did not advertise that key at that size; a
key reported by exactly one memory holder is always fetched from that holder; retry-cap and the
quorum/`op_deadline` parameters are honored. Precise routing that depends on reply order (e.g. "which
of two equal holders served key X") is **not** asserted. Determinism where needed is obtained by (a)
a **discovery barrier** (wait until every node sees the others via `peers_by_group`/ENTER before the
first SHOUT) and (b) **app-level reply delays** in the mock server (sleep before whispering
KEY_RESPONSE) sized well above transport jitter so the intended ordering dominates.

**Rationale**: There is no longer a plain build/test CI to keep hermetic (only `creusot-sync` and
`kani-sync-verify` workflows remain), and remote-lookup is already outside `default-members` and
SPDK-bound (Decision 10) — so its suite already requires the full local toolchain, and the real
`zyre` native libs are built here (`deps/zyre-build`, reproducible via `deps/build_zyre.sh`). Real
zyre therefore costs nothing we have not already spent and buys genuine message ordering plus the
dual-role concurrency coverage that is the whole point. The NIC stays mocked because the RDMA data
path is validated separately in the `remote-lookup-rdma-*` components; local state stays a scriptable
mock because staging "node2 holds K on disk, evicts it before the RDMA_REQUEST" needs precise control
that four real DMA pools cannot easily give. Structural SC-003 (greedy dispatch on the same event) is
asserted over the mock RDMA seam.

## Decision 9 — `op_deadline`: block-vs-recompute tradeoff (configurable, default 50 ms)

**Decision**: `op_deadline` bounds how long `batch_lookup` blocks a requester before finalizing an
unsatisfied key as `Err(NotFound)`. It is a **`LookupConfig` field, configurable, default 50 ms**
(SC-002). The value should be *derived from measurement*, not guessed; 50 ms is the MVP placeholder
until the measurement below is run (deferred — not on the MVP critical path).

**Rationale**: On a *total* miss the dispatcher returns `KeyNotFound` (dispatcher-p2p
`lib.rs:1830`) and the **inference engine above the dispatcher recomputes the KV entry** —
recompute is the client's fallback, not something remote-lookup does. So the real tradeoff is *how
long to wait for a remote hit before giving up and letting the client recompute*, i.e. exactly
`op_deadline`. It is **orthogonal to the publish/D1 choice**: the requester blocks identically
regardless of Decision 5 (the landing slot only affects *other* local readers).

- KV recompute is expensive (causal-prefix dependency → re-prefill the preceding context; ms to
  100s of ms) versus a sub-millisecond one-sided RDMA fetch (SC-001), and `lookup_async` overlaps
  the fetch on a warm CUDA stream (dispatcher-p2p `lib.rs:1837-1858`). ⇒ **for a genuine hit, always
  block and fetch** — recompute is strictly the last resort.
- Because a bounced local reader is deduped into a *waiting follower* (Decision 5) and never
  recomputes, publish-on-success carries **no recompute risk** of its own.

**Derivation methodology (deferred measurement)**: (1) measure representative KV-recompute
(prefill) latency across token spans / prefix lengths; (2) measure RDMA fetch + end-to-end
`batch_lookup` latency (`profile-hardware-ceiling` + a two-instance loopback harness); (3) set
`op_deadline` at the crossover — keep waiting while `E[remaining fetch] < recompute cost`, capped
by the tail-latency SLO. Because the field is configurable, an operator can retune it per platform
without a code change.

**Alternatives rejected**: hardcoding a single deadline (not portable across NIC/model
combinations); returning `NotFound` immediately on first miss (throws away the sub-ms-fetch win and
forces expensive recompute on transient contention).

## Decision 10 — `interfaces` SPDK gating: assume SPDK enabled (short-term); ungate later

**Context (discovered at implementation time; corrects an earlier plan claim).** The plan/tasks
asserted the *only* `interfaces`-crate change for 002 was `initialize`/`LookupConfig` and that D3
"needs no interface change." That is not true against the code as it stands: `IDispatchMap`,
`LookupResult` (`idispatch_map.rs`), and `IDispatcher` (`idispatcher.rs`) are all
`#[cfg(feature = "spdk")]`-gated. remote-lookup needs those trait *definitions* to declare its
`dispatch_map` and `dispatcher` receptacles (T007, T029), so they must be visible to it.

The gate is **organizational, not technical**: those signatures reference no SPDK/DPDK type (only
`CacheKey`, `u32`/`u64`, `*mut u8`, and `GpuStream`/`IpcHandle`, which are already exported
un-gated). Every existing implementor (dispatch-map, dispatcher, dispatcher-p2p) already enables
`interfaces/spdk` unconditionally, so gating the trait defs alongside them cost nothing — there was
simply never a non-SPDK consumer until remote-lookup. The DRAM the interfaces manage *is* real
DPDK-hugepage / NUMA-pinned / RDMA-registerable memory (`memory-tier` does one large
`spdk_zmalloc(..., node_id, SPDK_MALLOC_DMA)`), but that is a **runtime** property of the bound
implementation; across the interface remote-lookup only ever holds an opaque `*mut u8`.

**Decision (short-term, chosen 2026-07-15 to prioritize shipping remote-lookup over refactoring a
shared crate).** remote-lookup **enables `interfaces/spdk`** (mirrors memory-tier/dispatch-map/
dispatcher) to gain visibility of the gated traits. Because `interfaces/spdk = ["dep:spdk-sys"]`,
this transitively pulls `spdk-sys`, so remote-lookup is **removed from the workspace
`default-members`** and now requires a built SPDK at `deps/spdk-build/` (like its storage peers).
The SPDK-free `cargo build` stays green (remote-lookup is built explicitly with
`cargo build -p remote-lookup`). The mock-seam CI tests (Decision 8) still run without hardware, but
now require the SPDK libraries to be *present* to link.

**Consequence / assumption to revisit.** This sets aside the plan's "SPDK-orthogonal, stays in
`default-members`" goal for remote-lookup. It is documented as an assumption, not a permanent design.

**Future work (the SPDK-orthogonal fix).** Ungate `IDispatchMap`, `LookupResult`, and `IDispatcher`
from the `spdk` feature — exactly what was already done for `IMemoryTier` in commit `db7f70a` (a
pure-Rust trait with no SPDK types). Ungating the trait *definitions* does not force the impl crates
out of their own `spdk` gates and does not pull SPDK into any consumer. Once done, remote-lookup can
drop `features = ["spdk"]`, rejoin `default-members`, and its mock-seam tests run on an SPDK-free
runner. Tracked as **D4** below.

## Summary of cross-component dependencies

| ID | Dependency | Status | Blocks |
|----|------------|--------|--------|
| D1 | dispatch-map placeholder abort/notify | **NOT NEEDED — dropped by publish-on-success (Decision 5)** | — |
| D2 | initiator: `set_local_peer_id` + stamp it into `rdma_cm` `private_data` (`client_connect`) | **confirmed — done** (commit `e77c4a5`) | identity correlation, teardown (FR-014) |
| D3 | serving-node disk→memory promotion | **RESOLVED — `IDispatcher::promote_to_memory_tier` already exists (Decision 7); no interface/dispatcher change** | US4 disk fallback |
| D4 | `IDispatchMap`/`LookupResult`/`IDispatcher` are `spdk`-gated in `interfaces` | **DEFERRED — short-term: remote-lookup enables `interfaces/spdk` + leaves `default-members` (Decision 10); future: ungate the trait defs (like `IMemoryTier` in `db7f70a`)** | remote-lookup consuming dispatch-map/dispatcher receptacles |

D1 is resolved without any cross-component change (Decision 5). D2 is already implemented on this
branch. D3 is satisfied by an existing `IDispatcher` method — the only new coupling is a
`dispatcher` receptacle on remote-lookup plus a teardown-disconnect contract (Decision 7). D4 is the
`spdk`-gating of the consumed trait definitions: resolved short-term by enabling `interfaces/spdk`
in remote-lookup (which therefore leaves `default-members`), with the SPDK-orthogonal ungate tracked
as future work (Decision 10). **No cross-component *code* change is required for any of US1–US7;** the
D4 short-term resolution is a Cargo-manifest/feature change plus the workspace `default-members` edit.
