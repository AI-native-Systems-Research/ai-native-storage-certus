# Contract: `ITransferEndpoint`

**Feature**: `001-rdma-cache-transfer` | **Date**: 2026-10-15

Interface trait definition belongs in `components/interfaces/src/itransfer_endpoint.rs`
(Principle I). This document is the contract; the wire encoding is in
[wire-protocol.md](./wire-protocol.md).

## Role asymmetry — read this first

The endpoint is symmetric at the connection level (D5) but **not** at the API level:

| role | how it is driven |
|---|---|
| **destination** (wants values) | an explicit `fetch()` call |
| **source** (holds values) | **reactive** — inbound request messages, no interface call |

There is deliberately **no** `serve()` method. Once initialized, an endpoint answers peer
requests from its own thread by consulting the dispatch map and pushing. Serving is not
something a caller schedules, so exposing it would invite callers to try.

The legacy `push_*` methods are the exception: they are *source-initiated* because zyre has
already delivered the request out of band.

## Peer addressing

`peer` and `endpoint` are the same thing under two names: a **normalized instance address**,
`"ip:port"`. The legacy methods keep the name `endpoint` so their signature compatibility
with `IRemoteLookupRdmaInitiator` stays visible (FR-019); `fetch` uses `peer` because that is
what the spec calls it. Both are parsed by the same grammar.

Identity is per **instance**, not per host (FR-021, research D16): several certus instances
run per node, one per NUMA domain in the current rigs. An address without a port is an
`InvalidEndpoint` error, never a default. Two instances on one host differ only by port, hold
separate connections, and may both be the local host — which is also the single-host loopback
verification path.

## Types

```rust
/// Where a value is to be written on the requesting node.
/// Already exists for the legacy push path; reused verbatim — this is the real
/// definition from `interfaces/src/iremote_lookup_rdma_initiator.rs:62`. Note the
/// field is `length`, NOT `size`, and an earlier draft of this contract had both
/// the name and the field order wrong.
pub struct RemoteRegion {
    /// Remote virtual address to write into (the caller's registered buffer).
    pub addr: u64,
    /// Remote key authorizing the RDMA write into that region.
    pub rkey: u32,
    /// Expected length in bytes; must equal the local value's size.
    pub length: u32,
}

/// Per-entry result, in request order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryOutcome {
    /// Served from the source's memory tier.
    HitMemoryTier,
    /// Served from the source's block tier via a bounce buffer.
    HitBlockTier,
    /// The source does not hold it, holds it at a different size, or cannot
    /// serve it in this build (no `spdk` feature and the entry is block-tier).
    Miss,
    /// Not delivered for a reason other than residency: connection not made
    /// or lost, write refused or flushed, protocol fault, deadline, or the
    /// source's lookup failed. Unlike a miss, asking again may succeed.
    Failed,
}

/// Identifies one in-flight fetch. Echoed in the reply.
pub struct TransferContextId(pub u64);

/// Opaque handle for a region peers may write into.
pub struct LandingRegionHandle(/* private */);

pub enum TransferEndpointError {
    NotInitialized(String),
    InvalidEndpoint(String),
    /// The request exceeds what one message can carry; the caller should chunk,
    /// or let the endpoint chunk internally (FR-015).
    RequestTooLarge { max_entries: usize },
    /// Message credits exhausted and the caller asked not to block.
    WouldBlock,
    /// The deadline expired with the request neither fulfilled nor refused.
    /// Carries diagnostic state (FR-016, Principle XI). The queue pair has been
    /// forced to ERROR and this connection's other contexts were aborted with
    /// it (FR-023).
    Deadline(String),
    /// The peer answered our request with a protocol error: unknown opcode,
    /// unsupported version of a known kind, malformed message, or more entries
    /// than its posted buffer can hold (FR-024). A *reply* we could not process
    /// is a teardown, reported as `Transport`, not this.
    ///
    /// Distinct from `Deadline` in blast radius as well as cause: the peer is
    /// healthy and answered promptly, so **the connection remains usable** and
    /// sibling contexts are unaffected. Carries the offending opcode, the
    /// reason, and the peer's highest supported version for that kind.
    Protocol(String),
    /// The peer's supported protocol version range does not intersect the local
    /// one, so no connection was established and no request was issued
    /// (FR-025). Names peer and both ranges.
    IncompatiblePeer(String),
    Transport(String),
}
```

## Methods

