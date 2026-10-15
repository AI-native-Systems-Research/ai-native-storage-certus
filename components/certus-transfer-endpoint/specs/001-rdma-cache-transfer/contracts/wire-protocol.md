# Contract: transfer wire protocol

**Feature**: `001-rdma-cache-transfer` | **Date**: 2026-10-15

Two verbs SEND messages over the symmetric connection of D5. Values themselves travel by
one-sided RDMA WRITE and never appear in a message. Interface contract:
[itransfer_endpoint.md](./itransfer_endpoint.md).

All integers little-endian. All offsets in bytes from the start of the message.

## Message framing

Every message begins with a 24-byte header whose first word is a **fused opcode carrying
the version in its high half**:

```
opcode = (version << 16) | kind
```

| name | `opcode` | meaning |
|---|---|---|
| `OP_FETCH` | `0x0001_0001` | kind 1, version 1 |
| `OP_FETCH_REPLY` | `0x0001_0002` | kind 2, version 1 |
| `OP_HELLO` | `0x0000_FFFE` | kind 0xFFFE, **unversioned** |
| `OP_ERROR` | `0x0000_FFFF` | kind 0xFFFF, **unversioned** |

Two *versioned* opcodes carry the data plane; everything it needs — chunking, credits, tier
attribution — rides in the header flags or the body rather than in more kinds.

The last two are **frozen for all time** at version `0`. No real version is 0, so these
opcodes can never collide with a versioned one, and that is deliberate: they are the messages a
peer must be able to parse *even when it cannot parse the version in question*. Versioning
them would mean answering an unparseable message with an unparseable error. They stay
extensible through their **content** — an open reason-code space and a counted capability list
— never through their layout (research D20).

### The frozen header prefix

Bytes 0-15 — `opcode`, `flags`, the first reserved field and `context_id` — mean the same
thing in **every version of every kind, forever**. That is the minimum a receiver needs to
reject a message it does not understand *and still correlate* the rejection with the request
that caused it. Later header fields and all bodies may be redefined by a version bump; these
may not.

A receiver dispatches on the whole word and rejects any value it does not know. Fusing the
version in is deliberate: a v2 `OP_FETCH` has a *different opcode* from a v1 `OP_FETCH`, so
an older peer cannot parse it as v1 and misread the body. A separate version field would make
that safety depend on every implementer remembering to check it; here an unknown version is
structurally an unknown opcode. It also leaves room to add message kinds later — a cancel,
a credit update — without touching the framing.

| offset | size | field | notes |
|---|---|---|---|
| 0 | 4 | `opcode` | fused kind + version, above |
| 4 | 2 | `flags` | bit 0 = `MORE` (another chunk of this context follows); rest reserved, MUST be zero |
| 6 | 2 | reserved | MUST be zero |
| 8 | 8 | `context_id` | identifies the fetch; echoed in `OP_FETCH_REPLY` |
| 16 | 4 | `n_entries` | entries in *this* message |
| 20 | 4 | reserved | MUST be zero; keeps the entry vector 8-byte aligned |

### `OP_FETCH` (`0x0001_0001`)

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

### `OP_FETCH_REPLY` (`0x0001_0002`)

Header, then one outcome byte per entry in request order, zero-padded to an 8-byte boundary.

Outcome byte: `0` = Miss, `1` = HitMemoryTier, `2` = HitBlockTier. Values >= 3 reserved; a
receiver MUST treat an unknown outcome as Miss rather than erroring, so a future source can
add a tier without breaking an old requester.

**Ordering guarantee**: every RDMA write for an entry reported as a hit MUST have completed
before the reply is sent. The requester may therefore treat reply arrival as the completion
signal for the whole message and needs no per-entry completion notification. This is what
makes the one-sided WRITE safe without `WRITE_WITH_IMM`.

### `OP_HELLO` (`0x0000_FFFE`)

Sent by **both** ends immediately on connection establishment — the endpoint is symmetric, so
neither side is the client (D5). Neither end sends a data message until the peer's `OP_HELLO`
has arrived. `context_id` is zero and `n_entries` is the capability count.

| offset | size | field | notes |
|---|---|---|---|
| 24 | 4 | `max_entries` | the largest `OP_FETCH` **this** sender can receive |
| 28 | 2 | `n_kinds` | capability entries following |
| 30 | 2 | reserved | MUST be zero |
| 32 | 4 × `n_kinds` | capabilities | `(kind: u16, max_version: u16)` each |

A requester MUST use `min(local, peer)` as the version for each kind, and MUST respect the
peer's `max_entries` rather than its own when chunking — the two sides are not required to
have equal receive buffers, and before this exchange existed that equality was an *unchecked
assumption* (D20). A kind absent from the list is unsupported by that peer. If a peer cannot
meet the configured version floor, the **connection** fails at establishment, naming peer,
kind and both versions; no request is ever issued to it.

