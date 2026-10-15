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

Two opcodes is the whole protocol. Everything else the design needs — chunking, credits,
tier attribution — is carried in the header flags or the body, not in additional message
kinds.

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

This is exactly `(CacheKey, RemoteRegion)` on the wire, which is what
[itransfer_endpoint.md](./itransfer_endpoint.md) already passes to `fetch` and
`push_async`.

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

## Receive-buffer sizing

Receive buffers are posted unconditionally, whether or not the local caller uses messaging
(FR-005), so the sizing applies to every connection.

With an 8 KiB buffer: `(8192 - 24) / 24` = **340 entries** per `OP_FETCH`. At 2.25 MiB
per block that is ~765 MiB of KV per message — far more than any single scheduler step will
ask for — and it bounds `OP_FETCH_REPLY` at `24 + 340` = 364 bytes.

Posted-receive memory per peer is `message_credits x 8 KiB`. At 8 credits that is 64 KiB per
peer — negligible, which is why FR-005 can post unconditionally rather than negotiate.

`max_entries` is derived from the buffer size, not configured independently, and is what
`RequestTooLarge { max_entries }` reports.

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
pacing that answers the incast concern (D10).

## Connection and lifecycle

- One queue pair per peer pair, either end able to post (D5). The requester connects; the
  source replies and pushes over that same connection.
- Landing regions are registered once (`register_landing_region`), not per request;
  registration is what makes an `rkey` valid. The per-entry `rkey` on the wire selects
  among the requester's registered regions — it does not imply per-request registration.
- A closed connection aborts every context outstanding on it. Each side releases its own
  resources: the source its pins and bounce buffers, the requester its landing slots — the
  latter only after teardown completes (FR-017).
- Loss of liveness **without** a close is not covered by the transport; it is the
  destination deadline's job (FR-016).

## What is deliberately not here

- **No per-entry completion notification.** Subsumed by the reply ordering guarantee above.
- **No negotiation.** The fused opcode is matched or rejected, never negotiated. One implementation
  talks to itself across a homogeneous cluster.
- **No authentication or capability check.** Single-tenant assumption (spec Assumptions);
  RDMA write access into landing slots is accepted.
- **No peer discovery.** The peer is named by the caller, which got it from llm-d.
- **The shared-memory queue message** from the connector into the destination endpoint.
  That is the next layer up and is specified separately, per the bottom-up order: settle the
  transfer mechanism, then this framing, then the shm-queue message.
