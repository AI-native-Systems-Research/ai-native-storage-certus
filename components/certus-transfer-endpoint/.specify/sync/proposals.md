# Drift Resolution Proposals

Generated: 2026-10-22T01:03:54Z
Based on: drift-report from 2026-10-22T01:03:54Z (`.specify/sync/drift-report.json`)

Nothing here is approved. Every proposal is `approved: false`. Spec paths are relative to
`components/certus-transfer-endpoint/specs/001-rdma-cache-transfer/`.

**Re-stamp warning**: P05, P13, P15 and P19 touch doc comments in
`components/interfaces/src/itransfer_endpoint.rs`. The spec-sync digest folds interfaces into
every component, so landing any of them means re-stamping every component — do it last.

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 18 |
| Align (Spec → Code) | 2 (P07 code defect; P24 comments only) |
| Human Decision | 6 |
| New Specs | 0 (unspecced items folded into P26) |
| Remove from Spec | 0 |

## Proposals

### P01: FR-006 — request content
**Direction**: BACKFILL · **Severity**: moderate · **Confidence**: HIGH

- Spec says: "A transfer request MUST carry: a context ID, the destination's rkey, base address, per-entry size, and a list of `(block key, landing slot index)` pairs."
- Code does: per-entry `(key, addr, size, rkey)` (`src/message.rs:253-309`), as wire-protocol.md requires.

**Proposed spec text** (spec.md FR-006):
> old: **FR-006**: A transfer request MUST carry: a context ID, the destination's rkey, base address, per-entry size, and a list of `(block key, landing slot index)` pairs.
>
> new: **FR-006**: A transfer request MUST carry a context ID and, per entry, the block key and that entry's landing slot as an absolute destination address, size and rkey — the content of `(CacheKey, RemoteRegion)`. The source MUST NOT need a base address, a slot index or a uniform entry size: the memory tier is not index-addressable, and per-entry rkeys let one request land across several registered regions.

Key Entities, same file:
> old: **TransferContext** — one in-flight request: context ID, peer, key list, landing slots, outstanding-entry count, deadline.
> new: **TransferContext** — one in-flight request message (one chunk of a fetch): context ID, peer, its entries and their landing slots, its offset in the caller's fetch, and its send time, from which the reply deadline runs.

> old: **RemoteRegion** — a landing slot: rkey, address, size. Already exists.
> new: **RemoteRegion** — a landing slot: `addr`, `rkey`, `length`. Already exists.

**Rationale**: wire-protocol.md already explains why indices are unimplementable; the spec is the stale document.

---

### P02: FR-014 — send credits count every send-queue request
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

> old: **send-queue credits** — in-flight RDMA writes per connection; this transitively bounds held read pins, which is the existing rationale for the bound;
>
> new: **send-queue credits** — in-flight send-queue work requests per connection: RDMA writes *and* message sends, which share the queue (counting only writes would let messages overfill it). This transitively bounds held read pins, which is the existing rationale for the bound. Send credits are clamped to the depth the device grants, and the clamp is logged;

Also `TransferStats::send_queue_in_flight` doc (interfaces): "In-flight RDMA writes" → "In-flight send-queue requests (RDMA writes and message sends)".

**Rationale**: deliberate (CLAUDE.md already documents it); stricter than spec; SC-004 still holds.

---

### P03: FR-016 / INV-10 — where the deadline clock starts
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: MEDIUM

> old (FR-016): The deadline MUST run from when the request is sent, so a request waiting for a credit cannot expire.
>
> new: The deadline MUST run from when the request takes its message credit and is queued to the connection's send queue — not from the `fetch` call — so a request waiting for a message credit or the capability exchange cannot expire.

Contract INV-10 first sentence, same change.

**Rationale**: the residual window (outbox → post, waiting only on a send-queue credit) is one poll. If the reviewer wants literal "from the post", that is a small ALIGN (stamp in `flush_outbox` on success), not worth a behaviour change on its own.

---

### P04: FR-020 — remote-lookup test-seam additions
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

