# Research: Explicit certus-to-certus KV cache transfer

**Feature**: `001-rdma-cache-transfer` | **Date**: 2026-10-14

Decisions already settled, with the alternatives rejected and why. Recorded so they are
not re-litigated.

## D1 — Push, not pull

**Decision**: the destination reserves landing slots and sends their descriptors; the
**source** RDMA-writes into them.

**Rationale**: it narrows the source's pin window to a single local write with no network
round trip inside it — in a pull model the pin spans at least one RTT plus the reader's
scheduling delay. It also matches what the existing crates already do (see D3).

**Also considered**: pull (destination RDMA-READs from the source). Rejected: longer pin
window, and it would require the source to advertise addresses and hold them across a round
trip.

**Precedent**: vLLM ships both. `NixlPushConnector` has the destination stash "D's identity
+ locally allocated block IDs" and notify the source, which then matches and writes — the
same shape.

## D2 — Hidden from vLLM; a transfer is tier population

**Decision**: the transfer makes keys appear in the local memory tier. Nothing else changes.

**Rationale**: once resident, `lookup()` reports RESIDENT and the ordinary
`prepare_load` → Lookup → CUDA-IPC DMA path serves the GPU unmodified. The **only**
vLLM-visible effect is a deferral: return `None` from `get_num_new_matched_tokens` while the
fetch runs, report the hit on a later scheduler pass. vLLM specifies *what* must be present
before the layers read, never *how* it arrives.

**Consequence**: no vLLM-side involvement on the source at all — no `request_finished`, no
`delay_free_blocks`, no handshake metadata. This is available to us only because certus is
write-through, so the source's tier already holds the data independent of any request. Nixl
cannot do this; its rendezvous machinery exists precisely because the prefill side's KV does
not yet exist when the decode side registers.

## D3 — The existing crates are already a push design

**Finding**: the names are the opposite of intuition.
`remote-lookup-rdma-initiator` is the **source/holder** — "given a remote host endpoint and
a batch of `(key, remote-region)` pairs … RDMA-writes the value directly into the remote
host's memory". `remote-lookup-rdma-responder` is the **destination/requester** — "offers
local memory for a peer to write it into … never touches the data".