```rust
define_interface! {
    pub trait ITransferEndpoint {
        /// Register a region that peers may RDMA-write into. Must be called
        /// before any peer request can be satisfied against it.
        ///
        /// The region must be SPDK-DMA-able and RDMA-registrable — in practice
        /// allocated by `spdk_zmalloc` on the local NUMA node, as the memory
        /// tier already is (research D15).
        ///
        /// Registration is into the endpoint's long-lived protection domain and
        /// outlives any one queue pair, so the returned handle and the `rkey`s
        /// derived from it stay valid across a reconnect (research D17). This
        /// differs from the predecessor crates, which re-register per
        /// connection.
        fn register_landing_region(
            &self,
            base: *mut u8,
            len: usize,
        ) -> Result<LandingRegionHandle, TransferEndpointError>;

        /// DESTINATION SIDE. Ask one named peer for a key run, to be written
        /// into already-reserved landing slots.
        ///
        /// Blocks if message credits are exhausted (FR-014). Returns as soon as
        /// the request is sent; `on_complete` fires with one `EntryOutcome` per
        /// requested key, in request order, when the reply arrives.
        ///
        /// The endpoint may split the request internally; `on_complete` fires
        /// once, after the last chunk.
        fn fetch(
            &self,
            peer: &str,
            items: &[(CacheKey, RemoteRegion)],
            on_complete: FetchCompletion,
        ) -> Result<TransferContextId, TransferEndpointError>;

        /// LEGACY SUBSET (FR-019). Source-initiated push with no messaging, for
        /// callers whose control plane is elsewhere (`remote-lookup` over zyre).
        /// Signature-compatible with `IRemoteLookupRdmaInitiator::push_async`.
        /// Takes `FetchCompletion`, not the legacy `PushCompletion`: the
        /// latter delivers `Vec<PushStatus>`, so reusing it here would be a
        /// type error once the per-item type is `EntryOutcome`. The argument
        /// shapes are otherwise identical, which is what FR-019 needs.
        fn push_async(
            &self,
            endpoint: &str,
            items: &[(CacheKey, RemoteRegion)],
            on_complete: FetchCompletion,
        ) -> Result<(), TransferEndpointError>;

        /// Blocking convenience over `push_async`. One outcome per item, in order.
        fn push(
            &self,
            endpoint: &str,
            items: &[(CacheKey, RemoteRegion)],
        ) -> Result<Vec<EntryOutcome>, TransferEndpointError>;

        /// Tear down a peer's queue pair. MUST complete before the caller
        /// reclaims any landing slot that peer could still write into
        /// (teardown-before-reclaim, FR-017). This is also the mechanism the
        /// deadline path uses: transition to ERROR, drain completions, then
        /// reclaim (FR-023). Because one connection carries every request to a
        /// peer, this aborts all contexts outstanding on it.
        fn disconnect(&self, peer: &str) -> Result<(), TransferEndpointError>;

        /// Snapshot of the credit domains (per direction) and per-outcome
        /// counters, plus protocol-fault, version-downgrade and
        /// duplicate-connection-resolved counts — an unexercised path must not
        /// look like a working one (Principle XI).
        fn stats(&self) -> TransferStats;
    }
}
```

### Lifecycle: `ITransferEndpointAdmin`

`ITransferEndpoint` carries no lifecycle, so a second interface does, as
`IRemoteLookupRdmaResponderAdmin` does for the responder:

```rust
pub struct EndpointConfig {
    pub local: String,          // this instance's "host:port" — dialed by peers, and its own name
    pub max_send_wr: u32,       // per-connection send-queue depth requested
    pub max_recv_wr: u32,       // per-connection receive-queue depth requested
    pub send_credits: u32,      // in-flight send-queue requests (and so held pins) per connection
    pub actor_cpu: Option<usize>,
}

define_interface! {
    pub ITransferEndpointAdmin {
        fn configure(&self, config: EndpointConfig) -> Result<(), TransferEndpointError>;
        fn initialize(&self) -> Result<(), TransferEndpointError>;
        fn shutdown(&self) -> Result<(), TransferEndpointError>;
    }
}
```

Until `initialize` succeeds, every `ITransferEndpoint` method returns `NotInitialized` — a
real lifecycle state in every build, not the predecessor's "built without `rdma`" bypass.
`initialize` listens as `config.local` and starts the endpoint thread, which owns the
connection table; the `&self` methods reach it as commands. `shutdown` refuses new work,
tears down and drains every connection so all held state is released, then joins the
thread. It is the **only** way to stop a running endpoint: a component holds references to
itself for interface queries, so releasing every handle does not drop it.

## Invariants

Implementations MUST uphold, and tests MUST verify:

1. **Order and length.** `on_complete` receives exactly `items.len()` outcomes, and index
   *i* corresponds to `items[i]`. True across internal chunking.
2. **Outcome honesty.** `HitBlockTier` is reported only when the value was actually read
   from the block device. A build without `spdk` reports `Miss` for block-tier entries, never
   `HitBlockTier` and never an error (research D14). `Miss` and `Failed` are never
   conflated: an entry lost to a fault is `Failed`, never `Miss`, so a broken path cannot
   pass for a cold cache (Principle XI).
3. **No broadcast.** A `Miss` MUST NOT trigger any further peer traffic. This endpoint never
   contacts an instance the caller did not name (D12, SC-006 — note there is deliberately no
   FR for this; it is a property of the mechanism, asserted by SC-006. An earlier draft of
   this invariant mis-cited FR-012, which is the bounce-buffer/no-promotion requirement).
4. **Pin discipline.** Residency and pin come from one dispatch-map lookup. A read reference
   is released exactly once per *pinned* answer, by the completion reaper owning its
   `wr_id`, and never for an unpinned answer (Principle IX, D6, D7).
5. **Pin bound.** Concurrently held read pins never exceed the configured send-queue credit
   limit, regardless of `items.len()` (SC-004).
