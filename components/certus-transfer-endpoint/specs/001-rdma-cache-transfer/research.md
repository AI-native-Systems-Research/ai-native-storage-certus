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

## D16 — Peer identity is an instance, not a host

**Decision**: a peer is addressed at instance granularity by a normalized `"ip:port"`, which
is the connection-table key. A host without a port is rejected, never defaulted.

**Rationale**: several certus instances run per node — one per NUMA domain in the current
rigs — so a host is not an address. Keyed by host, two NUMA-pinned instances on one node
collapse into a single connection and one of them becomes unreachable. The same-host case is
not hypothetical: a cross-NUMA transfer between two instances on one node is a legitimate
transfer, and single-host loopback is the hardware verification path, so both ends of a
connection may share an IP and differ only by port.

**The precedent already does this**: `remote-lookup-rdma-initiator/src/connection.rs:1075`
has `parse_endpoint(endpoint) -> (String, u16)` requiring `"ip:port"` and rejecting both a
missing port and port 0, and the table is keyed by `format!("{addr}:{port}")`
(`:479-480`, and the doc comment at `:446` — "a table of outbound RDMA connections keyed by
`"ip:port"` endpoint"). Reuse `parse_endpoint` rather than inventing a second grammar.

**Also considered**: an opaque instance id, resolved to a transport address separately.
Rejected for now — it is the more honest model if an instance ever becomes reachable at more
than one IP, but nothing in the current rigs needs it, and it would add a resolution step and
a registry this component deliberately does not have (it does not discover peers). Revisit if
multi-homed instances appear.

**Consequence for inbound correlation**: the responder stamped its zyre `PeerId` bytes into
each connect's `private_data` (`connection.rs:1102`) to correlate an accepted connection with
a node. FR-004 forbids zyre here, so if inbound correlation is wanted, that field carries a
certus instance id instead.

## D17 — One long-lived protection domain; registrations outlive queue pairs

**Decision**: the endpoint owns one protection domain per device for its lifetime. Landing
regions are registered once into that PD and stay registered; queue pairs are created,
destroyed and rebuilt beneath them. An `rkey` therefore survives a reconnect.

**Rationale**: `contracts/itransfer_endpoint.md` already promises that landing regions are
registered once, not per request, and that registration is what makes an `rkey` valid. That
promise is only true if MR lifetime is decoupled from QP lifetime. It also makes recovery
cheap: after a deadline forces a teardown (FR-023), a retry needs no re-registration and the
addresses and rkeys already sent remain valid.

**This differs from the precedent, deliberately.** In the initiator the PD is per-connection
(`src/rdma.rs:169`, with `ibv_dealloc_pd` in its `Drop`) and the memory-tier pool is
re-registered on *every* connect (`src/connection.rs:1131`; see the doc comment at `:1143`,
"a live `rdma_cm` connection plus its registered memory-tier pool region"). There, MR
lifetime equals connection lifetime and an rkey changes across a reconnect. That is tolerable
for a crate that only ever pushes from its own pool, and intolerable here, where the
destination has *already advertised* per-entry rkeys inside an in-flight request.

**Also considered**: keeping the per-connection PD and re-validating `LandingRegionHandle`
after every reconnect. Rejected: it makes every in-flight request's rkeys stale on
reconnect, turns the handle into something callers must re-check, and contradicts the
contract's "registered once".

**Note**: an MR belongs to a PD, not to a QP, so this is the natural verbs model rather than a
trick; the precedent's coupling is an artifact of letting `rdma_cm` own the PD per connection.

**Additional benefit, independent of rkey stability**: re-registration disappears from the
reconnect path. `ibv_reg_mr` pins pages and builds the device's translation entries, so
registering a multi-GB memory tier is slow — and the precedent pays it on *every* connect.
With one long-lived PD a reconnect creates a queue pair and nothing else.

**Accepted drawbacks**, none of which threaten the decision:

1. **The PD must be owned explicitly, and keyed per verbs context.** `rdma_cm` will create a
   PD per `rdma_cm_id` if allowed to; using our own means passing it to `rdma_create_qp`
   instead of accepting `id->pd`, and the PD must belong to the *same device context* as the
   resolved id. So the PD is allocated lazily per `ibv_context` after address resolution, and
   a node with several RDMA devices holds one PD per device — hence "one PD per device"
   rather than one per process.
2. **A PD is the protection boundary.** Every queue pair in it can reach every memory region
   in it, so one shared PD means any connected peer can address any registered landing
   region, not merely the regions intended for it. This is acceptable *only* under the
   single-tenant trust assumption the spec already states. If cross-tenant isolation is ever
   wanted, per-peer PDs are the mechanism, and this decision is what would have to be
   revisited first.
3. **Destruction ordering becomes a real constraint.** `ibv_dealloc_pd` fails with `EBUSY`
   while any queue pair or memory region still references the PD, so shutdown must tear down
   every connection and deregister every region before the PD goes. The precedent gets this
   for free by construction.
4. **`MemoryRegion`'s thread-safety justification changes.** The initiator's
   `unsafe impl Send for MemoryRegion` is justified by "only accessed from the thread that
   created it" (`src/rdma.rs:126`). A region registered once and used by several per-peer
   connection threads is no longer that, so the `// SAFETY:` comment must be rewritten to
   justify the sharing it actually does — and `Sync` may be needed alongside `Send`.
   Principle V makes an inaccurate safety comment a defect in its own right.

**Not drawbacks, though they look like candidates**: device resource limits (one PD holding N
regions is strictly cheaper than N PDs holding N regions); PD-level error states (queue pairs
enter ERROR, protection domains have no such state); and recovery from a device reset or
hot-remove (fatal async events invalidate every region and queue pair either way).

## D18 — Deadline cleanup: make writes impossible before reclaiming, and accept a peer-wide blast radius

**Decision**: on expiry — queue pair to ERROR, drain and discard completions, reclaim landing
slots, then reconnect lazily on next need. Expiry aborts **every** context outstanding on
that connection, and that is logged rather than designed around.

**Rationale**: FR-016 as originally written required only that queue-pair state be *logged*.
Logging it is not changing it. Fail the request, release the slots, and a slow-but-live
source's write lands in a slot since re-reserved for another entry — a silent corruption of
an unrelated reader, which is exactly the class of bug Principle IX exists to prevent and
exactly what FR-017's teardown-before-reclaim barrier already demands for the ordinary path.
The deadline path simply was not wired to it. Transitioning to ERROR is what actually makes a
further write impossible; nothing weaker is sufficient.

**Why peer-wide**: one queue pair per peer pair (D5) means a single QP carries every request
to that instance, so tearing it down necessarily aborts the siblings. The alternative — a QP
per request, so expiry is isolated — multiplies queue pairs by request concurrency and was
already rejected in D5 for a weaker reason. In a research posture an over-broad abort that is
logged is strictly better than a narrow one that risks a silent write into live memory.

**Precedent to reuse, on the source side**: `STALL_TIMEOUT = 2s`
(`remote-lookup-rdma-initiator/src/connection.rs:116`) caps how long a posted write may go
uncompleted before its connection is declared stuck and rebuilt, because `rdma_cm` leaves the
hardware ACK timeout at its large default and a stale-but-warm connection would otherwise
burn ~15s of retransmit before `RETRY_EXC`. It is measured from the batch's *first posted*
write, not from submission, so a queued batch is not mistaken for a stuck one.
`ensure_connected()` (`:1009`) already drops a stale or error-state connection before
reconnecting, and there is a one-repair replay budget with tests. That is a **source-side
stall cap**; FR-016's is a **destination-side reply deadline**. They are different mechanisms
and must not share a name or a knob.

## D19 — Scale separation: fix only the barrier that costs milliseconds

**Decision**: require the source to avoid phase barriers *within* a request (FR-022). Do
**not** build cross-message pipelining. Take the cross-message overlap that message credits
already give for free, and engineer nothing further for it.

**Rationale**: the magnitudes are three orders of magnitude apart, so only one of them can
matter.

| quantity | at 50G (measured rig) | at 200G |
|---|---|---|
| one `OP_FETCH` of 340 × 2.25 MiB | ~130 ms | ~30 ms |
| one 2.25 MiB entry write | ~380 us | ~90 us |
| inter-message bubble (reply RTT + lookup) | ~10-50 us | ~10-50 us |
| a device read on the block-tier path | ~100 us - ms | ~100 us - ms |

Both columns are given because the figure matters to the argument and the two rigs differ:
the active port on the current node is **50 Gbps** (see *Rig facts* below), while the
precedent's comment assumes 200G RoCE. The conclusion is invariant, and is in fact *stronger*
at the lower rate — the bubble is 0.008-0.04% of a message at 50G versus ~0.1% at 200G, and
it stays well below the cost of a single entry's write either way. So overlapping messages
buys essentially nothing at any link rate this design will see. Message credits default to 8 and block only at zero, so that overlap already exists at
no cost; it just must not drive design. The same arithmetic disposes of deep send-queue
pipelining: at 2.25 MiB per entry this path is bandwidth-bound and a window of 2-4 saturates
the link.

Note the one place the margin *narrows* at 50G: a device read (~100 us - ms) stops being much
larger than a single entry's write (~380 us) and becomes comparable to it. That does not
change the conclusion below — a lease-all/read-all/post-all barrier still idles the wire for
the whole read phase, whatever one read costs — but it does mean the block-tier barrier is the
only one whose cost stays in the same order of magnitude as the transfer itself.

**What does matter** is the block-tier read barrier. A source that leases N bounce buffers,
reads all N, posts all N writes and only then replies idles the wire for the entire read
phase — the one barrier whose cost is measured in milliseconds. D7's early pin release and D9's
decoupling argument already make the pipelined form expressible ("the bounce buffer
*decouples* the device read from the RDMA, and that decoupling is what makes early pin
release, chunking and cheap RDMA retry expressible"); FR-022 now requires it.

**Note on request length**: multi-message fetches are the normal case, not a rarity. 340
entries is ~5,440 tokens at 16 tokens per block, so any prefix longer than that chunks; the
24k-token example in plan.md is ~5 messages. The numbers are self-consistent — Qwen3-8B at
2 x 36 layers x 8 KV heads x 128 head_dim x 2 bytes = 144 KiB/token, x 16 tokens = 2.25 MiB
per block, x 1,536 blocks = 3.375 GiB. So chunking is routine; it is simply not a throughput
problem.

**And ordering is not a constraint on any of this**: outcome-vector order is index
correspondence (contract invariant 1), not temporal order, and nothing in the design requires
writes or replies to arrive in order — the destination cannot observe write order at all,
which is why one-sided writes need no `WRITE_WITH_IMM`. The one real ordering concern lives in
the *consumer*: a prefix cache only benefits from a contiguous prefix, so a miss at block *k*
makes later blocks useless for that request (they stay cached for later). Its consequence is
the opposite of serialising — issue a chunked prefix in block-index order, so a partial
success truncates cleanly into a usable prefix.

## D20 — One negotiated protocol version, exchanged eagerly, with a frozen error reply

**Revised 2026-10-15**, same day as first written — see *Revision* at the end of this
decision. The first version of D20 made versioning **per message kind**; it is now a single
protocol version. The analysis of the rejected alternatives below is unchanged and still
load-bearing.

**Decision**: three parts.

1. **One protocol version for the whole wire protocol**, negotiated per connection. Each end
   advertises the range `[min, max]` it supports; the agreed version is `min(a.max, b.max)`,
   and the connection fails if that falls below `max(a.min, b.min)`.
2. **Capabilities are exchanged eagerly**, once per connection, before any data message: the
   version range and the largest request the sender can receive. A requester respects the
   peer's maximum request size rather than its own.
3. **A protocol error reply exists and is frozen forever**, as the backstop for anything the
   exchange cannot prevent.

The fused opcode is **kept**, now carrying the negotiated version, but its job has changed: it
is no longer the versioning mechanism, it is a structural guard on it. Both ends agreed on a
version, so every message should already be at that version; if a negotiation bug or a
mismatched build emits something else, the receiver rejects a whole unknown word rather than
misparsing a known one. It costs nothing — the version occupies bits of a word already on the
wire — and it keeps `OP_ERROR`'s "unsupported version" reason meaningful.

**Why an error reply at all, independent of versioning**: "rejects any value it does not know"
had no expression on the wire, so an unknown opcode was byte-identical to a hang. With FR-023
in place that is actively harmful — the deadline expires, forces the queue pair to ERROR and
aborts every sibling context, then reports "peer hung" with queue-pair state. A loud but
*misdiagnosed* failure costs more than a silent one, because it sends you to debug the fabric;
Principle XI is about diagnosis, not just volume. It also pays for itself immediately and
independently of any version story: a stale binary on one node is the routine development
failure, and it currently costs a multi-second timeout and a destroyed connection.

**Why one version rather than per-kind.** See *Revision* below; the short form is that
per-kind bookkeeping buys nothing once a handshake exists, and actively permits an incoherent
state — `OP_FETCH` at v2 answered by `OP_FETCH_REPLY` at v1, when a reply's semantics are tied
to its request's version. One negotiated version makes that unrepresentable.

**Alternatives considered for how versions are compared.** Both of these were rejected for
reasons that still stand, and neither is what the revision adopted:

* **One protocol version, peers at different versions refuse to talk.** Rejected: it forces
  lockstep upgrade. Mid-rolling-upgrade, half the instances cannot serve the other half, and
  since the fallback for a cache miss is recompute, the cluster would quietly lose most of its
  hit rate for the duration — a silent degradation, which Principle XI exists to prevent.
* **One protocol version, with the higher side implementing every older version.** Rejected as
  the most code of the three: the compatibility surface is versions x kinds, and a kind that
  never changed still acquires a new version on every bump.
The surviving scheme — one version, negotiated, with the higher side able to speak down to its
declared minimum — is the second of those two with the lockstep problem removed by the
handshake and the code cost removed by the observation in the revision below.

**Why eager discovery rather than lazy fallback on error.** This reverses the "No negotiation"
posture in `contracts/wire-protocol.md`, deliberately. Lazy fallback sounds cheaper and is
not, because its hard part is hidden: on an error the requester must re-frame and **retry a
request whose landing slots are already reserved and whose rkeys are already on the wire**,
without double-publishing, plus cache the learned version, invalidate it on reconnect, and
handle a retry that also fails. Eager exchange deletes all of that — the first request is
already correctly framed, so nothing is ever retried for version reasons, and the deferred
work shrinks from a state machine to a `min()`.

The round trip is free in context: connection setup already costs address resolution, route
resolution and the rdma_cm handshake, each separately measured in the predecessor's telemetry.
One exchange against that is noise, and it happens once per connection, not per request.

**And it closes a hazard nothing else does.** `max_entries` is currently an *unchecked shared
assumption* — both sides assume an 8 KiB receive buffer and therefore 340 entries. Nothing
verifies it, so a future change on one side would let a requester overrun the peer's posted
receive. Making it part of the exchange is what gives eager discovery a consumer **today**,
which is the test Principle VI sets; the version-selection machinery on its own would not pass
it, and is deferred accordingly (see below).

**Frozen messages, and extensibility through data rather than layout.** The error reply and the
capability exchange are the two messages that can never be versioned — a peer that cannot
parse version N must still be able to parse them, or an unparseable error is answered with
another unparseable error. So both are fixed at version `0` (no real version is 0, so the
opcode can never collide) and their **layout** is frozen permanently. They stay extensible
because their *content* grows: reason codes are an open numeric space, and the capability list
is a counted array of `(kind, max_version)` pairs, so new kinds simply appear in it. Likewise a
version-independent **header prefix** — the opcode and the `context_id` — must mean the same
thing in every version forever, since that is the minimum needed to reject a message and still
correlate the rejection with its request.

**Floor, not a matrix.** Compatibility is bounded at one version back, with a configured floor
below which the connection fails loudly at establishment. The premise of a single
implementation across a homogeneous cluster means the only real skew is a rolling upgrade,
which is N and N-1. Failing at *connect* rather than on a request is also a much better
diagnostic, and it is only possible because discovery is eager.

**Deliberately deferred**: any compatibility shim for a v2 that does not exist. Build the
affordances that are expensive to retrofit — the frozen opcodes, the frozen prefix, the reason
space, the advertised version range — and decline the mechanism until something needs it. Same
posture as D19: the affordance now, the mechanism when it has a consumer. With a single
negotiated version the deferred work is a `min()` and a range check, not a state machine.

**On the stance this replaced**: "No negotiation" was never a numbered decision — it was a
bullet in the wire contract's *What is deliberately not here*, an assertion with no premise and
no rejected alternatives recorded. The D-numbered entries survive scrutiny because each states
what else was considered and why it lost, so when a premise moves it is visible what to
revisit; a bare stance has nothing to check against, rots silently, and then gets cited as a
constraint. Anything load-bearing belongs here, with its premise explicit.

### Revision (2026-10-15): per-kind versioning dropped

**What changed**: `OP_HELLO` advertised a counted list of `(kind, max_version)` pairs; it now
advertises one `[min, max]` protocol version range. Everything else stands.

**Why the original was wrong**: per-kind versioning won the first comparison *because discovery
was lazy*. An error reply can only report the version of the one opcode that failed, so per-kind
was the only scheme that needed no handshake — that was its entire advantage. When part 2 of
this decision switched discovery to eager, that advantage evaporated, and the per-kind
machinery was carried over without rechecking whether its justification had survived. It had
not. This is the same failure this project has already paid for twice and that D11 cites as the
reason for a loud-failure posture: a rationale verified under one premise and never re-examined
after the premise changed.

**The code-volume argument was also simply wrong.** It claimed a whole-protocol bump makes the
compatibility surface *versions x kinds*, with unchanged kinds acquiring a version on every
bump. They do not acquire any *code*: if v2 changes only `OP_FETCH`, then `OP_FETCH_REPLY` at
v1 and at v2 is the same function. Unchanged kinds cost nothing under either scheme, so there
was no explosion to avoid.

**And per-kind is worse than redundant.** It can represent `OP_FETCH` at v2 answered by
`OP_FETCH_REPLY` at v1. A reply's meaning is tied to the version of the request it answers, so
that combination is a correctness trap that one negotiated version cannot express.

**Two incidental improvements** fall out. `OP_HELLO`'s body becomes **fixed size**, which is
what a permanently frozen layout wants — the counted array was a variable-length body in a
message whose layout can never change, an uncomfortable pairing. And the floor check becomes
one range intersection instead of a per-kind comparison, so the failure diagnostic is a single
pair of numbers.

## D21 — The version is a validated header field, not part of the opcode

**Decision**: the opcode names a **kind only**. The protocol version occupies its own 2-byte
header field, inside the frozen prefix, and is **validated once** in the header parse against
the version negotiated for the connection — never dispatched on.

**Rationale**: fusing was the entire version-safety mechanism while there was no handshake, and
a good one then. Once the version is negotiated per connection (D20) it became both redundant
and actively cluttering. A bump mints a new opcode for *every* kind, including the ones that
did not change, so an unchanged `OP_FETCH_REPLY` accumulates one opcode value per version —
all dispatching to the same handler. By v4 that is four match arms for a message that never
changed. And the natural way to write that handler is `match opcode & 0xFFFF`, masking the
version straight back off: a mechanism whose common path discards it is not earning its place.
Version changes are also localized in practice — add a kind, or add a field to one kind — so
fusing makes a local change look global.

**It costs nothing to move.** The header already had a reserved `u16` at offset 6, inside the
frozen prefix, so the version field consumes no additional bytes and stays readable by a peer
at any version. The frozen messages' opcode values are numerically unchanged, since their
version half was already zero.

**What is preserved**: the guarantee that a message is never parsed at the wrong version. One
assertion in the single header parser gives that, and answers a mismatch with `OP_ERROR`
reason 2. The original objection to a separate field — "safety depends on every implementer
remembering to check it" — was fair when it implied a per-message manual check; it does not
hold for one assertion in one function in one implementation.

**Also considered**: dropping the version from the wire entirely and relying solely on
negotiated connection state. Rejected: it leaves no structural guard, so a negotiation bug or
a mismatched build would be *misparsed* rather than rejected — and with `addr`/`rkey` in the
body, a misparse is an RDMA write to the wrong place. The field is two bytes of existing
padding; the check is one comparison.

## D22 — One connection per instance pair, carrying both directions

**Decision**: keep exactly one queue pair per instance pair, used in both directions, as
FR-002 and D5 already require. Do not split into per-direction connections.

**Rationale**: FR-017 decides it. `disconnect` must guarantee no write from a peer can still
land before a slot is reclaimed, and with one queue pair one teardown achieves that. With two,
the guarantee requires tearing down both, and a partial teardown is a silent write into
reclaimed memory — the Principle IX hazard no test reliably catches.

**Correction to D5.** D5 rejected "two connections per pair," but the alternative it actually
weighed was splitting *message from data* (dest->source for the request, source->dest for the
payload), and its argument — "two QPs and two setups per pair, and the reply still needs a
direction" — addresses that split. It does **not** address splitting by *role*: each end owning
its own outbound connection for the requests it originates. That variant is genuinely
attractive, since it removes the simultaneous-connect race entirely (each end has exactly one
outbound, so there is no duplicate) and makes receive-buffer accounting trivially per-stream.
It loses on FR-017, as above. D5's rejection should be read as not covering this variant rather
than as having disposed of it.

**Accepted consequences**, each now a requirement (FR-026):

1. **Per-direction context-id namespaces.** The requester allocates ids, so both ends can
   independently allocate the same value on one connection. A reply with id 7 belongs to the
   local outbound table; a request with id 7 belongs to the inbound serving table. They are
   distinguishable only by message kind, so the tables must be separate. The obvious
   implementation — one map of contexts per connection — silently conflates two unrelated
   requests, which is why this is written down rather than left to inference.
2. **Per-direction message credits.**
3. **Reply starvation**, the one that is not obvious and that sharing a connection creates
   outright: posted receive buffers are consumed by *both* replies owed to this end and
   requests originated by the peer. At 8 credits and 64 KiB posted, a peer issuing 8 requests
   can take every buffer, leaving none for a reply this end is blocked on — an RNR stall and,
   with two-way traffic, a credible deadlock. Nothing in FR-014 or D10 covered it, because both
   were written as though a connection had one user.

   The fix is self-consistent rather than a new tunable: each end reserves receive buffers for
   the replies its own outstanding requests can generate — a count it already bounds via its own
   message credits — and advertises the remainder as the peer's inbound-request quota. That
   quota is why `OP_HELLO` carries `max_outstanding_requests`, and it had to be added **before**
   that layout froze.

## D23 — Simultaneous connect is resolved by a deterministic rule, not an arbitrator

**Decision**: on detecting a duplicate, both ends keep the connection whose **initiator is the
lower `PeerId`** and close the other. Resolution happens at accept, before `OP_HELLO` is posted.

**Rationale**: FR-001 lets either end initiate, so both can initiate at once; D22 explains why
two surviving connections break FR-017. The resolution reuses the property that makes version
selection safe — a deterministic function of two values both ends already know, evaluated
independently, with no proposal, no arbitration and no extra round trip. `PeerId` already
derives a total order for exactly this kind of use.

**Why at accept, and why identity in `private_data`**: evaluating the rule requires knowing
which end initiated each queue pair, so the connection request must carry it. The predecessor
already stamped its zyre `PeerId` into connect `private_data`; D16 replaces that with a certus
instance id, so the field exists and this makes it load-bearing rather than vestigial.
Resolving at accept — rather than after the capability exchange — means the losing queue pair
dies before any context exists on it, so there is nothing to migrate and no window in which a
context's landing slots are reachable over a doomed connection.

**Also considered**: letting only the lower `PeerId` ever initiate. Rejected — it deadlocks
whenever the higher-id instance is the one that needs data, since the lower has no reason to
connect.

## Rig facts (measured 2026-10-15, read-only, cluster left running)

Recorded because several decisions above were written against assumed numbers.

| fact | value |
|---|---|
| rdma-core | 2510.0.11-1.el9, with `rdma-core-devel`; `libibverbs` 1.14.58.0, `librdmacm` 1.3.58.0 |
| headers | `/usr/include/infiniband/verbs.h`, `/usr/include/rdma/rdma_cma.h` present |
| devices | **two**: `mlx5_0` -> `ens3f0np0`, `mlx5_1` -> `ens3f1np1` (dual-port ConnectX, fw 32.38.1002) |
| active port | `mlx5_0` only: `PORT_ACTIVE`, **50 Gbps**, MTU 4096, `link_layer: Ethernet` (RoCE) |
| down port | `mlx5_1`: `PORT_DOWN` / `phys_state: DISABLED` |
| RDMA data-path address | `10.0.0.102/24` on `ens3f0np0` (GUID `...45579c` matches `mlx5_0`) |
| NUMA | host has 2 nodes (0: 0-15,32-47 / 1: 16-31,48-63); **both RDMA devices are on NUMA node 0** |
| limits | `max_mr_size` unlimited, `max_mr` 16.8M, `max_pd` 8.4M, `max_qp` 131072, `max_qp_wr` 32768, `max_cqe` 4194303, `max_sge` 30 |

Consequences for the decisions above:

* **T008 is unblocked.** `pkg-config` resolves both libraries and the devel headers are
  installed, so the `cc` shim has everything it needs.
* **D17's per-device PD keying is load-bearing, not theoretical.** Two `ibv_context`s exist,
  so a PD allocated against one cannot serve a connection that resolves to the other — even
  though only one port is currently up.
* **D17's "resource limits are not a drawback" is confirmed with numbers.** `max_mr_size` is
  unbounded, so a multi-GB tier registers as a single MR, and at 8.4M PDs the choice between
  one shared PD and one per connection is invisible to the device.
* **`link_layer: Ethernet` with `transport: InfiniBand (0)` is RoCE, not a contradiction** —
  the transport field names the verbs transport, not the fabric. Worth stating so nobody
  "corrects" the RoCE assumption after reading `ibv_devinfo`.
* **A caveat for D15 and for the GPU-NUMA-instance pinning assumption**: both RDMA ports sit
  on NUMA node 0, so the NUMA-1 instance has no local NIC and its traffic crosses the
  inter-socket link regardless. D15 allocates the bounce pool on the *serving instance's*
  NUMA node, which is right for the device read but leaves NIC DMA remote for that instance.
  This is a measurement caveat, not a design change — but a 50G link and a cross-socket hop
  should both be stated when any transfer number from this rig is quoted.
* **Phase 9 scope on this node**: single-host loopback (T054, T055) runs on `mlx5_0`. The
  two-node test (T056) needs the peer node's equivalent. A topology with one NIC per NUMA
  domain is *not* currently available, since the second port is down and both ports are on
  node 0 anyway.

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