Capabilities live on the connection and die with it, so a rebuilt connection re-learns them
and an upgraded peer is picked up with no separate invalidation step.

### `OP_ERROR` (`0x0000_FFFF`)

The answer to any message that cannot be processed. `n_entries` is zero.

| offset | size | field | notes |
|---|---|---|---|
| 24 | 4 | `offending_opcode` | the whole fused word as received |
| 28 | 2 | `reason` | below |
| 30 | 2 | `max_supported_version` | highest version of that *kind* the sender supports; 0 if the kind is unknown entirely |

| `reason` | meaning |
|---|---|
| 1 | unknown opcode — the kind itself is not recognised |
| 2 | unsupported version of a recognised kind |
| 3 | malformed message |
| 4 | `n_entries` exceeds the receiver's posted buffer |

Reason 0 is reserved and values ≥ 5 are available; a receiver MUST treat an unknown reason as
a generic protocol failure, never as success. `max_supported_version` exists so a requester can
go straight to a workable version instead of probing downward.

**Scope is protocol faults only.** A miss is an outcome byte, not an error, and a resource
shortage chunks (FR-015). Using `OP_ERROR` for either would quietly become the escape hatch
that undermines D12 and FR-015.

**Blast radius.** Receiving `OP_ERROR` fails **that one context** and leaves the connection
usable: the peer is healthy and answered promptly, so the FR-023 deadline teardown must *not*
run and sibling contexts on the connection must be unaffected. This is the deliberate contrast
with a deadline, where the peer's state is unknown and tearing the queue pair down is the only
thing that makes a late write impossible. Two fail-loud paths, two blast radii.

## Receive-buffer sizing

Receive buffers are posted unconditionally, whether or not the local caller uses messaging
(FR-005), so the sizing applies to every connection.

With an 8 KiB buffer: `(8192 - 24) / 24` = **340 entries** per `OP_FETCH`. At 2.25 MiB
per block that is ~765 MiB of KV per message — far more than any single scheduler step will
ask for — and it bounds `OP_FETCH_REPLY` at `24 + 340` = 364 bytes.

Posted-receive memory per peer is `message_credits x 8 KiB`. At 8 credits that is 64 KiB per
peer — negligible, which is why FR-005 can post unconditionally rather than negotiate.

`max_entries` is derived from the buffer size, not configured independently, and is what
`RequestTooLarge { max_entries }` reports. It is also what `OP_HELLO` advertises, so a peer
with a different buffer size is honoured rather than overrun — the two sides are **not**
assumed equal.

## Chunking and the `MORE` flag

A fetch larger than `max_entries`, or one the source cannot satisfy at once for want of
bounce buffers (D10), is split. Chunks of one context share a `context_id`; every chunk but
the last sets `MORE`.

- The **requester** chunks when `items.len() > max_entries`.
- The **source** MAY further split its *replies* — it sends a reply per batch it completes,
  each covering a contiguous run of that request's entries, with `MORE` set on all but the
  last.
- A `context_id` is complete when a reply without `MORE` has been seen and the outcome
  counts sum to the number of entries requested.

Message credits are consumed per *message*, not per context, so a chunked fetch of N
messages consumes N credits and blocks when they are exhausted — which is the credit-based
pacing that answers the incast concern (D10). Because credits block only at zero, several
messages of one context are normally in flight at once; that overlap is free and is **not** a
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
  resources: the source its pins and bounce buffers, the requester its landing slots — the
  latter only after teardown completes (FR-017).
- Loss of liveness **without** a close is not covered by the transport; it is the
  destination deadline's job (FR-016). On expiry the queue pair is forced to ERROR and
  drained *before* any landing slot is reclaimed, which necessarily aborts every context
  outstanding on that connection, not only the one that expired (FR-023, D18).

## What is deliberately not here

- **No per-entry completion notification.** Subsumed by the reply ordering guarantee above.
- ~~**No negotiation.**~~ **Superseded by D20.** The original stance was that the fused opcode
  is matched or rejected and never negotiated. It now *is* negotiated, once per connection, via
  `OP_HELLO` — because the alternative (learning the peer's version from an error and retrying)
  requires re-framing and re-sending a request whose landing slots are already reserved and
  whose rkeys are already advertised, which is strictly more machinery than one exchange at
  connect. The exchange also makes `max_entries` explicit, which nothing previously checked.
  The fused opcode is unchanged and still does the structural work: a version bump *is* an
  opcode change, so an unknown version cannot be misparsed as a known one.
- **No authentication or capability check.** Single-tenant assumption (spec Assumptions);
  RDMA write access into landing slots is accepted.
- **No peer discovery.** The peer is named by the caller, which got it from llm-d.
- **The shared-memory queue message** from the connector into the destination endpoint.
  That is the next layer up and is specified separately, per the bottom-up order: settle the
  transfer mechanism, then this framing, then the shm-queue message.
