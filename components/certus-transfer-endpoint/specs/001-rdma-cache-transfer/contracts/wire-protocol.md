# Contract: transfer wire protocol

**Feature**: `001-rdma-cache-transfer` | **Date**: 2026-10-15

Two verbs SEND messages over the symmetric connection of D5. Values themselves travel by
one-sided RDMA WRITE and never appear in a message. Interface contract:
[itransfer_endpoint.md](./itransfer_endpoint.md).

All integers little-endian. All offsets in bytes from the start of the message.

## Message framing

Every message begins with a 24-byte header. The first word is the **opcode, which names a
message kind only** — the protocol version lives in its own header field and is *not* fused
into the opcode (research D21).

| name | `opcode` | |
|---|---|---|
| `OP_FETCH` | `0x0000_0001` | kind 1 |
| `OP_FETCH_REPLY` | `0x0000_0002` | kind 2 |
| `OP_HELLO_REPLY` | `0x0000_FFFD` | kind 0xFFFD, **unversioned** |
| `OP_HELLO` | `0x0000_FFFE` | kind 0xFFFE, **unversioned** |
| `OP_ERROR` | `0x0000_FFFF` | kind 0xFFFF, **unversioned** |

**Requests and replies.** `OP_FETCH` and `OP_HELLO` are **requests**; `OP_FETCH_REPLY`,
`OP_HELLO_REPLY` and `OP_ERROR` are **replies**. Every request costs one message credit and
draws exactly one reply — its success reply, or `OP_ERROR` in its place — which lands in the
requester's reply reserve (*Receive-buffer partitioning*). **A reply is never answered**, not
even a malformed one; see *Bad replies*. A message of unknown kind is treated as a request
and answered with `OP_ERROR`: this end only sends requests whose replies it understands, so
an unknown kind cannot legitimately be a reply to us.

The high 16 bits of `opcode` MUST be zero and are reserved for future kinds. Two kinds carry
the data plane; everything it needs — chunking, credits, tier attribution — rides in the
header flags or the body rather than in more kinds.

`OP_HELLO` and `OP_ERROR` are **frozen for all time**: they are the messages a peer must be
able to parse *even when it cannot parse the version in question*, since versioning them would
mean answering an unparseable message with an unparseable error. They carry `version = 0`,
which no real version uses, and a receiver ignores the version field for them. They stay
extensible through their **content** — an open reason-code space — never through their layout
(research D20).

**Why the version is a field and not part of the opcode.** An earlier draft fused them as
`(version << 16) | kind`, which was the whole version-safety mechanism while there was no
handshake. With the version negotiated at connect (D20) fusing became redundant *and*
cluttering: a bump would mint a new opcode for every kind including the ones that did not
change, so an unchanged `OP_FETCH_REPLY` would accumulate one opcode value per version, all
dispatching to the same handler — and the handler would mask the version straight back off.
A mechanism whose common path discards it is not earning its place. See D21.

### The frozen header prefix

Bytes 0-15 — `opcode`, `flags`, `version` and `context_id` — mean the same thing in **every
version of every kind, forever**. That is the minimum a receiver needs to reject a message it
does not understand, say *why*, and still correlate the rejection with the request that caused
it. Later header fields and all bodies may be redefined by a version bump; these may not.

A receiver dispatches on `opcode` alone and **validates `version` once**, in the header parse,
against the version negotiated for that connection. It is a consistency check, not a dispatch
key: both ends already agreed, so a mismatch means a negotiation bug or a mismatched build,
and it is answered with `OP_ERROR` reason 2. Validating in one place keeps the guarantee that
fusing used to provide — a message is never parsed at the wrong version — without a match arm
per kind per version.

| offset | size | field | notes |
|---|---|---|---|
| 0 | 4 | `opcode` | message kind; high 16 bits MUST be zero |
| 4 | 2 | `flags` | reserved, MUST be zero; in the frozen prefix so a future flag needs no layout change |
| 6 | 2 | `version` | protocol version in force; validated, not dispatched on. Zero and ignored for `OP_HELLO`/`OP_ERROR` |
| 8 | 8 | `context_id` | identifies one request **message**; echoed in its one reply |
| 16 | 4 | `n_entries` | entries in *this* message |
| 20 | 4 | reserved | MUST be zero; keeps the entry vector 8-byte aligned |

### `OP_FETCH` (`0x0000_0001`)

Header, then `n_entries` × 24-byte entries. **Nothing else** — no base address, no rkey, no
uniform entry size in the header.

