# Feature Specification: Explicit certus-to-certus KV cache transfer over a symmetric RDMA endpoint

**Feature Branch**: `001-rdma-cache-transfer`

**Created**: 2026-10-14

**Status**: Draft

**Input**: User description: "Symmetric RDMA endpoint with verbs messaging for explicit certus-to-certus KV cache transfer, as an alternative to broadcast remote-lookup"

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Destination fetches a prefix from a named peer (Priority: P1)

A certus instance is asked for a run of block keys it does not hold. An external
decision-maker (llm-d, via `kv_transfer_params`) has already named a single peer believed
to hold them. The destination reserves unpublished slots in its own memory tier, opens (or
reuses) one RDMA connection to that peer, and sends a single request naming the keys and
the landing slots. The source pushes the values it holds directly into those slots and
replies with a per-key outcome. The destination publishes the slots that arrived and
releases the rest.

No broadcast, no peer polling, no reap: exactly one peer is asked, exactly once.

**Independently testable**: two endpoints over RDMA loopback (or the mock seam); assert the
requested keys land in the named slots and the reply's per-key outcomes match the source's
actual residency.

### User Story 2 - The existing broadcast remote-lookup keeps working (Priority: P2)

`remote-lookup` continues to drive transfers exactly as it does today: the control plane
arrives over zyre, and it invokes a push with `(key, remote-region)` pairs. It does not use
verbs messages, context IDs, or the reply path. It sees no behaviour change.

**Independently testable**: the scenario of the hardware loopback test
`remote-lookup-rdma-initiator/src/loopback_test.rs::loopback_push_writes_into_remote_buffer`
passes through the new endpoint's push, and its order, reuse and teardown properties hold
over the mock seam, with messaging unused. Running the `remote-lookup` test layer itself
against the endpoint is part of the cutover (T065).

### User Story 3 - The source serves an entry that has been demoted to SSD (Priority: P3)

A requested key is resident on the source's block device rather than in its memory tier.
The source reads it into a bounce buffer — space set aside in its memory tier's pool, outside
the cache — pushes from there, and reports `hit_block_tier`. Its own memory tier is **not**
polluted: no promotion occurs, so serving a peer does not evict the source's own working set.

**Independently testable**: force a key to the block tier, request it, assert the value
arrives, the reply says `hit_block_tier`, and the source's memory-tier occupancy and the
relative LRU order of its memory-tier entries are unchanged.

### Edge Cases

- **Source does not hold a requested key.** Reported as `miss` in the reply. The source
  does **not** fall back to broadcast remote-lookup. The destination releases that slot and
  lets the caller recompute.
- **Destination cannot reserve all slots.** It requests only what it reserved; the shortfall
  is simply not fetched.
- **Source is short of bounce space or device readers, or destination short of evictable
  space.** The transfer is split into chunks internally. Neither side fails the request for
  want of a transient resource. An entry larger than the source's whole bounce reserve is not
  transient: it is `failed`, loudly.
- **The source's device is slow or hung.** The source waits for the read; it has no device
  timeout, because reclaiming a buffer the drive may still write is unsafe. The requester's
  reply deadline (FR-016) fires instead, and that is the intended behaviour.
- **Peer process dies mid-transfer.** The connection closes; each side releases its own
  resources. The destination's slots are reclaimed only after its queue pair is torn down
  (teardown-before-reclaim).
- **Peer hangs or partitions with the connection open.** Detected by deadline, not by
  connection state. In research configuration the request **fails loudly** with full
  diagnostic state; see FR-016. The queue pair is then forced to ERROR before any slot is
  reclaimed, which also aborts the request's siblings on that connection; see FR-023.
- **Duplicate or overlapping requests for the same key.** Pins are reference-counted;
  concurrent pushes of one key are safe.
- **Peer does not understand the message.** Answered with a protocol error reply naming the
  opcode and the sender's highest supported protocol version (0 if the kind itself is
  unknown), which fails the one context
  and leaves the connection and its siblings intact (FR-024). A *reply* the requester cannot
  understand is the opposite case: never answered, reported loudly, and the connection torn
  down, because the peer may have written into slots it can no longer vouch for. A stale binary on one node is
  the common cause and is the case this turns from a mystifying timeout into a precise
  diagnostic.
