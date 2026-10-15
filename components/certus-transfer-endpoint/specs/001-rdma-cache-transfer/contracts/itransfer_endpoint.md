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
    /// Carries diagnostic state (FR-016, Principle XI).
    Deadline(String),
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
        fn push_async(
            &self,
            endpoint: &str,
            items: &[(CacheKey, RemoteRegion)],
            on_complete: PushCompletion,
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

        /// Snapshot of the three credit domains and per-outcome counters.
        fn stats(&self) -> TransferStats;
    }
}
```

## Invariants

Implementations MUST uphold, and tests MUST verify:

1. **Order and length.** `on_complete` receives exactly `items.len()` outcomes, and index
   *i* corresponds to `items[i]`. True across internal chunking.
2. **Outcome honesty.** `HitBlockTier` is reported only when the value was actually read
   from the block device. A build without `spdk` reports `Miss` for block-tier entries, never
   `HitBlockTier` and never an error (research D14).
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
   order unchanged (SC-005, D8).
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

## Why `fetch` returns a context id

The caller needs it for exactly one reason: to correlate a `Deadline` error, or a log line,
with the request it came from — including distinguishing the context that actually expired
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