| offset | size | field |
|---|---|---|
| 0 | 8 | `key` — the 64-bit block key |
| 8 | 8 | `addr` — absolute address to write to, on the requester |
| 16 | 4 | `size` — bytes for this entry |
| 20 | 4 | `rkey` — requester's rkey covering `addr` |

This carries exactly the content of `(CacheKey, RemoteRegion)`, which is what
[itransfer_endpoint.md](./itransfer_endpoint.md) already passes to `fetch` and
`push_async`. It is **content equivalence, not a memory image**: the real struct is
`{ addr: u64, rkey: u32, length: u32 }` and its last two fields sit in the opposite order
from the wire's `size` then `rkey`. The wire's `size` *is* `RemoteRegion::length`. Since both
trailing fields are `u32`, a transposition silently swaps a length for a key, so the codec
must name fields explicitly and never copy the pair wholesale. (Reordering the wire to `rkey`
then `length` to match the struct is still free — nothing implements this yet — and would
remove the hazard entirely; left as-is for now because the layout is already a committed
contract.)

**Why addresses rather than slot indices.** An earlier draft sent
`(key, slot_index)` with a per-message `base_addr` and uniform `entry_size`, computing the
target as `base_addr + slot_index * entry_size`. That is **unimplementable against the
memory tier**: `IMemoryTier` is `insert(key, size) -> *mut u8` and
`get(key) -> Option<(*mut u8, u32)>` — an allocator keyed by `CacheKey` with a per-entry
size, with no index-addressable slot table and no uniform entry size. Indices would have
invented a layout the tier does not have, and coupled the wire format to it. The saving
would have been 8 bytes per entry. Addresses cost nothing in practice (see sizing) and the
`rkey` rides in what would otherwise be padding, so per-entry regions are supported for
free — the requester may land entries across several registered regions.

### `OP_FETCH_REPLY` (`0x0000_0002`)

Header, then one outcome byte per entry in request order, zero-padded to an 8-byte boundary.

Outcome byte: `0` = Miss, `1` = HitMemoryTier, `2` = HitBlockTier, `3` = Failed (the source
holds the entry, or could not tell, but did not deliver it — its lookup failed, or its write
did). Values >= 4 reserved; a receiver MUST treat an unknown outcome as Miss rather than
erroring, so a future source can add a tier without breaking an old requester.

**Ordering guarantee**: every RDMA write for an entry reported as a hit MUST have completed
before the reply is sent. The requester may therefore treat reply arrival as the completion
signal for the whole message and needs no per-entry completion notification. This is what
makes the one-sided WRITE safe without `WRITE_WITH_IMM`.

### `OP_HELLO` (`0x0000_FFFE`)

Sent by **both** ends immediately on connection establishment — the endpoint is symmetric, so
neither side is the client (D5). Neither end sends a data message until the peer's `OP_HELLO`
has arrived. `context_id`, `version` and `n_entries` are zero.

**`OP_HELLO` is a request; `OP_HELLO_REPLY` exists only to balance credits.** Both ends send
`OP_HELLO` unsolicited, so after one exchange each end holds both advertisements, and the
version is computed by each end from the two — **nobody decides it**. `OP_HELLO_REPLY` carries
**no content**: it is the 24-byte header only, `context_id` and `n_entries` zero, and must
stay empty forever. Its one job
is to keep the credit rule uniform — every request costs a credit and draws exactly one reply
— so `OP_HELLO` needs no special-case buffer in either partition. A reply that carried
capabilities or a verdict would create a second decision point, the only way the two ends
could disagree; a contentless one decides nothing.

**Bootstrap.** The quota that sizes each end's credits arrives *inside* `OP_HELLO`, so the
first `OP_HELLO` is sent before either end knows the other's quota. Each end therefore starts
every connection with **one** message credit, spends it on `OP_HELLO`, and on the peer's
`OP_HELLO` sets its limit to the quota the peer advertised (FR-026). This requires each end to
have at least one reply-reserve buffer and one quota buffer posted before the connection is
established, and an advertised `max_outstanding_requests` of 0 is invalid (the connection
fails, as for a non-intersecting version range).

The body is **fixed size**, 40 bytes total, which is what a permanently frozen layout wants:

| offset | size | field | notes |
|---|---|---|---|
| 24 | 4 | `max_entries` | largest `OP_FETCH` **this** sender can receive |
| 28 | 4 | `max_outstanding_requests` | how many concurrent requests the **peer** may have outstanding against this sender |
| 32 | 2 | `version_min` | lowest protocol version this sender can speak |
| 34 | 2 | `version_max` | highest protocol version this sender can speak |
| 36 | 4 | reserved | MUST be zero |