- **Key present but stored size does not match the requested region.** Treated as a miss.

## Requirements *(mandatory)*

### Functional Requirements

**Endpoint and connections**

- **FR-001**: The endpoint MUST be symmetric: a queue pair may be inbound-accepted or
  outbound-initiated, and **either** may post work requests. The current split — responder
  accepts but never posts, initiator connects and posts — MUST NOT constrain the new path.
- **FR-002**: At most one connection per peer pair MUST be used, reused across requests, and
  established on first need. A "peer pair" means a pair of *instances*, not of hosts — see
  FR-021.
- **FR-003**: The endpoint MUST build and unit-test with default features — no rdma-core
  present — over an in-process transport seam, as both existing crates do today.
- **FR-004**: The endpoint MUST NOT depend on zyre.
- **FR-021**: A peer MUST be identified at **instance** granularity, not host granularity.
  Several certus instances run per node — one per NUMA domain in the current rigs — so a
  host alone is not an address. The identifier MUST carry at least host and port, MUST be
  normalized before use as a connection-table key, and an identifier without a port MUST be
  rejected rather than defaulted. Two instances on the same host MUST be addressable
  independently and MUST hold separate connections, including when both are the local host
  (which is also the single-host loopback verification path).

**Request / reply messaging**

- **FR-005**: The endpoint MUST post receive buffers unconditionally, whether or not the
  caller uses messaging.
- **FR-006**: A transfer request MUST carry a context ID and, per entry, the block key and that
  entry's landing slot as an absolute destination address, size and rkey — the content of
  `(CacheKey, RemoteRegion)`. The source MUST NOT need a base address, a slot index or a
  uniform entry size: the memory tier is not index-addressable.
- **FR-007**: A transfer reply MUST carry the context ID and one outcome per requested key,
  from `{ hit_memory_tier, hit_block_tier, miss, failed }`, in request order. `miss` means the
  source does not serve the entry (absent, a different size, or a tier this build cannot
  serve); `failed` means it was not delivered for any other reason (a failed lookup, a
  lookup answering a location outside the registered pool, a refused or flushed write, a connection that could not be made or was lost, a protocol
  fault, a deadline). The two MUST NOT be conflated: a broken path is not a cold cache. Each request message
  MUST carry its **own** context ID and MUST draw exactly one reply (or one protocol error,
  FR-024): the source MUST NOT split a reply, so that a requester's reply reserve (FR-026) is
  bounded exactly by its own message credits. When the requester chunks a transfer, each
  chunk is a separate request message with its own context ID; correlating the chunks of
  one transfer is the requester's own bookkeeping, not a wire concern.
- **FR-008**: Because landing slots are not contiguous, the source MUST issue one RDMA write
  per entry rather than one per batch.
- **FR-024**: The endpoint MUST answer a **request** it cannot process with an explicit
  **protocol error reply**, sent in place of that request's one reply, never with silence. Silence is indistinguishable from a hang, so it would be
  diagnosed by the FR-016 deadline and would then tear down a healthy connection and abort
  that connection's other contexts (FR-023) — a loud but *wrong* diagnosis, which Principle XI
  treats as worse than none. The error reply MUST echo the `context_id` it is answering, name
  the offending opcode, and carry a reason drawn from protocol faults only: unknown opcode,
  unsupported version of a known kind, malformed message, or more entries than the receiver's
  posted buffer can hold. It MUST NOT be used for a miss (an outcome) or for resource
  shortage (which chunks, FR-015). Receiving one MUST fail that one context and **leave the
  connection usable**: the peer is healthy and acted on nothing, so the FR-023 teardown path
  MUST NOT run. A request's opcode is even and its reply's is the same with the low bit set,
  so a message of unknown kind is classified by that bit: an even one MUST be answered as a
  request; an odd one is a reply (below).

  The endpoint MUST tear a connection down only when it must: (i) the queue pair went to
  ERROR; (ii) a write may still land in memory about to be reclaimed (a reply deadline, a
  bad reply); (iii) its credit accounting is in doubt (a reply nobody asked for, an unknown
  reply, an `OP_ERROR` naming a request never sent, a request beyond the advertised quota, a
  receive that failed unread); or (iv) the peer's version or framing cannot be trusted (a
  failed capability exchange, which fails waiting fetches with `IncompatiblePeer`). Anything
  else a request does wrong — before `OP_HELLO`, a duplicate `OP_HELLO`, malformed, unknown —
  MUST be answered with `OP_ERROR` and the connection kept. Every `OP_ERROR` sent or received,
  and every teardown an inbound message causes, MUST be counted as a protocol fault.

  The endpoint MUST NOT answer a **reply** (a success reply or a protocol error reply), even
  one it cannot process: the replier holds no state to act on, and answering replies would
  allow an error loop. A reply that cannot be processed — malformed, wrong kind, unknown
  context, or a duplicate — MUST instead be reported loudly and MUST tear the connection down
  through the FR-023 ordering, aborting its contexts: the peer may have posted writes it can no
  longer be trusted to have completed, so its landing slots are reclaimed only after teardown
  (FR-017).
