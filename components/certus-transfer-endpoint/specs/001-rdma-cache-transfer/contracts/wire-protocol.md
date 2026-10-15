# Contract: transfer wire protocol

**Feature**: `001-rdma-cache-transfer` | **Date**: 2026-10-15

Two verbs SEND messages over the symmetric connection of D5. Values themselves travel by
one-sided RDMA WRITE and never appear in a message. Interface contract:
[itransfer_endpoint.md](./itransfer_endpoint.md).

All integers little-endian. All offsets in bytes from the start of the message.

## Message framing

Both messages share an 8-byte prologue so a receiver can dispatch before parsing:

| offset | size | field | notes |
|---|---|---|---|
| 0 | 1 | `msg_type` | `1` = Request, `2` = Reply |
| 1 | 1 | `version` | `1`. A receiver MUST reject a version it does not know. |
| 2 | 2 | `flags` | bit 0 = `MORE` (another chunk of this context follows); rest reserved, MUST be zero |
| 4 | 4 | `n_entries` | entries in *this* message |

### Request (`msg_type = 1`)

| offset | size | field |
|---|---|---|
| 0 | 8 | prologue |
| 8 | 8 | `context_id` |
| 16 | 8 | `base_addr` — landing region base on the **requester** |
| 24 | 4 | `rkey` — requester's rkey for that region |
| 28 | 4 | `entry_size` — bytes per entry; uniform within a message |
| 32 | 16 × `n_entries` | entry vector |

Entry (16 bytes):

| offset | size | field |
|---|---|---|
| 0 | 8 | `key` — the 64-bit block key |
| 8 | 4 | `slot_index` — index of the reserved landing slot |
| 12 | 4 | reserved, MUST be zero |

Target address for entry *i* is `base_addr + slot_index[i] * entry_size`. The source
performs that arithmetic; the requester never sends per-entry addresses. One RDMA write is
issued per entry because slots are not contiguous (FR-008) — `slot_index` is an index into
the requester's slot table, **not** a byte offset, and consecutive indices carry no
implication of adjacency.

### Reply (`msg_type = 2`)

| offset | size | field |
|---|---|---|
| 0 | 8 | prologue |
| 8 | 8 | `context_id` — echoes the request |
| 16 | 1 × `n_entries` | outcome vector, one byte per entry, request order |
| … | 0–7 | zero padding to an 8-byte boundary |

Outcome byte: `0` = Miss, `1` = HitMemoryTier, `2` = HitBlockTier. Values ≥ 3 reserved; a
receiver MUST treat an unknown outcome as Miss rather than erroring, so a future source can
add a tier without breaking an old requester.

**Ordering guarantee**: every RDMA write for entries reported as a hit MUST have completed
before the reply is sent. The requester may therefore treat reply arrival as the completion
signal for the whole message and needs no per-entry completion notification. This is what
makes the one-sided WRITE safe without `WRITE_WITH_IMM`.

## Receive-buffer sizing

Receive buffers are posted unconditionally, whether or not the local caller uses messaging
(FR-005), so the sizing applies to every connection.

With an 8 KiB buffer: `(8192 − 32) / 16` = **510 entries** per request message. At 2.25 MiB
per block that is ~1.1 GiB of KV per message — comfortably more than any single scheduler
step will ask for, and it bounds the reply at `16 + 510` = 526 bytes.

Posted-receive memory per peer is therefore `message_credits × 8 KiB`. At 8 credits that is
64 KiB per peer — negligible, which is why FR-005 can post unconditionally rather than
negotiate.

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
- Landing regions are registered once (`register_landing_region`), not per request. Only
  `rkey` and `base_addr` travel, per message — the same economy vLLM's Nixl connector uses,
  where the address space is exchanged once at handshake and only block indices travel per
  request.
- A closed connection aborts every context outstanding on it. Each side releases its own
  resources: the source its pins and bounce buffers, the requester its landing slots — the
  latter only after teardown completes (FR-017).
- Loss of liveness **without** a close is not covered by the transport; it is the
  destination deadline's job (FR-016).

## What is deliberately not here

- **No per-entry completion notification.** Subsumed by the reply ordering guarantee above.
- **No negotiation.** `version` is checked and rejected, not negotiated. One implementation
  talks to itself across a homogeneous cluster.
- **No authentication or capability check.** Single-tenant assumption (spec Assumptions);
  RDMA write access into landing slots is accepted.
- **No peer discovery.** The peer is named by the caller, which got it from llm-d.
- **The shared-memory queue message** from the connector into the destination endpoint.
  That is the next layer up and is specified separately, per the bottom-up order: settle the
  transfer mechanism, then this framing, then the shm-queue message.