**Already present and reusable**: push direction; `push_async(endpoint, &[(CacheKey,
RemoteRegion)], on_complete)` plus a blocking `push` returning one `PushStatus` per item in
order; connection table with reuse; send-queue credits + `wr_id` slab + partial batch
posting; **pins held across the write** (an in-flight write "holds its submitter's
resources (typically read pins, which block eviction)"); and the
**teardown-before-reclaim barrier** (`Disconnect{node}` / `DisconnectAck` tears down a
peer's QP *before* the requester reclaims its landing slots).

**Genuinely absent**: any send/recv messaging. `IBV_WR_SEND` is a bare constant in
`ffi.rs`; `post_recv` appears only in spec documents, never in code. Today the request
arrives over zyre and `remote-lookup` calls `push`.

## D4 — One new component subsuming both old crates, with a phased cutover

**Decision**: build `certus-transfer-endpoint` designed for the new path, exposing a subset
usable by the old. Re-point `remote-lookup` and retire both old crates as a **later** task.

**Rationale**: the duplication argument cuts this way. `ffi.rs` already declares **28
identical public items** across the two existing crates, so a third copy would be the worst
outcome. Both crates already have `rdma_connect`, `rdma_listen` *and* `rdma_accept`, so the
connection-management surface for symmetric endpoints largely exists; the only real role
difference is that the responder never posts work requests. `rdma.rs` shares **zero** public
items, so those wrapper layers are role-specific and get combined rather than deleted.

**Phasing** satisfies "do not break remote-lookup now" without accepting permanent
duplication: prove the new engine first, cut over second, delete third.

## D5 — Symmetric endpoints

**Decision**: one queue pair per peer pair; a QP may be inbound-accepted or
outbound-initiated, and **either** may post work requests.

**Rationale**: the destination connects and sends the request, and the source must reply and
push **over that same connection**. Today neither crate can do that — the responder never
posts, the initiator only connects outbound.

**Also considered**: two connections per pair (dest→source for the message, source→dest for
data), preserving the current role split. Rejected: two QPs and two setups per pair, and the
reply still needs a direction.

**Precedent**: vLLM's p2p tier — "a single bidirectional `P2PSession` per remote peer
handles both client-role and server-role over the same control connection".

## D6 — Residency and pin are one atomic operation

**Decision**: obtain location and pin in a single dispatch-map lookup. Never probe-then-pin.

**Rationale**: the dispatch map "increments `read_ref` **before** it inspects the location,
so `MemoryTier` and `BlockDevice` answers both come back pinned, while `NotExist` and
`MismatchSize` return earlier, unpinned". So there is no window — provided we do not
introduce one. Using `Check` to decide the tier first *would* introduce it, because `Check`
releases immediately.

**Corollary**: never release a read reference for an answer that was never pinned; doing so
decrements another reader's pin. This is the live `Dispatcher::check()` defect (it computes
`exists = !matches!(result, NotExist)` and releases on that), deferred separately.

## D7 — Per-entry pin lifetime, with early release on the block-tier path

**Decision**: pin and release **per entry**, not per batch. Memory-tier pins release when
the RDMA write completes (matched by `wr_id`). Block-tier pins release as soon as the device
read lands in the bounce buffer — *before* the write is posted.

**Rationale**: landing slots are not contiguous, so there is one RDMA write per entry
anyway (FR-008); pin lifetime therefore has no reason to span the batch. On the block-tier
path the data has been *copied*, so the entry needs no further protection — only the buffer
does. Concurrent pins are already bounded by the send-queue credit limit, which exists to
stop "the backlog of held read pins growing without bound", so no separate pin accounting
is needed.

## D8 — Bounce buffers, not promotion, for block-tier serving

**Decision**: read a block-tier entry into a buffer from a registered pool and push from
there. Do **not** promote it into the source's memory tier.

**Rationale**: promotion would evict the source's own working set — serving a peer would
degrade the server's hit rate, which creates bad incentives and would eventually need
accounting or throttling to compensate. With bounce buffers, serving a peer costs device read
bandwidth and NIC only. This extends llm-d's p2p property ("the source pod's GPU is never
touched, so serving a pull costs the source no prefill capacity") to cache quality as well.

**Note**: neither vLLM surface can express this. `SecondaryTierManager` forces all data
through the CPU primary tier; `OffloadingSpec` has no vocabulary for a scratch read.

## D9 — No peer-to-peer DMA / "direct from SSD"

**Decision**: stage through a host bounce buffer. Do not attempt NVMe→NIC P2PDMA.

**Rationale**: flash is not memory — the NIC cannot pull bytes out of NAND. "Direct" can
only mean staging in an NVMe CMB/PMR, which *is* a bounce buffer, merely on-device and
typically tens of MB shared across queues. The real comparison is host-DRAM staging versus
device-CMB staging: one DMA hop. Current path is NVMe DMA → hugepage, NIC DMA → wire: **two
DMAs, zero CPU copies**, already near the floor.

P2PDMA would require relaxed/disabled ACS on intervening bridges, a permitting PCIe
topology, kernel/SPDK enablement, and a usefully large CMB — platform-fragile in a way that
could invalidate a benchmark rather than improve one.

**The architectural argument is stronger than the performance one**: the bounce buffer
*decouples* the device read from the RDMA, and that decoupling is what makes early pin
release (D7), chunking (D10) and cheap RDMA retry expressible. Coupled, a failed RDMA means
re-reading flash and the pin spans both.

**And it is the wrong path to optimise**: measured workload-generator runs show single-digit
percent SSD hit rates, and a well-sized memory tier avoids them altogether.

**Revisit if**: a deliberately small-DRAM/large-SSD configuration makes block-tier serving
the common case. Cheaper lever first: speculative promotion on llm-d's source-selection
hint (costs tier capacity, reintroduces the pollution D8 avoids).

## D10 — Three credit domains, and chunking as flow control

**Decision**: bound three resources separately, with distinct names:

| domain | status | bounds |
|---|---|---|
| send-queue credits | **exists** | in-flight RDMA writes per connection, transitively held read pins |
| message credits | new | outstanding requests against the peer's posted receive buffers |
| bounce-buffer occupancy | new | concurrent block-tier-served entries |

**Rationale**: only the middle one was in the original sketch, and the existing code already
uses the word "credits" for the first. Conflating them would be easy and confusing.

**Chunking is not only a fallback**: chunk grants from the destination are credit-based
pacing, which answers the incast concern (a push design otherwise moves rate control to the
sender), and it shrinks over-reservation against llm-d's lagging index to one chunk's worth
rather than the whole prefix.

## D11 — Fail loudly; graceful degradation deferred

**Decision**: a destination-side deadline detects a request neither fulfilled nor refused.
Default action: log full state and fail. The degrading action is deferred.

**Rationale**: two of the three triggers originally proposed for this deadline are not
timeout cases at all — "source lacks the keys" is a *reply*, and "rendezvous never matches"
does not exist here (D2). What remains is defence against a certus bug or a partition, which
in a research posture should be visible, not papered over. This project has already paid for
the alternative: `needs_disable_async_scheduling` was "verified @0.22", never re-checked, and
silently cost throughput; the `touch()`/`lookup()` ordering is an optimisation silently not
firing with correctness intact and invisible in every metric.

**But note**: *omitting* the deadline does not produce a loud failure. vLLM defers the one
request (`pop_request` + `prepend_request` + `continue`) and keeps serving everything else —
one stuck request on a healthy-looking engine. Hence a failing deadline rather than none.

**The stronger defence is harness hygiene**: always report attempted-vs-completed and fail
the run on any error, so a dropped request cannot inflate a latency-over-completions number.

## D12 — No fallback to broadcast on miss

**Decision**: a key the named source does not hold is reported `miss`. The source does not
forward to broadcast remote-lookup.

**Rationale**: this mechanism is the *alternative* to broadcast/reap, not a layer above it.
Mixing them would reintroduce the O(nodes) fan-out the design exists to avoid, and make
results uninterpretable.

## Plan-phase questions, resolved

OQ1 and OQ2 are resolved below as **D13** and **D14**, and OQ3 as **D15**. **OQ4** — whether
`PushStatus` extends to carry the tier distinction or a new outcome type appears alongside it
— is resolved by `contracts/itransfer_endpoint.md` ("Relationship to
`IRemoteLookupRdmaInitiator`"): `PushStatus` widens to `EntryOutcome`, and can collapse back
to `PushStatus` at the boundary at cutover time, since the legacy caller does not use the tier
information. FR-019 compatibility is therefore unaffected.

Only **OQ5** remains open, deliberately, and only in part — see *Deferred to the next layer
up* at the end of this document.

## D13 — Receptacle set: look up through the dispatch map, not the memory tier

**Resolved** (was OQ1). The existing initiator has only `logger: ILogger` and
`memory_tier: IMemoryTier`, which is why it is memory-tier-only — `IMemoryTier` cannot
express tri-state residency. This component needs:

| receptacle | purpose | gated |
|---|---|---|
| `logger: ILogger` | as today | no |
| `dispatch_map: IDispatchMap` | **atomic lookup+pin**, and it returns the tier *and* the block offset in one call (`MemoryTier{pointer,size}` / `BlockDevice{offset}` / `NotExist` / `MismatchSize`) | no |
| `block_device` | the device read for block-tier serving | **yes** |

The dispatch map is what makes D6 and D7 possible: one call yields residency, the source
address (for memory-tier) or the extent offset (for block-tier), *and* the pin. No separate
probe, no separate pin call.

Block reads go over `connect_client() -> ClientChannels` — the actor-per-controller model
with shared-memory client channels, per `CLAUDE.md` — not through direct trait methods. So
the device-read path is: `dm.lookup(key)` → `BlockDevice{offset}` (pinned) → submit a read
of that offset into a bounce buffer over the client channel → release pin on read completion
→ post the RDMA write.

## D14 — Feature matrix, and the default build's honest degradation

**Resolved** (was OQ2). Three independent features, none in `default`:

| build | serves | transport |
|---|---|---|
| `default` | memory tier only | in-process mock seam |
| `rdma` | memory tier only | real libibverbs/librdmacm |
| `rdma,spdk` | memory **and** block tier | real verbs |

A build without `spdk` has no `block_device` receptacle bound, so a `BlockDevice{offset}`
answer is reported as **`miss`** rather than failing. That is a clean degradation and
exactly what Phases 1–3 need: the whole messaging and pin-discipline surface is testable
with neither rdma-core nor SPDK present, satisfying SC-003 and CLAUDE.md's default-members
rule. Block-tier serving (Phase 4, P3) is the only thing that requires SPDK.

## D15 — Bounce-pool registration: follow the memory tier, which is already dual-registered

**Resolved** (was OQ3). No new mechanism is needed; the precedent is exact.

The memory tier is allocated with `spdk_zmalloc` — "hugepage-backed, zero-initialized
memory", NUMA-aware (`components/memory-tier/src/lib.rs:281-297`) — so it is SPDK-DMA-able
by construction, which is how promotion reads device data into it today. And the existing
initiator already registers that same region for RDMA: `rdma.rs` has a `MemoryRegion` whose
registration path covers memory "owned by the caller (e.g., memory-tier pool)".

**So the memory tier is already both SPDK-DMA-able and `ibv_reg_mr`-registered.** The bounce
pool should be allocated and registered the same two ways: `spdk_zmalloc` on the NUMA node of
the serving instance, then `MemoryRegion`-registered over caller-owned memory.

`gpu-services::prepare_memory_for_spdk` is **not** needed — it addresses making *GPU* memory
SPDK-DMA-able, a different problem.

## Upstream change picked up 2026-10-14 (rebase onto unstable)

The connector fix for issue **#541** (the `touch()`/`lookup()` ordering) landed as
`d3f08a9d`, together with a fused **`OP_TOUCH_CHECK = 16`** opcode (`a7e0ffbf`), a bulk
codec for `touch()`+`lookup()` (`2fae06c8`) and a fallback for servers lacking TouchCheck
(`6d6a7174`). Two corrections to what #541 recorded:

* **The premise held only on vLLM <= 0.20**, not "pre-0.30" as the issue guessed. Upstream
  PR #41228 reversed the order in **0.21** and it has been that way since.
* **The severity was understated.** #541 said correctness was unaffected. In fact a *stale
  positive* — cache filled by one request, read by the next in a later scheduler step after
  the block was evicted — "makes vLLM schedule a load whose Pin fails, and a failed load
  stops vLLM." It was a latent engine-stop, not merely a lost optimisation.

The fix keeps the cache but trusts an entry for only 1 ms. Note for this component: the
wire op list now has 16 opcodes, and any documentation here that enumerates them should
include TouchCheck.

## Deferred to the next layer up

- **OQ5 (in part) — the shared-memory queue message** from the connector into the destination
  endpoint. Deliberately deferred, per the bottom-up order: settle the transfer mechanism
  first, then the framing, then this. Tracked as task T067.

The other half of OQ5 — **message framing and receive-buffer sizing** — is now **settled** in
`contracts/wire-protocol.md`: a 24-byte header with a fused `(version << 16) | kind` opcode,
24-byte entries of `(key, addr, size, rkey)`, and 340 entries per `OP_FETCH` in an 8 KiB
buffer, with `max_entries` derived from the buffer size rather than configured.