- **FR-025**: Both ends of a new connection MUST exchange capabilities before either sends a
  data message, and MUST NOT send one until the peer's capabilities are known. The exchange
  MUST report the **protocol version range** the sender supports and the maximum entries it
  can receive in one request. The version in force for the connection MUST be
  `min(a.max, b.max)`, and the **connection** MUST fail at establishment if that falls below
  `max(a.min, b.min)` — with a diagnostic naming peer and both ranges, not a failed request
  and not a later deadline. A requester MUST respect the peer's maximum request size rather
  than assume it equals its own; it chunks to the smaller of the peer's advertised
  `max_entries` and the largest request it can itself encode in one send buffer. An advertised
  `max_entries` of zero MUST fail the connection, as a zero quota does. One version governs the whole protocol: a reply's meaning is tied to the
  version of the request it answers, so per-kind versions MUST NOT be negotiated
  independently.

  The exchange MUST be **unsolicited and symmetric** — both ends send, with no initiator
  privilege — and the rule MUST be a **commutative function of the two advertised ranges**, so
  both ends compute the same answer independently and no arbitrator exists. Each capability
  message is a request and draws a reply, but that reply MUST carry no content: a reply
  carrying capabilities or a verdict, or an asymmetric rule (initiator decides, higher version
  decides), would create a second decision point and with it the possibility of the two ends
  disagreeing. Because the peer's request quota is learned from this exchange, each end MUST
  begin a connection with exactly one message credit, for its capability message, and an
  advertised quota of zero MUST fail the connection. Capabilities MUST be held on the connection and discarded with it, so a
  rebuilt connection re-learns them and an upgraded peer is picked up without separate
  invalidation. It MUST also report how many concurrent requests the peer may have
  outstanding against it (FR-026).
- **FR-026**: One connection carries traffic in **both directions** (FR-002, D5): an instance
  may be destination for one context and source for another on the same queue pair. Three
  things follow, each of which MUST hold:
  - **Context identifiers are scoped per direction.** The requester allocates them, so both
    ends may independently allocate the same value on one connection. Each end MUST keep its
    outbound-request table and its inbound-serving table **separate**; a single
    contexts-on-this-connection map would silently conflate two unrelated requests.
  - **Message credits are per direction.** Each end's credits bound its own sends against the
    peer's posted receives; the two counters are independent.
  - **Awaited replies MUST NOT be starved by inbound requests.** Both consume the same posted
    receive buffers, so a peer issuing requests up to the buffer count can leave no buffer for
    a reply the local side is blocked on. Each end MUST reserve one receive buffer per own
    message credit for replies — exact, because each request draws one reply (FR-007) — and
    advertise a separate inbound-request quota to the peer; a peer MUST NOT exceed the quota
    it was granted, and posts reserve + quota receive buffers in all.