**Nobody decides the version; both ends compute it.** The version in force is
`min(a.version_max, b.version_max)`, and the connection fails at establishment if that is
below `max(a.version_min, b.version_min)` — naming peer and both ranges, with no request ever
issued. Both `min` and `max` are commutative, so the result is a pure function of two values
*both ends possess*, symmetric in its arguments: there is no proposer, no arbitrator, and
structurally no way for the two ends to choose differently. Any asymmetric rule — initiator
decides, higher version decides — would reintroduce that possibility and is forbidden.

`max_outstanding_requests` is the peer's **inbound-request quota**, and it exists because one
connection carries both directions (FR-026): see *Receive-buffer partitioning* below.

**One version governs the whole protocol**, not each kind. A reply's meaning is tied to the
version of the request it answers, so independently negotiated per-kind versions could
represent an `OP_FETCH` at v2 answered by an `OP_FETCH_REPLY` at v1 — a combination that is a
correctness trap and that a single version cannot express. An earlier draft advertised a
counted `(kind, max_version)` list; see D20's *Revision* for why that was dropped.

A requester MUST respect the peer's `max_entries` rather than its own when chunking — the two
sides are not required to have equal receive buffers, and before this exchange existed that
equality was an *unchecked assumption* (D20).

Capabilities live on the connection and die with it, so a rebuilt connection re-learns them
and an upgraded peer is picked up with no separate invalidation step.

### `OP_ERROR` (`0x0000_FFFF`)

**Fixed size: 32 bytes total**, header included — frozen, like `OP_HELLO`'s 40.

The reply to any **request** that cannot be processed, sent **in place of** that request's
success reply — so it lands in the same reply-reserve buffer, and the reserve stays exact.
`n_entries` is zero. A reply is never answered with `OP_ERROR`; see *Bad replies*.

| offset | size | field | notes |
|---|---|---|---|
| 24 | 4 | `offending_opcode` | the whole opcode word as received |
| 28 | 2 | `reason` | below |
| 30 | 2 | `max_supported_version` | highest **protocol** version the sender supports; 0 if the opcode's kind is unknown entirely |

| `reason` | meaning |
|---|---|
| 1 | unknown opcode — the kind itself is not recognised |
| 2 | unsupported version of a recognised kind |
| 3 | malformed message |
| 4 | `n_entries` exceeds the receiver's posted buffer |

Reason 0 is reserved and values ≥ 5 are available; a receiver MUST treat an unknown reason as
a generic protocol failure, never as success. `max_supported_version` exists so a requester can
go straight to a workable version instead of probing downward — needed only when negotiation
has been bypassed or has a bug, since `OP_HELLO` normally settles the version before any data
message.

**Scope is protocol faults only.** A miss is an outcome byte, not an error, and a resource
shortage chunks (FR-015). Using `OP_ERROR` for either would quietly become the escape hatch
that undermines D12 and FR-015.

**Blast radius.** Receiving `OP_ERROR` fails **that one context** and leaves the connection
usable: the peer is healthy and answered promptly, so the FR-023 deadline teardown must *not*
run and sibling contexts on the connection must be unaffected. This is the deliberate contrast
with a deadline, where the peer's state is unknown and tearing the queue pair down is the only
thing that makes a late write impossible. Two fail-loud paths, two blast radii.

### Bad replies

A reply this end cannot process — malformed, of the wrong kind, naming no outstanding
context, or a second reply to one request — is **never answered**. The replier holds no
state for a request once it has replied (its pins are released, its context is gone), so an
error back would give it nothing to act on, and it cannot be the side left waiting: the side
that would wait is the one that just received the reply. Answering replies would also allow
an error-for-an-error loop; not answering them rules it out by construction.

Instead the receiver **reports loudly and tears the connection down** through the FR-023
path — queue pair to ERROR, drain, *then* reclaim, aborting every context on the connection —
and counts it as a protocol fault. This is the third case, and it deliberately differs from
`OP_ERROR` received for our own request. There the peer *acted on nothing*, so failing one
context is safe. A bad reply comes from a peer that *did* process our request and may have
posted writes into our landing slots, and whose ordering guarantee (*every write completes
before the reply*) we can no longer trust; a duplicate reply has also already broken the
one-reply rule the reserve counts on. Reclaiming those slots without teardown would break
FR-017. Reconnection then happens on next need, as for any teardown.