> old: **FR-020**: `remote-lookup` MUST NOT be modified as part of delivering P1.
>
> new: **FR-020**: `remote-lookup`'s logic MUST NOT be modified as part of delivering P1. The only permitted change is to its test doubles, where an interface it implements grows (the bounce-reserve methods of `IMemoryTier` and `read_to_bounce`/`release_bounce` of `IDispatcher`); those doubles refuse every lease.

tasks.md:345 "and `remote-lookup` has not been modified" → "and `remote-lookup`'s logic has not been modified (its test doubles gained the T083 bounce methods)".

---

### P05: FR-024 / INV-13 — which request faults tear down instead of being answered
**Direction**: HUMAN_DECISION · **Severity**: moderate · **Confidence**: MEDIUM

Code tears down (no `OP_ERROR`) for: an `OP_FETCH` over the advertised quota or before `OP_HELLO` (`endpoint.rs:1410-1413`); an `OP_HELLO` that is malformed or duplicated (`endpoint.rs:1311-1316`); an `OP_ERROR` refusing anything other than an `OP_FETCH` (`endpoint.rs:1340-1343`).

- **Option A (BACKFILL, recommended)**: these are session-rule violations, not unparseable messages. A peer over quota has already consumed a receive it was not granted — the RNR-safety partition is broken — and a bad `OP_HELLO` means there is no session to answer within. Proposed FR-024 addition:
  > A request that violates the connection's session rules rather than its format — an `OP_FETCH` beyond the granted inbound quota or before the peer's `OP_HELLO`, or an `OP_HELLO` that is malformed or repeated — MUST instead tear the connection down through the FR-023 ordering, reported loudly: the peer has broken the receive partition or the capability exchange, so no reply this end could send is guaranteed a buffer, and no further data on the connection can be trusted. An `OP_ERROR` refusing this end's `OP_HELLO` is likewise a teardown: it means the peer cannot speak the frozen exchange at all.
- **Option B (ALIGN)**: answer an over-quota `OP_FETCH` with `OP_ERROR` reason 3/new reason 5 and keep the connection. Cost: the answer itself needs a reserve buffer at the peer that the quota breach may already have taken.

Question: is a quota breach a "request it cannot process" (answer) or a peer that broke the partition (teardown)?

---

### P06: FR-025 — local cap on max_entries; zero max_entries
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

> old: A requester MUST respect the peer's maximum request size rather than its own.
>
> new: A requester MUST respect the peer's maximum request size rather than assume it equals its own; it chunks to the smaller of the peer's advertised `max_entries` and the largest request it can itself encode in one send buffer. An advertised `max_entries` of zero MUST fail the connection, as a zero quota does.

---

### P07: FR-026 — per-direction gauges never filled
**Direction**: ALIGN · **Severity**: moderate · **Confidence**: HIGH · **Code defect**

`TransferStats.requests_outstanding_outbound` / `requests_in_service_inbound` always read 0 (`telemetry.rs:49-52`; nothing writes them). The per-direction values exist in each `Session` (`session.rs:172-180`).

**Proposed code change**: compute both at snapshot time, like the send-queue gauges — in the actor's `Command::Stats` handler (`endpoint.rs:1682-1689`), sum `conn.session.outbound().0` (minus a still-owed hello, or include it and say so) and `conn.session.inbound().0` over `self.conns` and overwrite the two fields of the snapshot; drop the two fields from `Counters`. Add a test asserting both are non-zero while fetches are held (e.g. with the source's sends held) and zero after. Update `telemetry.rs:15-20` (no field is unfilled afterwards).

**Rationale**: Principle XI — a gauge reading 0 on a path that is exercised is exactly the "plausible number" the component forbids; T079 is marked done.

---

### P08: SC-002 — legacy test layer against this endpoint
**Direction**: BACKFILL · **Severity**: moderate · **Confidence**: MEDIUM