- **FR-027**: Because either end may initiate (FR-001), both MAY initiate simultaneously. The
  endpoint MUST converge on **exactly one** connection per instance pair, chosen by a
  deterministic rule over the two `PeerId`s that both ends evaluate identically, with no
  arbitrator and no extra round trip. Resolution MUST happen when the duplicate is detected at
  accept — which requires the initiator to identify itself in the connection request — and
  therefore **before** the capability exchange, so the losing queue pair carries no context and
  nothing has to be migrated. Tolerating two connections is NOT acceptable: `disconnect`
  tears down *the* queue pair, so a second one could still land a write into a slot already
  reclaimed, defeating FR-017.

  A duplicate the race cannot explain — from an end that already owns or out-ranks the live
  link — MUST be resolved *newer wins*: the old link goes to ERROR first. An inbound request
  while the peer's previous link is still draining MUST be refused. An acceptor MUST answer to
  the name it was dialed by, else the link fails. Known gap: a crashed-and-restarted peer that
  out-ranks nobody is refused until a deadline notices the stale link (research D24).
- **FR-028**: A panic on the endpoint thread or a block-reader thread MUST abort the process:
  continuing with a possibly corrupt pin, slab or credit table risks a silent double release
  (Principle XI).

**Per-entry handling on the source**

- **FR-009**: Residency determination and pinning MUST be atomic. The source MUST obtain
  location and pin in a single dispatch-map lookup, which returns memory-tier and
  block-tier answers already pinned. It MUST NOT determine residency with an unpinned probe
  and then pin.
- **FR-010**: The source MUST release exactly the read references its lookup took, each
  exactly once. An answer that was never pinned (`NotExist`, `MismatchSize`) MUST NOT be
  released — that decrements another reader's pin. An answer that was pinned but cannot be
  served — a memory-tier entry whose stored size differs from the requested one, or a
  block-tier entry in a build that cannot serve it — MUST be released before reporting
  `miss`, or the entry stays pinned and can never be evicted. The lookup takes no size, so
  a size mismatch is detected by the source and comes back **pinned**.
- **FR-011**: Pins MUST be held and released **per entry**, not per batch. A memory-tier
  pin is released when its RDMA write completes (matched by work-request id). A block-tier
  pin is held for the device read and released as soon as the read into its bounce buffer
  completes — before the write is posted — since the data has been copied; the buffer is then
  returned when the write completes.
- **FR-012**: Block-tier entries MUST be served from an RDMA-registered bounce reserve, set
  aside in the memory tier's pool outside its cache and read into by the dispatcher (research
  D13, D15). The source MUST NOT promote them into its memory tier to serve a peer, and
  serving MUST NOT evict a cached entry.
- **FR-013**: The source MAY process a request list concurrently, bounded by the credit
  limits in FR-014.
- **FR-022**: The source MUST NOT introduce a phase barrier within a request. Specifically it
  MUST NOT drain its send queue between entries, and on a request mixing tiers it MUST NOT
  wait for device reads to finish before posting memory-tier writes: memory-tier entries are
  posted as soon as they are resolved, and each block-tier entry is posted when its own
  device read completes. Only the end-of-message reply (FR-007) is a synchronization point.
  See research D19 for why this is the *only* barrier worth engineering away, and why no
  cross-message pipelining is required.

**Resource bounds** — three distinct domains, separately configured and named

- **FR-014**: The endpoint MUST bound each of:
  - **send-queue credits** — in-flight send-queue work requests per connection: RDMA writes
    *and* message sends, which share the queue (counting only writes would let messages
    overfill it). This transitively bounds held read pins, which is the existing rationale for
    the bound. Send credits are clamped to the depth the device grants, and the clamp is
    logged;
  - **message credits** — outstanding requests against the peer's posted receive buffers.
    The destination decrements on send and increments on reply; at zero a request MUST
    wait inside the endpoint — never fail, and never block the caller;
  - **bounce-buffer occupancy** — bounce-reserve bytes allocated to block-tier entries, and
    concurrent device reads (the reader count). A `NoSpace` is believed only if no buffer was
    returned after its read was submitted: otherwise a deferred entry could wait for space
    that is already free.
- **FR-015**: On exhaustion of any bound, the endpoint MUST chunk internally and make
  progress, not fail.

**Failure handling**