**Open — repeated teardown of one peer.** A responder bug that produces bad replies, like one
that makes requests hang, gives teardown → reconnect on next need → teardown again. It is
demand-driven, not a spin — one teardown per request sent to that peer, each failing loudly —
but it also aborts that connection's healthy siblings each time. Whether to add a hold-off
after a fault teardown, shared by both causes, is deferred until measured; the protocol-fault
and deadline counters make such a loop visible at once.

## Receive-buffer sizing

Receive buffers are posted unconditionally, whether or not the local caller uses messaging
(FR-005), so the sizing applies to every connection.

With an 8 KiB buffer: `(8192 - 24) / 24` = **340 entries** per `OP_FETCH`. At 2.25 MiB
per block that is ~765 MiB of KV per message — far more than any single scheduler step will
ask for — and it bounds `OP_FETCH_REPLY` at `24 + 340` = 364 bytes.

Posted-receive memory per peer is `(message_credits + inbound_quota) x 8 KiB` — the reply
reserve plus the peer's quota (*Receive-buffer partitioning*). At the defaults of 8 credits
and an 8-request quota that is 16 buffers, 128 KiB per peer — still negligible, which is why
FR-005 can post unconditionally rather than negotiate.

`max_entries` is derived from the buffer size, not configured independently, and is what
`RequestTooLarge { max_entries }` reports. It is also what `OP_HELLO` advertises, so a peer
with a different buffer size is honoured rather than overrun — the two sides are **not**
assumed equal.

## Chunking

A fetch larger than the peer's `max_entries` is split by the **requester** into chunks, each
its own `OP_FETCH` message with **its own `context_id`**, each costing its own message credit.