> old: **SC-002**: The existing `remote-lookup` test layer and the initiator's `loopback_push_writes_into_remote_buffer` hardware loopback test pass against this endpoint with messaging unused.
>
> new: **SC-002**: The legacy push surface reproduces the initiator's `loopback_push_writes_into_remote_buffer` scenario on hardware through this endpoint, and its order, reuse and teardown properties hold over the mock seam, with messaging unused. Running the `remote-lookup` test layer itself against this endpoint requires re-pointing its receptacle and is part of the cutover (T065), not of P1.

Same for US2's "Independently testable" paragraph.

**Rationale**: running remote-lookup against it is out of scope by FR-020/Phase 11; the spec should not claim it.

---

### P09: SC-008 — FFI defined once
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

> new: **SC-008**: This crate defines the verbs FFI once, as the single copy the predecessors will share or be replaced by; the duplication itself ends when `remote-lookup-rdma-initiator` and `-responder` are retired (T066). (Today their two `ffi.rs` files declare 52 public names in common.)

---

### P10: CT-1 — `IncompatiblePeer` is never returned
**Direction**: HUMAN_DECISION · **Severity**: moderate · **Confidence**: MEDIUM · **possible code defect**

- **Option A (ALIGN)**: when `on_hello` returns `HelloError::Incompatible`, carry the ranges into the teardown cause so `fail_fetches` aborts the waiting fetches with `IncompatiblePeer("peer P speaks [a,b], we speak [c,d]")` instead of `Transport("connection lost")`; pushes report `Failed` as now. Test: extend `an_incompatible_version_fails_the_connection_at_bring_up` (`lib.rs:1802`) with a queued fetch and assert the variant.
- **Option B (BACKFILL)**: remove `IncompatiblePeer` from the `fetch` errors doc and contract, and state that an incompatible peer fails its waiting fetches as `Transport`, with the ranges in the log.