- **FR-016**: A destination-side deadline MUST detect a request that is neither fulfilled
  nor refused. Detection MUST be by deadline, not by connection state: a peer that hangs or
  partitions with its connection open is invisible to the transport. The deadline MUST run
  from when the request takes its message credit and is queued to the connection's send
  queue — not from the `fetch` call — so a request waiting for a message credit or the
  capability exchange cannot expire. The action MUST be
  configurable, defaulting to **fail loudly** — log the key range, peer, chunk and
  queue-pair state, then fail the request. A degrading action (report what arrived and let
  the caller recompute) is deferred, not implemented. This destination-side *reply* deadline
  is distinct from a source-side stall cap on uncompleted writes, and the two MUST be named
  and configured separately.
- **FR-017**: The destination MUST NOT reclaim or publish a landing slot until it is certain
  no write can still land in it — queue-pair teardown MUST precede reclaim.
- **FR-018**: A partially written entry MUST NOT be published. Landing slots follow
  reserve → arrive → commit/abort, as local stores do.
- **FR-023**: On deadline expiry the endpoint MUST make further writes *impossible* before it
  reclaims anything, in this order: transition the queue pair to ERROR, drain and discard its
  completions, reclaim the landing slots, and only then permit a fresh connection to be
  established on next need. Logging the queue-pair state (FR-016) is not a substitute for
  changing it — a slow-but-live source whose write lands in a reclaimed slot corrupts an
  unrelated entry. Because one connection serves all requests to a peer (FR-002), expiry of
  any one request MUST abort every context outstanding on that connection; this blast radius
  MUST be logged, and MUST NOT be narrowed by opening additional connections per request.

**Compatibility**

- **FR-019**: The endpoint MUST expose a push entry point usable by `remote-lookup`
  unchanged: `(endpoint, [(key, remote-region)], completion)` with one status per item in
  order, and no messaging involved. The blocking `push` MUST be refused, not deadlock, when
  called on an endpoint thread (from a completion callback, say).
- **FR-020**: `remote-lookup`'s logic MUST NOT be modified as part of delivering P1. The only
  permitted change is to its test doubles, where an interface it implements grows (the
  bounce-reserve methods of `IMemoryTier` and `read_to_bounce`/`release_bounce` of
  `IDispatcher`); those doubles refuse every buffer. Re-pointing it
  at this endpoint, and retiring `remote-lookup-rdma-initiator` and
  `remote-lookup-rdma-responder`, is a later task.

### Key Entities

- **TransferContext** — one in-flight request message (one chunk of a fetch): context ID,
  peer, its entries and their landing slots, its offset in the caller's fetch, and its send
  time, from which the reply deadline runs.
- **TransferRequest / TransferReply** — the two verbs messages of FR-006 / FR-007.
- **LocalSlot** — where a fetched entry lands: address and length of a buffer the destination
  has allocated in its own memory tier and not yet published. Needs no registration: the
  pool is registered once, and the endpoint names its rkey.
- **RemoteRegion** — a slot as the wire names it: `addr`, `rkey`, `length`. Already exists.
- **EntryOutcome** — `hit_memory_tier | hit_block_tier | miss | failed`.
- **BounceBuffer** — `(ptr, len)` in the bounce reserve: a range of the memory tier's pool,
  outside the cache, that the dispatcher reads a block-tier entry into and the endpoint
  writes from. Already DMA-able and RDMA-registered, because the pool is.
- **PeerConnection** — one queue pair per instance pair, used in **both** directions, keyed by
  `PeerId` and replaceable without invalidating the pool's registration. Holds per-direction credit
  counters, separate outbound-request and inbound-serving context tables, and the negotiated
  capabilities (FR-026).
- **PeerId** — an instance-granular peer address, carrying at least host and port (FR-021).
- **PeerCapabilities** — supported protocol version range, maximum receivable entries, and
  granted inbound-request quota; learned at connect and discarded with the connection
  (FR-025, FR-026).
- **ProtocolError** — the error reply of FR-024: offending opcode, reason, and the sender's
  highest supported protocol version (0 if the kind itself is unknown).
- **LandingRegion** — the memory-tier pool as peers write into it: address, length and rkey.
  Registered once, at `initialize`; the registration outlives any one queue pair, so its
  `rkey` survives a reconnect; see research D17.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A destination fetches a requested key run from one named peer with **zero**
  zyre messages and **zero** messages to any third instance.