6. **Tier neutrality.** Serving any entry leaves the source's memory-tier occupancy and LRU
   order unchanged (SC-005, D8). **Not yet met for memory-tier entries:** the dispatch map's
   lookup refreshes LRU position as it pins, and no `IDispatchMap` call pins without it.
   Met once a non-touching lookup exists (D6); block-tier serving never promotes.
7. **No partial publication.** A landing slot whose write did not complete is not published.
   Slots follow reserve → arrive → commit/abort (FR-018).
8. **Teardown before reclaim.** After `disconnect(peer)` returns, no write from that peer
   can land (FR-017). The same ordering holds on the deadline path, where the queue pair is
   forced to ERROR and drained *before* any slot is reclaimed, and a fresh connection is
   established only on next need (FR-023).
9. **Progress under scarcity.** Exhaustion of send-queue credits, message credits or
   bounce buffers causes chunking, never failure (FR-015).
10. **Deadline fails loudly.** On expiry the default action is `Deadline(..)` carrying key
    range, peer, chunk index and queue-pair state — not a silent partial success
    (Principle XI). Expiry aborts every sibling context on that connection, and says so.
11. **No phase barriers.** Within a request the source never drains its send queue between
    entries and never waits for device reads before posting memory-tier writes; each
    block-tier entry is posted when its own read completes (FR-022, D19). The only
    synchronization point is the end-of-message reply.
12. **Instance-granular addressing.** Two peers differing only by port are distinct, hold
    distinct connections, and an address without a port is rejected (FR-021, D16).
13. **Unprocessable requests are answered; replies never are.** A request that cannot be
    processed draws a protocol error reply naming the offending opcode, within a round trip
    rather than a deadline. It fails one context; the connection stays usable and its siblings
    complete (FR-024, SC-012). The error reply is never used for a miss or a resource shortage.
    A reply that cannot be processed is never answered: it is reported loudly and tears the
    connection down through the FR-023 ordering, aborting its contexts, since its sender may
    have written into slots it can no longer vouch for.
14. **Capabilities precede data.** No data message is sent on a connection before the peer's
    capabilities are known. One protocol version governs the connection —
    `min(a.max, b.max)`, failing if below `max(a.min, b.min)` — and the peer's maximum request
    size is respected rather than the local one. A non-intersecting range fails at connect, not
    on a request (FR-025, SC-013). Capabilities are per connection and re-learned when one is
    rebuilt. The rule is **commutative**, so both ends compute the same verdict with no
    arbitrator; each capability message draws a reply that carries nothing and exists only
    to balance credits (SC-014).
15. **One connection, two directions, separate books.** An instance may be destination for one
    context and source for another on the same queue pair. Outbound-request and inbound-serving
    context tables are separate — the requester allocates ids, so one id can be live in both
    directions — message credits are per direction, and posted receive buffers are partitioned
    so a peer at full inbound-request quota cannot starve a reply this end awaits
    (FR-026, SC-016).
16. **Exactly one connection survives a race.** Simultaneous initiation converges on the
    connection whose initiator has the lower `PeerId`, resolved at accept before the capability
    exchange, with both ends agreeing (FR-027, SC-015). Two live connections to one instance
    would defeat `disconnect`, since tearing one down leaves the other able to land a write
    into a reclaimed slot.

## Two fail-loud paths, deliberately different

Both `Deadline` and `Protocol` are loud failures, and the distinction between them is the
point rather than an accident:

| | `Deadline` (FR-016, FR-023) | `Protocol` (FR-024) |
|---|---|---|
| what is known | nothing — the peer may be hung, partitioned, or about to write | the peer is alive and answered |
| queue pair | forced to ERROR and drained | untouched |
| other contexts | all aborted with it | unaffected |
| latency to detect | a deadline | a round trip |

Tearing the queue pair down is the only thing that makes a late write impossible, so it is
mandatory when the peer's state is unknown and gratuitous when it is not. Routing a protocol
error through the deadline's cleanup would destroy a healthy connection and take unrelated
requests with it.

## Why `fetch` returns a context id

The caller needs it for exactly one reason: to correlate a `Deadline` error, or a log line,
with the request it came from. It names the caller's whole fetch; a fetch the endpoint
chunks puts several wire `context_id`s on the wire, one per chunk message, and the endpoint
maps them back to this one (wire-protocol *Chunking*) — including distinguishing the context that actually expired
from the siblings aborted with it when its connection was torn down (FR-023). It is **not** a handle for cancellation — cancelling a fetch
cannot be made safe without the destination holding its landing slots anyway, and holding
them is the whole point of FR-017. If cancellation is ever wanted, it must be a separate
method with its own teardown story.

## Relationship to `IRemoteLookupRdmaInitiator`

`push_async` / `push` are deliberately signature-compatible so `remote-lookup` can be
re-pointed with a receptacle change and no logic change (FR-019). The one difference is the
return type: `PushStatus` becomes `EntryOutcome`, which adds the tier distinction. If that
widening is unwanted at cutover time, `EntryOutcome` can collapse to `PushStatus` at the
boundary — the legacy caller does not use the tier information.