- The **requester** chunks when `items.len() > max_entries` (the peer's, see `OP_HELLO`).
  It keeps the mapping from each chunk's `context_id` to the caller's fetch and that chunk's
  offset in the caller's item list. The `TransferContextId` that `fetch` returns names the
  whole fetch and stays a local value; when a fetch is not chunked the two may coincide, but
  nothing may assume they do. Chunking is entirely a requester-side concern: nothing on
  the wire says two messages belong to one fetch, and the source never needs to know.
- The **source never splits a reply.** Every `OP_FETCH` message draws **exactly one** answer
  — one `OP_FETCH_REPLY` covering all of that message's entries, or one `OP_ERROR` — echoing
  that message's `context_id`. A source short of bounce space or device readers (D10) serves the message's
  entries as buffers free up, chunking internally (FR-015), and still replies once; scarcity
  delays the reply, it never multiplies it.
- A fetch is complete when every one of its chunks has been answered.

**Why each chunk has its own id.** FR-013 lets the source serve messages concurrently, so
replies to the chunks of one fetch may arrive in any order. With one id per message, a reply
names exactly the request it answers, and the requester places its outcomes from that
chunk's recorded offset; nothing else on the wire is needed. A shared id plus a
first-entry-offset header field was the alternative, and it buys nothing a requester-side
table does not already give — while putting a second correlation key on the wire for every
message forever.

**Why exactly one reply per request.** The requester's receive reservation for replies
(*Receive-buffer partitioning*, below) is sized from its own message credits, which bound
its outstanding request *messages*. That bound only covers the replies if each message
draws a bounded number of them. With source-split replies, N outstanding requests could
produce more than N replies and overrun the reservation — the RNR stall the partition exists
to prevent. One reply per message makes the reservation exactly the credit count. The cost
is that a requester learns a large message's outcomes only when all of it is served; D19
already shows cross-message overlap is worth ~0.1%, and requester-side chunking still bounds
how much any one reply waits for.

Message credits are consumed per *message*, so a fetch chunked into N messages consumes N
credits and blocks when they are exhausted — which is the credit-based
pacing that answers the incast concern (D10). Because credits block only at zero, several
chunks of one fetch are normally in flight at once; that overlap is free and is **not** a
throughput lever worth engineering (D19 — the inter-message bubble is smaller than a single
entry's write). The barrier that does cost is inside a message, on the block-tier path, and
FR-022 forbids it.

Chunking is routine rather than exceptional: 340 entries is ~5,440 tokens at 16 tokens per
block, so any prefix longer than that is chunked. Issue the chunks in block-index order — a
prefix cache only benefits from a *contiguous* prefix, so a partial success should truncate
cleanly rather than leave a hole.

## Connection and lifecycle

- One queue pair per peer **instance** pair, either end able to post (D5). A peer is
  addressed as a normalized `"ip:port"`, so two instances on one host are distinct peers
  (FR-021, D16). The requester connects; the source replies and pushes over that same
  connection.
- Landing regions are registered once (`register_landing_region`), not per request;
  registration is what makes an `rkey` valid. The per-entry `rkey` on the wire selects
  among the requester's registered regions — it does not imply per-request registration.
  Registration lives in a long-lived protection domain and outlives any one queue pair, so
  an `rkey` already sent on the wire stays valid across a reconnect (D17). A retry after a
  forced teardown therefore needs no re-registration and no new addresses.
- A closed connection aborts every context outstanding on it. Each side releases its own
  resources: the source its pins and bounce leases (each only once its read or write has completed), the requester its landing slots — the
  latter only after teardown completes (FR-017).
- Loss of liveness **without** a close is not covered by the transport; it is the
  destination deadline's job (FR-016). On expiry the queue pair is forced to ERROR and
  drained *before* any landing slot is reclaimed, which necessarily aborts every context
  outstanding on that connection, not only the one that expired (FR-023, D18).

### Both directions, one connection

An instance may be destination for one context and source for another **on the same queue
pair** — that is the point of D5, and FR-002 allows only one connection per instance pair.
Three consequences:

- **Context ids are scoped per direction.** The requester allocates them, so both ends may
  independently allocate id 7 on one connection. An `OP_FETCH_REPLY` with id 7 belongs to the
  local outbound table; an `OP_FETCH` with id 7 belongs to the inbound serving table. They are
  distinguishable only by kind, so the two tables MUST be separate — one
  contexts-on-this-connection map would silently conflate two unrelated requests.
- **Message credits are per direction**, each bounding that end's sends against the peer's
  posted receives.
- **Replies must not be starved by inbound requests** — below.

### Receive-buffer partitioning

A posted receive buffer does not know what will land in it, and on a shared connection two
independent streams compete for the same pool: replies owed to this end, and requests
originated by the peer. Were only one buffer posted per own message credit, a peer issuing
that many requests could consume every one, leaving nowhere for a reply this end is blocked
waiting on. That is an RNR stall and, with traffic both ways, a credible deadlock.

So each end partitions its posted receives into two parts:

- a **reply reserve** of exactly one buffer per own message credit — each request message
  draws exactly one reply (*Chunking*, above), and credits bound the request messages
  outstanding, so this is exact rather than an estimate;
- an **inbound-request quota**, advertised as the peer's `max_outstanding_requests`.

Total posted = reserve + quota. A peer MUST NOT exceed the quota it was granted, so its
effective message credits are `min(its configured credits, the quota we advertised)`. The
partition is self-consistent precisely because each end sizes the reply reserve from a bound
it already enforces locally.

### Simultaneous connect

Either end may initiate (FR-001), so both can initiate at once and produce two queue pairs
between one instance pair. That is **not** benign: `disconnect` tears down *the* queue pair,
so a surviving second one could still land a write into a slot already reclaimed, defeating
FR-017 and the pin discipline of Principle IX.

The duplicate is resolved by the same device as the version — a deterministic function of two
values both ends know, with no arbitrator. `PeerId` has a total order, so both ends keep the
connection whose **initiator is the lower `PeerId`** and close the other. This requires the
initiator to identify itself in the connection request, which is what the connect
`private_data` carries (D16 puts a certus instance id there, replacing the predecessor's zyre
`PeerId`). Resolution happens at **accept**, before `OP_HELLO` is posted — and since no data
message may be sent before the exchange completes, the losing queue pair dies holding no
context and nothing has to be migrated.

## What is deliberately not here

- **No per-entry completion notification.** Subsumed by the reply ordering guarantee above.
- **No version discovery by trial.** The version is negotiated once per connection, via
  `OP_HELLO` — because the alternative (learning the peer's version from an error and retrying)
  requires re-framing and re-sending a request whose landing slots are already reserved and
  whose rkeys are already advertised, which is strictly more machinery than one exchange at
  connect. The exchange also makes `max_entries` explicit, which nothing previously checked.
  The structural guard is the header's `version` field, validated once against the
  negotiated version (D21): a message at the wrong version is refused, never misparsed.
- **No authentication or capability check.** Single-tenant assumption (spec Assumptions);
  RDMA write access into landing slots is accepted.
- **No peer discovery.** The peer is named by the caller, which got it from llm-d.
- **The shared-memory queue message** from the connector into the destination endpoint.
  That is the next layer up and is specified separately, per the bottom-up order: settle the
  transfer mechanism, then this framing, then the shm-queue message.