- **SC-002**: The legacy push surface reproduces the initiator's
  `loopback_push_writes_into_remote_buffer` scenario on hardware through this endpoint, and
  its order, reuse and teardown properties hold over the mock seam, with messaging unused.
  Running the `remote-lookup` test layer itself against this endpoint requires re-pointing its
  receptacle and is part of the cutover (T065), not of P1.
- **SC-003**: The crate builds and unit-tests with `--no-default-features` equivalent: no
  rdma-core, no zyre.
- **SC-004**: Peak concurrently held read pins on a source never exceeds the configured
  send-queue credit limit plus the configured device-reader count (writes in flight plus reads
  in flight), however long the requested key list is.
- **SC-005**: Serving a block-tier entry leaves the source's memory-tier occupancy, and the
  relative LRU order of its memory-tier entries, unchanged. (The lookup that pins the
  block-tier entry refreshes that entry's own position; nothing in the memory tier moves.)
- **SC-006**: A request naming keys the peer does not hold returns all-`miss` and generates
  no broadcast traffic.
- **SC-007**: With bounce space, message credits or the peer's `max_entries` artificially
  scarce, a large request still completes, in chunks, with its outcomes in request order.
  (Landing slots are the caller's: it requests only what it reserved.)
- **SC-008**: This crate defines the verbs FFI once, as the single copy the predecessors will
  share or be replaced by; the duplication itself ends when `remote-lookup-rdma-initiator` and
  `-responder` are retired (T066). (Today
  `remote-lookup-rdma-initiator/src/ffi.rs` and
  `remote-lookup-rdma-responder/src/ffi.rs` declare 52 public names in common.)
- **SC-009**: Two instances on one host, differing only by port, are addressed independently
  and hold separate connections; an identifier without a port is rejected.
- **SC-010**: After a deadline expires, no write from that peer can land in a reclaimed
  landing slot, and every sibling context on the connection is reported as aborted. A
  subsequent fetch to the same peer succeeds over a fresh connection without re-registering
  the pool.
- **SC-011**: On a request mixing memory-tier and block-tier entries, every memory-tier write
  is posted before the first device read completes, and the send queue is never idle while
  unposted entries remain.
- **SC-012**: A message with an unknown opcode draws an error reply naming that opcode, within
  a round trip rather than a deadline; the connection stays usable and the other contexts
  outstanding on it complete normally.
- **SC-013**: A peer advertising a smaller maximum request than the local one is never sent a
  larger one, and a peer whose version range does not intersect the local one fails at connect
  with a diagnostic naming peer and both ranges — no request is ever issued to it.
- **SC-014**: For every pair of advertised version ranges, both argument orders yield the same
  version and the same accept/refuse verdict — the rule is commutative, so the two ends cannot
  disagree.
- **SC-015**: Two instances initiating simultaneously end with exactly one connection, and
  both agree which one survived.
- **SC-016**: A peer saturating its granted inbound-request quota does not prevent the local
  side from receiving replies to its own outstanding requests; bidirectional traffic at full
  quota makes progress rather than deadlocking.

## Assumptions

- **A single peer is chosen externally.** llm-d names the source; this endpoint does not
  discover or select peers, and deliberately does not broadcast.
- **The destination already knows the keys.** It computes block hashes locally, so the
  request names a contiguous run following its own local prefix.
- **Transfers are host memory to host memory.** GPU memory is not involved; the existing
  tier→GPU load path is unchanged, so a completed transfer is indistinguishable from a
  local tier hit.
- **Block-tier serving is a completeness path, not a hot path.** Measured workload-generator
  runs show single-digit-percent SSD hit rates, and a well-sized memory tier avoids them
  altogether; the bounce reserve is sized accordingly (32 MiB by default, taken quietly from
  the pool), and SSD/RDMA pipelining is out of scope.
- **Single-tenant trust.** RDMA write access into a peer's landing slots is acceptable;
  no cross-tenant isolation is attempted.
- **Research posture over production resilience.** Where a safety net would mask a bug and
  still yield a plausible benchmark number, the endpoint fails loudly instead. See FR-016.