Recommendation: A — the variant exists precisely so the caller can tell a stale binary from a lost link (FR-025's "diagnostic naming peer and both ranges").

---

### P11: CT-2 — `fetch` and `NotInitialized` without a landing region
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

Interface doc (`itransfer_endpoint.rs:589-591`): drop "[`NotInitialized`] if no landing region is registered" → "[`NotInitialized`] if the endpoint is not running". `NotInitialized` variant doc (`:247`): "A required receptacle is unbound, or no landing region is registered" → "The endpoint is not configured and running, or a required receptacle is unbound."

**Rationale**: landing slots may be in any registered region (or, today, only test code's pool key); the endpoint does not, and cannot cheaply, check.

---

### P12: CT-3 — landing into the destination's own memory tier
**Direction**: HUMAN_DECISION · **Severity**: major · **Confidence**: MEDIUM

US1 and the contract say slots are in the destination's memory tier. The pool is registered `REMOTE_WRITE` at `initialize` (`lib.rs:510-521`) but its rkey is not exposed, and `register_landing_region` refuses pool overlap (`endpoint.rs:1519-1523`). A real caller cannot land into the memory tier.

- **Option A (ALIGN)**: add `ITransferEndpoint::pool_landing_region() -> Result<LandingRegionHandle, _>` returning the already-registered pool as a handle (interface change → re-stamp all).
- **Option B (BACKFILL)**: state that landing regions are caller-allocated buffers outside the memory-tier pool, and that the connector copies or adopts them; change US1, the Assumptions, and the contract's "in practice the memory tier's own pool".
- Decide before the shm-queue/connector layer (T067) is specified.

---

### P13: CT-4 — contract's `IMemoryTier` bounce signatures
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

contracts/itransfer_endpoint.md:378-379:
> old: `fn bounce_alloc(&self, len: u32) -> Result<*mut u8, BounceError>;` / `fn bounce_free(&self, ptr: *mut u8, len: u32);`
> new: `fn bounce_alloc(&self, len: u32) -> Result<BounceLease, BounceError>;` / `fn bounce_free(&self, lease: BounceLease);`

Also add "`len` is rounded up to 4 KiB in the reserve; `bounce_usage` reports both values after rounding" (`imemory_tier.rs:64-65, 236-238`).

---

### P14: CT-5 — `stats()` before initialize
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

contract :237: "Until `initialize` succeeds, every `ITransferEndpoint` method returns `NotInitialized`" → "…every `ITransferEndpoint` method that can fail returns `NotInitialized`; `stats()`, which cannot fail, returns all-zero counters before `initialize` and after `shutdown`."

---

### P15: CT-6 — what `protocol_faults` counts
**Direction**: HUMAN_DECISION · **Severity**: moderate · **Confidence**: MEDIUM · **possible code defect**

Today: received `OP_ERROR` only. Doc says "received or sent"; wire-protocol says a bad reply "counts it as a protocol fault"; the *Open* paragraph relies on that counter to expose a bad-reply teardown loop — which currently counts nothing.

- **Option A (ALIGN, recommended)**: increment `protocol_faults` on every protocol-caused teardown (bad reply, quota breach, bad hello) in addition to received `OP_ERROR`; keep sent `OP_ERROR` out (the test at `lib.rs:2019-2025` asserts the answerer did not fault, which is a reasonable reading). Doc → "Protocol faults this end detected: `OP_ERROR`s received for its requests, and connections torn down for a protocol violation by the peer. An `OP_ERROR` this end sends is not counted."
- **Option B**: add a separate `protocol_teardowns` field (interface change).
- **Option C (BACKFILL only)**: doc → "received", and delete the wire-protocol claims. Leaves the loop invisible.

---

### P16: WP-1 — frozen kinds' version field
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

wire-protocol.md:40 "They carry `version = 0`, which no real version uses, and a receiver ignores the version field for them." → "They carry `version = 0`, which no real version uses; a receiver accepts them whatever version was negotiated and refuses any non-zero version on them as malformed." Header table :70: "Zero and ignored for `OP_HELLO`/`OP_ERROR`" → "Zero for `OP_HELLO`, `OP_HELLO_REPLY` and `OP_ERROR`, whatever was negotiated".

---

### P17: WP-2 — reply size
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

wire-protocol.md:251 "bounds `OP_FETCH_REPLY` at `24 + 340` = 364 bytes" → "bounds `OP_FETCH_REPLY` at `24 + 344` = 368 bytes (340 outcome bytes padded to 8)".

---

### P18: WP-3 — receives posted after establishment
**Direction**: HUMAN_DECISION · **Severity**: moderate · **Confidence**: MEDIUM

Code posts reserve + quota on `Up` (`endpoint.rs:1132-1175`); the SC-016 test admits the first `OP_HELLO` can RNR-stall (`lib.rs:2731-2735`). Safe only because RNR retry is infinite (`verbs.rs:53-57`).

- **Option A (ALIGN)**: post the receives when the link is created (accept, or connect before `rdma_connect`) — the verbs accept path already allows it (`verbs.rs:356`) — so no `OP_HELLO` can find an empty queue. Needs a table hook between link creation and `Up`.
- **Option B (BACKFILL)**: wire-protocol.md:142-145 and T080 → "posted as the link comes up; the first `OP_HELLO` may meet a not-yet-posted receive and is absorbed by RNR retry, which is therefore required to be infinite (`rnr_retry_count = 7`)".

---

### P19: WP-4 — `"ip:port"` vs `"host:port"`
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

contract :28 and wire-protocol.md:322 "`\"ip:port\"`" → "`\"host:port\"` (an IPv6 literal bracketed), compared as written: there is no resolver, so every instance must be named by the same string everywhere".

---

### P20: SPEC-1 — "for that kind"
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

spec.md:74 and :287: "the sender's highest supported version for that kind" → "the sender's highest supported protocol version (0 if the kind itself is unknown)".

---

### P21: TK-1 — T031 description
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

tasks.md T031: "Implement the context table in `src/message.rs`: `TransferContext` = …" → "Implement the context tables in `src/endpoint.rs`: the per-connection outbound book `context_id → (fetch, chunk)` and the per-fetch chunk list (offset, entries, send time). Allocate ids per connection, match replies by echoed id. …" (keep the rest).

---

### P22: DOC-1 — CLAUDE.md "Current state"
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

Replace the trailing sentences "`disconnect` and `stats` work; the rest say which task they land in. The verbs side of the seam is T053." with "Every `ITransferEndpoint` and `ITransferEndpointAdmin` method is implemented; the verbs side of the seam is `connection/verbs.rs` (T053)." Ideally the whole paragraph is rewritten as a current-state list, not an accretion of task numbers.

---

### P23: DOC-2 — plan.md "Next Step"
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: HIGH

Replace the section with: "Phases 1-10 of `tasks.md` (T001-T083) are complete; Phase 11 (cutover and retirement, T065-T067) is out of scope." Remove "Begin at T001-T002" and the T002 prerequisite narrative (or move it to research as history, if wanted).

---

### P24: SRC-1 — stale source comments
**Direction**: ALIGN (comments only) · **Severity**: minor · **Confidence**: HIGH

`lib.rs:643` drop "(until T037-T040)", say block-tier entries are served with `spdk` and missed without; `telemetry.rs:15-20` and each "Filled by T0xx" line — rewrite once P07 lands; `message.rs:23-25`, `session.rs:31` remove the "later slices" text and the `#![allow(dead_code)]` if clippy stays clean.

---

### P25: SRC-2 — outside-pool pointer reported Miss
**Direction**: HUMAN_DECISION · **Severity**: minor · **Confidence**: LOW · **possible code defect**

A memory-tier answer outside the registered pool is a wiring/defect condition (map and tier disagree), not residency. Option A (ALIGN): report `Failed` (`serve.rs:185-187`), keep the warn. Option B (BACKFILL): add "or a location outside the registered pool" to FR-007's `Miss` list and `EntryOutcome::Miss` doc. Principle XI favours A.

---

### P26: Unspecced behaviour — backfill into existing FRs
**Direction**: BACKFILL · **Severity**: minor · **Confidence**: MEDIUM

No new spec; add short clauses:
- **FR-019 note**: "`push` blocks its caller and MUST be refused, not deadlock, when called on an endpoint thread (e.g. from a completion callback)." (`lib.rs:676-682`)
- **New FR-028 (Principle XI)**: "A panic on the endpoint thread or a block-reader thread MUST abort the process: continuing with a possibly corrupt pin, slab or credit table risks a silent double release." (`endpoint.rs:156-174`)
- **FR-027 extension**: "A duplicate the race cannot explain — from an end that already owns or out-ranks the live link — is resolved *newer wins*: the old link goes to ERROR first. An inbound request while the peer's previous link is still draining is refused. An acceptor MUST answer to the name it was dialed by, else the link fails. Known gap: a crashed-and-restarted peer that out-ranks nobody is refused until a deadline notices the stale link." (`table.rs:13-38`, `741-776`, `806-816`; research D24)
- **wire-protocol.md, new section "Connect private data"**: the frozen format `'C' 'T' 0x01 len <PeerId>` for connect/accept and `'C' 'T' 0x01 reason` for reject, extended only by a new format byte (`identity.rs:1-110`).
- **Contract `register_landing_region`**: "refuses a null, empty or address-wrapping range, one overlapping the memory-tier pool or another landing region; there is no deregistration" (subject to P12).
- **Admin contract `shutdown`**: "a connection whose drain makes no progress for 5 s is reported and its queue pairs and every registration deliberately leaked, never destroyed with state held" (`endpoint.rs:75, 1640-1651`).
- **FR-014 bounce domain**: "a `NoSpace` is believed only if no lease was returned after its read was submitted" (`bounce.rs:66-72`).
- **wire-protocol `OP_FETCH_REPLY`**: "non-zero padding or a non-zero reserved field is malformed" (`message.rs:220-229, 366-369`).
- **stats contract**: mention the `telemetry`-gated connect counters, which are not in `TransferStats`.
