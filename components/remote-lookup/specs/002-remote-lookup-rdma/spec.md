# Feature Specification: Remote Lookup over Zyre + RDMA

**Feature Branch**: `002-remote-lookup-rdma`
**Created**: 2026-07-12
**Status**: Synced (2026-07-22; re-swept 2026-08-07 on branch `sync/spec-drift-sweep-20260807`)
— implementation matches this spec (see `.specify/sync/drift-report.md`); two prior
low-severity drift items (FR-023, FR-009) were backfilled into the text below on 2026-07-22.
The 2026-08-07 sweep backfilled five previously-unspecced, load-bearing behaviors as FR-030…FR-034
(caller-wait / background-continue, orphan force-reclaim backstop, orphan-reuse guard, extra
`LookupConfig` fields, and the `integrity-check` feature). Two code-side items were left as ALIGN
tasks (not applied): the stale `src/lib.rs` module/`batch_lookup` docstrings that still claim the
protocol is unbuilt (FR-001 doc-drift, Medium), and the missing log on unknown wire frames
(FR-018, Low). See `.specify/sync/align-tasks.md` and `apply-report.md`.
The 2026-08-20 sweep backfilled three previously-unspecced, load-bearing behaviors into the text
below (FR-006 `AlreadyExists` size-collision guard, FR-014 fixed `DISCONNECT_ACK_TIMEOUT`
handshake bound, and FR-018 malformed/truncated-frame handling) and re-queued the FR-018 logging
ALIGN task (now covering both the unknown-type and malformed-decode arms).
**Supersedes**: `001-remote-lookup-placeholder` (placeholder implementation)
**Input**: Refresh of the remote-lookup design against the two now-built RDMA components
(`remote-lookup-rdma-initiator`, `remote-lookup-rdma-responder`). The remote-lookup
component is invoked by the local dispatcher with a batch of `(key, size)` entries that
missed the local memory and disk tiers. Its job is to **copy those entries from peer
Certus instances' caches into this node's local DRAM memory tier** — nothing more. GPU
delivery (memory-tier → device) remains the dispatcher's / gpu-services' responsibility
and is out of scope here. Acting as a **client**, remote-lookup discovers which peers hold
the entries (a zyre SHOUT), then drives one-sided RDMA writes of the values into
locally-allocated DRAM landing slots, using a greedy, two-phase (memory-first,
disk-fallback) strategy with bounded retries. Acting as a **server**, it answers peers'
queries and pushes its own local values back over RDMA. Both roles run on one actor thread.

### Boundary with the RDMA initiator/responder components

This component owns the **RDMA accept side** and the **zyre control plane**, which the
data-path crates explicitly defer to it (`remote-lookup-rdma-initiator` spec 002 "Known
Limitations / Boundary"; `remote-lookup-rdma-responder` spec 001). Concretely: the
**responder** runs the `rdma_cm` listener and registers this node's memory-tier pool with
remote-write access (FR-025); the **initiator** performs only the outbound one-sided writes
(FR-024, via `IRemoteLookupRdmaInitiator`); and remote-lookup carries the keys and
`RemoteRegion` descriptors between peers over zyre (KEY_QUERY/KEY_RESPONSE/RDMA_REQUEST/
RDMA_STATUS). The initiator's `IMemoryTier::peek`→write eviction-race caveat is closed here
by holding a dispatch-map read reference across the serve's `push` (see `server.rs`).

## Clarifications

### Session 2026-07-12 (design refresh — resolved)

- Q: What is remote-lookup's output — does it deliver to GPU memory? → A: No. remote-lookup
  works exclusively with **CPU/DRAM** memory. It fills the local **memory tier** (DRAM cache)
  with values fetched from peers. The `IpcHandle` (GPU destination) used by
  `IDispatcher::batch_lookup` does **not** belong at the `IRemoteLookup` boundary; the
  dispatcher performs the DRAM→GPU copy afterward for any key remote-lookup resolved. The
  `IRemoteLookup::batch_lookup` signature therefore becomes
  `&[(CacheKey, u32 /* size */)] -> Vec<Result<(), RemoteLookupError>>`, where `Ok(())` means
  "the key is now resident in the local memory tier."
- Q: How do the two RDMA components map onto the two node roles? → A: The **requesting**
  node runs the **responder** (`IRemoteLookupRdmaResponder`) — it pre-registers its whole
  DRAM memory tier, binds an ephemeral RDMA endpoint, advertises `{ip, port}` in its
  whispers, and accepts inbound one-sided writes. The **serving** node runs the **initiator**
  (`IRemoteLookupRdmaInitiator::push`) to write its local values into the requester's landing
  slots. Data movement is one-sided; the RDMA_STATUS whisper is the completion signal.
- Q: Where does the `rkey` in each RDMA_REQUEST come from, and how is RDMA code kept
  isolated? → A: Memory registration is **whole-pool and permanent**: the DRAM memory-tier
  pool is allocated once at `initialize` and never grows, so `insert`/`evict` are pure
  sub-allocation within a fixed, already-registered region — there is **no per-entry
  registration in the I/O path**. Because an inbound one-sided write is validated against
  MRs in the **responder's** protection domain, the **responder owns the inbound MR**: it
  gains a `memory_tier` receptacle, calls `pool_info()` at `initialize`, registers the whole
  pool once with `REMOTE_WRITE`, and exposes the `rkey` (e.g. `local_region() -> { base,
  len, rkey }`). The **memory tier stays RDMA-agnostic** (no ibverbs, no MR). remote-lookup
  caches that single pool-wide rkey at startup and pairs it with each landing slot's address.
- Q: How is the `(key, size)` identity handled to avoid size-mismatch races? → A: The wire
  identity of an entry is the tuple `(key, size)`. A peer that holds `key` at a different
  size reports it as **not available** rather than as a size mismatch. This removes the
  size-mismatch reply class from the protocol; the initiator's `PushStatus::SizeMismatch`
  remains only as a defensive internal guard.
- Q: What is the resolution strategy across peers and tiers? → A: Greedy and asynchronous.
  As each KEY_RESPONSE arrives, immediately request (whisper) the subset of that peer's
  **memory-tier** hits that are still unsatisfied and not already in-progress, then record
  the peer's full reply for later. This is **Phase 1**. After replies from a configurable
  percentage of the group's peers (or a timeout), **Phase 2** re-scans the *cached* replies
  (no second SHOUT) for still-unsatisfied keys, now allowing **disk-tier** hits as a fallback
  (the serving node promotes the entry disk→memory before pushing). A bounded number of retry
  rounds re-targets alternate peers for keys that failed.
- Q: How are local landing slots managed so that same-key lookups block during a fetch but
  other keys are unaffected? → A: **Publish-on-success.** A landing slot is reserved *privately*
  in the requester's own responder-registered pool (`memory_tier.insert` → a pool `addr`) and is
  **not published to dispatch-map until the RDMA transfer succeeds**. On success the fetcher
  `create_memory_tier_entry(key, addr, len)` (write_ref=1, the DRAM is already filled) +
  `release_write` — the entry appears in dispatch-map exactly once, fully valid. On
  failure/peer-exit it `memory_tier.remove`s the private slot and never touches dispatch-map, so
  there is no removal-vs-blocked-reader race. **Single-flight** is enforced in the actor, not by a
  dispatch-map placeholder: a per-key in-flight index coalesces concurrent same-key misses into
  followers of one in-flight fetch (they block on it, never issue a duplicate RDMA), and other
  keys are unaffected. (This drops the earlier dispatch-map dependency D1 — no dispatch-map change.)
- Q: How is memory safety preserved when a peer departs mid-transfer? → A: **Completing** the
  waiting operations (with a not-found result for the abandoned key) is separate from the
  **physical** slot reclaim (returning the DRAM slot to the allocator). Before a landing slot
  exposed to a departing peer is returned to the allocator, remote-lookup issues
  `ResponderCommand::Disconnect { node }` and blocks for `ResponderEvent::DisconnectAck`. The
  ack is an unconditional guarantee that the peer's QP is in ERROR state and no late one-sided
  write can still land (teardown-before-reclaim). The pool stays permanently registered, so
  holding the slot reserved across that brief window is harmless.
- Q: Are a node's own SHOUTs delivered back to it? → A: No — zyre does not deliver a node's
  own SHOUT to itself. A self-filter (peer-id vs. own uuid) is retained only as defensive
  belt-and-suspenders and is not load-bearing.

### Dependencies on other components (implied by the above)

- **`IRemoteLookup` interface change** — (a) drop `IpcHandle`; `batch_lookup` takes
  `&[(CacheKey, u32)]` (already on this branch). (b) **002 adds `initialize(LookupConfig)`** and a
  new public `interfaces` type `LookupConfig` (derives `Default`), mirroring
  `IDispatcher::initialize(DispatcherConfig)`. The only existing `IRemoteLookup` impl is
  remote-lookup's own (002 rewrites it), so the blast radius is contained; this is the sole
  `interfaces`-crate change for 002 and lands as its own commit.
- **Responder becomes the tier registrar** — new work on the otherwise-complete responder:
  add a `memory_tier` receptacle, register the pool in its PD at `initialize`, expose
  `local_region()`/rkey, and deregister on shutdown after QPs are down.
- **dispatch-map: no change required** — publish-on-success means dispatch-map only ever holds a
  *fully-filled* entry (created on RDMA success), and the failure/peer-exit path never creates
  one, so the earlier blocked-reader-wakeup dependency (D1) is dropped. remote-lookup uses only
  the existing `IDispatchMap` surface (`create_memory_tier_entry`, `release_write`, `lookup`).
- **dispatcher: no change required (US4)** — disk→memory promotion uses the existing
  `IDispatcher::promote_to_memory_tier` (D3 resolved). remote-lookup adds a `dispatcher: IDispatcher`
  receptacle; the integrating mainline must `disconnect()` one side of the resulting
  `dispatcher-p2p ⇄ remote-lookup` `Arc` cycle at teardown (see research Decision 7).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Remote Memory-Tier Hit (Priority: P1)

The local dispatcher has a batch of `(key, size)` entries that missed both the local memory
tier and disk tier. It calls `remote_lookup.batch_lookup`. The actor SHOUTs a KEY_QUERY to
the group. Peers reply indicating which entries they hold in memory, on disk, or not at all.
For a peer's memory-tier hits, the actor reserves a private local DRAM slot, whispers an
RDMA_REQUEST carrying that slot's region, and the peer's initiator writes the value directly
into it. On the RDMA_STATUS(Success) whisper the actor publishes the slot to dispatch-map — the
key is now resident in the local memory tier and `Ok(())` is returned for it.

**Why this priority**: This is the core value proposition — populating the local cache from a
peer's in-memory copy over RDMA, with no disk I/O anywhere.

**Independent Test**: Two Certus nodes on one LAN. Populate `(K, S)` in node B's memory tier;
call `batch_lookup([(K, S)])` on node A. Verify A's memory tier subsequently contains K's
value (a following local lookup hits) and the positional result is `Ok(())`.

**Acceptance Scenarios**:

1. **Given** node B holds `(K, S)` in its memory tier, **When** node A calls
   `batch_lookup([(K, S)])`, **Then** K's value is RDMA-written into A's private landing slot,
   the slot is published to A's memory tier on RDMA_STATUS(Success), and the positional result
   is `Ok(())`.
2. **Given** two peers B and C both report `(K, S)` in memory, **When** A resolves K, **Then**
   A requests K from exactly one of them (the first whose response it processed) and does not
   double-request K from the other.
3. **Given** no peer reports `(K, S)`, **When** completion criteria are met, **Then** the
   positional result for K is `Err(NotFound)` and no landing slot is left in A's memory tier.

---

### User Story 2 - Server Role: Answering Queries (Priority: P1)

When a peer SHOUTs a KEY_QUERY, the local actor receives it via its zyre poll loop, checks
each requested `(key, size)` against its dispatch-map, and whispers back a KEY_RESPONSE
reporting, per entry, whether it holds it in memory, on disk, or not at all (a size mismatch
is reported as "not held").

**Why this priority**: The system is symmetric; without the server role no node can answer.

**Acceptance Scenarios**:

1. **Given** this node holds `(K, S)` in memory, **When** a KEY_QUERY for `(K, S)` arrives,
   **Then** a KEY_RESPONSE reports K as memory-available.
2. **Given** this node holds `(K, S)` only on disk, **When** a KEY_QUERY for `(K, S)` arrives,
   **Then** a KEY_RESPONSE reports K as disk-available.
3. **Given** this node holds K at size `S'` ≠ `S`, **When** a KEY_QUERY for `(K, S)` arrives,
   **Then** a KEY_RESPONSE reports K as not available (no size-mismatch status).

---

### User Story 3 - Server Role: Serving an RDMA Request (Priority: P1)

When a peer whispers an RDMA_REQUEST (its responder endpoint plus, per key, the landing
`addr`/`rkey`/`length`), the local actor pins each requested value, delegates the write to
`IRemoteLookupRdmaInitiator::push`, unpins, and whispers back the per-key RDMA_STATUS derived
from the returned `PushStatus`.

**Why this priority**: This is the only path that actually moves data.

**Acceptance Scenarios**:

1. **Given** this node holds `(K, S)` in memory, **When** an RDMA_REQUEST for K with a valid
   region arrives, **Then** `push` writes the value and RDMA_STATUS(Success) is whispered.
2. **Given** `(K, S)` was evicted between KEY_RESPONSE and RDMA_REQUEST, **When** the request
   arrives, **Then** `push` returns `KeyNotFound` and RDMA_STATUS(KeyNoLongerAvailable) is
   whispered.
3. **Given** the requester's endpoint is unreachable, **When** `push` returns
   `UnableToConnect`, **Then** RDMA_STATUS(UnableToConnect) is whispered for the affected keys.

---

### User Story 4 - Disk Fallback (Phase 2) (Priority: P2)

After Phase 1 (memory-only) has gathered replies from the configured quorum of peers (or timed
out), the actor makes a second pass over still-unsatisfied keys using the **cached** replies
only (no new SHOUT). For a key a peer reported on disk, the actor whispers an RDMA_REQUEST; the
serving node promotes the entry disk→memory (into its RDMA-registered pool), then pushes.

**Why this priority**: Materially improves hit rate for cold data at the cost of a remote disk
read; functional as a memory-only cache filler without it.

**Acceptance Scenarios**:

1. **Given** only node B reported `(K, S)` and only on disk, **When** Phase 2 runs, **Then** B
   promotes K to memory, pushes it, and RDMA_STATUS(Success) is whispered.
2. **Given** Phase 1 already satisfied all keys, **When** the quorum/timeout fires, **Then**
   Phase 2 is skipped entirely.

---

### User Story 5 - Retry to an Alternate Peer (Priority: P2)

When an RDMA_STATUS reports a non-success (UnableToConnect or KeyNoLongerAvailable), the actor
returns the key to the unsatisfied set and, in a bounded number of retry rounds, re-targets it
to another peer that (per cached replies) reported holding it — preferring a memory hit, then a
disk hit. If no cached peer has it, the key is finalized as not found.

**Acceptance Scenarios**:

1. **Given** B and C both reported `(K, S)`, and B's push fails, **When** RDMA_STATUS(fail) from
   B is received and retries remain, **Then** an RDMA_REQUEST for K is sent to C.
2. **Given** only B reported K and B's push fails, **When** no other cached peer has K, **Then**
   K is finalized as `Err(NotFound)` and its private landing slot is discarded.
3. **Given** the configured retry-round cap is reached, **When** keys remain unsatisfied,
   **Then** the operation stops re-targeting and finalizes those keys as `Err(NotFound)`.

---

### User Story 6 - Completion Criteria and Timeout (Priority: P2)

An operation completes at the first of: (1) all keys satisfied; (2) no cached peer holds any
remaining unsatisfied key (and no more replies are expected); (3) the retry-round cap is
reached; or (4) the overall operation deadline expires. The Phase-1→Phase-2 transition fires at
the first of: a configurable quorum percentage of the group's peers replied, or the Phase-1
timeout.

**Acceptance Scenarios**:

1. **Given** a 10-node group and 80% quorum, **When** 8 peers have replied, **Then** the Phase-2
   transition fires without waiting for the last two.
2. **Given** all keys satisfied after the first peer's reply, **When** none remain unsatisfied,
   **Then** the operation completes immediately.
3. **Given** an operation deadline of 50 ms, **When** no peer replies within it, **Then** all
   keys are `Err(NotFound)` and the operation completes without hanging.

---

### User Story 7 - Peer Departure (Priority: P3)

If a zyre `Exit` for a peer arrives during an operation, the actor deletes that peer's cached
reply, returns any of its in-progress keys to the unsatisfied set, and — before returning any
landing slot exposed to that peer to the allocator — issues a responder `Disconnect { node }`
and blocks for `DisconnectAck`. Completion criteria are then re-evaluated.

**Why this priority**: Prevents a hang on a departed node and preserves memory safety (no late
one-sided write into a reclaimed slot).

**Acceptance Scenarios**:

1. **Given** peer B is expected to reply, **When** a zyre `Exit` for B arrives, **Then** B's
   cached reply is dropped, B's in-progress keys return to unsatisfied, and completion is
   re-evaluated.
2. **Given** B had an in-flight RDMA landing slot, **When** B exits, **Then** the actor receives
   `DisconnectAck` for B before that slot is returned to the allocator.

---

### Edge Cases

- Node receives its own SHOUT → not delivered by zyre; a defensive self-id filter drops it if it
  ever appears.
- KEY_RESPONSE / RDMA_STATUS whose `op_id` is not in the active-operation map → discarded as
  stale, without error.
- `batch_lookup` with an empty entry list → returns an empty result vector immediately.
- No peers in the group at operation start → all entries `Err(NotFound)` immediately.
- A batch larger than the max keys per KEY_QUERY → split across multiple KEY_QUERY messages under
  one `op_id`; responses aggregate before completion criteria apply.
- Concurrent `batch_lookup` for the same missing key → the second attaches to the actor's in-flight
  index as a follower and waits (single-flight) rather than launching a duplicate remote fetch.
- Placeholder allocation fails (memory-tier pool full / cannot evict) → that key is finalized
  `Err(NotFound)` (cannot receive the value).
- RDMA_REQUEST arrives for a key this node no longer holds (evicted) → RDMA_STATUS(KeyNoLongerAvailable).
- An inbound frame with an unrecognized `msg_type`, or a malformed/truncated frame that fails to
  decode → logged and dropped; the poll loop continues (FR-018).
- On RDMA_STATUS(Success), publishing the landing slot races a concurrent publisher and
  `create_memory_tier_entry` returns `AlreadyExists` → success only if the resident entry's size
  matches the slot length; a size mismatch discards the private slot and leaves the resident entry
  untouched (never evicted) (FR-006).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The component MUST implement `IRemoteLookup`. `batch_lookup` MUST take
  `&[(CacheKey, u32 /* expected size */)]` and return one positional `Result<(),
  RemoteLookupError>` per entry, where `Ok(())` means the key is now resident in the local
  memory tier. It MUST also implement `initialize(config: LookupConfig)` (see FR-022),
  `join_cluster(endpoint: &str)`, and `leave_cluster()`.
  *(The prior `IpcHandle` parameter is removed — remote-lookup never touches GPU memory.)*
- **FR-002**: The component MUST run as an actor on a dedicated OS thread that owns an
  `IZyreNode` (from the `IZyre` factory) and polls it in its event loop.
- **FR-003**: On activation the actor MUST join the single configured zyre group
  (`LookupConfig.group`, default `"remote_lookup"`); on deactivation it MUST leave it. The
  group name is configuration, not a constant — an integrating deployment MAY override it
  (e.g. per-cluster or per-tester) to isolate meshes. `join_cluster`/`leave_cluster`
  gossip-bind or connect the underlying zyre node to the cluster.
- **FR-004**: The wire identity of an entry MUST be the tuple `(key, size)`. KEY_QUERY MUST carry
  `(key, size)` pairs; a peer that holds `key` at a different size MUST report it as not
  available (no size-mismatch status exists on the wire).
- **FR-005** (client, Phase 1): On `batch_lookup`, the actor MUST SHOUT a KEY_QUERY for all
  requested `(key, size)` entries (splitting into multiple messages under one `op_id` if the
  batch exceeds the configured max keys per query), and initialize per-operation state:
  unsatisfied set, per-key status, per-peer cached replies, and the operation deadline.
- **FR-006** (client, landing slot / publish-on-success): To receive a value, the actor MUST
  reserve a **private** local landing slot (`memory_tier.insert`) whose DRAM address lies within
  the responder's pre-registered pool. The slot MUST NOT be published to dispatch-map while the
  fill is in flight. On RDMA success the actor MUST publish it via
  `create_memory_tier_entry(key, addr, len)` then `release_write(key)`; on failure/peer-exit it
  MUST discard the slot with `memory_tier.remove(key)` and MUST NOT create a dispatch-map entry.
  (No dispatch-map write-reference is held across the transfer; dependency D1 is dropped.) A racing
  `create_memory_tier_entry` that returns `AlreadyExists` counts as success **only when the
  existing entry's size equals the landing slot's length** (`entry_size(key) == len`); if the
  resident entry has a **different** size the collision is treated as unsatisfied — the actor MUST
  discard its private slot (`memory_tier.remove(key)`) and MUST NOT evict or overwrite the resident
  entry (never-evict-on-collision). *(Backfilled 2026-08-20 — documents the shipped
  `publish_success` size-check, `src/actor.rs:576-591`; see `knowledge/size-mismatch-handling.md`.)*
- **FR-006a** (client, greedy request): On each KEY_RESPONSE, the actor MUST, for that peer's
  reported **memory** hits that are unsatisfied and not in-progress, mark them in-progress and
  whisper an RDMA_REQUEST to that peer with the landing region(s); it MUST also record the peer's
  full reply (memory + disk sets) for later phases. Keys already in-progress or satisfied MUST be
  skipped.
- **FR-007** (client, RDMA region): Each RDMA_REQUEST MUST carry the requester's responder
  `Endpoint` and, per key, a `RemoteRegion { addr, rkey, length }` where `addr`/`length` are the
  private landing slot and `rkey` is the single pool-wide rkey the responder exposes (cached at
  startup).
- **FR-008** (client, success): On RDMA_STATUS(Success) for a key, the actor MUST publish the
  landing slot (per FR-006) and mark the key satisfied.
- **FR-009** (client, failure): On RDMA_STATUS(UnableToConnect | KeyNoLongerAvailable) for a key,
  the actor MUST return the key from in-progress to unsatisfied, eligible for retry. Per the wire
  protocol's ordering & safety invariants (`contracts/wire-protocol.md`), a serving peer sends
  RDMA_STATUS only after reaping its RDMA completions, and a non-success status means zero bytes
  were written for that key — so the landing slot MAY be reclaimed immediately on receipt of any
  RDMA_STATUS (success or failure) while the peer remains a live zyre member. The
  teardown-before-reclaim gate of FR-014 (blocking for `DisconnectAck`) applies only to the
  peer-departure path (no RDMA_STATUS was received at all), not to this RDMA_STATUS-received path.
- **FR-010** (client, Phase 2): After the Phase-1 transition (quorum% of the group's peers
  replied, or Phase-1 timeout), the actor MUST re-scan the **cached** replies (no new SHOUT) and,
  for still-unsatisfied keys, whisper RDMA_REQUESTs to peers that reported the key on **disk**,
  preferring peers not already tried for that key.
- **FR-011** (client, retries): The actor MUST perform at most a configurable number of retry
  rounds re-targeting still-unsatisfied keys to alternate cached peers (memory preferred, then
  disk). It MUST NOT re-target a key to a peer already known to have failed it in this operation.
- **FR-012** (client, completion): The actor MUST finalize an operation at the first of: all keys
  satisfied; no cached peer holds any remaining unsatisfied key and no further replies are
  expected; the retry-round cap is reached; or the operation deadline expires. Unsatisfied keys
  at finalization MUST be reported `Err(NotFound)` and their unpublished landing slots discarded.
- **FR-013** (client, peer exit): On a zyre `Exit` for a peer, the actor MUST delete that peer's
  cached reply, return its in-progress keys to unsatisfied, and re-evaluate completion.
- **FR-014** (memory safety, teardown-before-reclaim): Before returning a landing slot exposed to
  a peer that has departed (or is otherwise being severed) to the allocator, the actor MUST issue
  `ResponderCommand::Disconnect { node }` and block for `ResponderEvent::DisconnectAck`. Late
  one-sided writes MUST NOT be able to land into a reclaimed slot. Completing the same-key waiters
  with a not-found result MAY happen immediately; only the physical slot reclaim waits on the ack.
  The block for `DisconnectAck` MUST be **bounded** by a fixed `DISCONNECT_ACK_TIMEOUT` (a hardcoded
  500 ms constant, `src/actor.rs:37`); if the ack is lost the actor gives up the wait rather than
  hanging its poll loop. This ack-handshake bound is a fixed constant, deliberately **not** a
  `LookupConfig` knob, and is distinct from the configurable `connection_teardown_timeout` orphan
  grace of FR-031 (that timer decides *when* an orphan is force-torn-down; this bound caps *how
  long* the resulting ack handshake may block). *(Backfilled 2026-08-20 — documents the shipped
  `DISCONNECT_ACK_TIMEOUT` bound, `src/actor.rs:37,1020-1033`.)*
- **FR-015** (server, availability): On a KEY_QUERY SHOUT from another peer, the actor MUST, for
  each `(key, size)`, consult the dispatch-map: a memory-tier match with equal size ⇒ memory; a
  block/disk match with equal size ⇒ disk; otherwise not available. It MUST whisper a KEY_RESPONSE
  echoing `op_id` with the per-key classification.
- **FR-016** (server, push): On an RDMA_REQUEST whisper, for any requested key that `dm.lookup`
  reports as disk-only (`BlockDevice`), the actor MUST first promote it to the memory tier via
  `dispatcher.promote_to_memory_tier(&[key])` (batching all such keys in the request) and re-consult
  `dm.lookup` to obtain the resulting `MemoryTier{ptr,size}`. It MUST then pin each requested value
  (read reference), delegate the write to `IRemoteLookupRdmaInitiator::push_async(endpoint,
  [(key, RemoteRegion)], on_complete)`, and whisper a per-key RDMA_STATUS mapped from the
  `PushStatus` values `on_complete` reports (`Success`→Success, `UnableToConnect`→UnableToConnect,
  `KeyNotFound`→KeyNoLongerAvailable, `SizeMismatch`→KeyNoLongerAvailable defensively).
- **FR-016a** (server, pin lifetime): Because `push_async` returns before the NIC has finished
  reading the pinned values, the read references MUST be owned by the completion callback and
  released when it runs — never at the submission site. They MUST also be released if that callback
  is dropped rather than invoked (a rejected submission, or teardown), so no path can leak a pin: a
  leaked read reference makes its entry permanently unevictable and is indistinguishable from a live
  reader, and there is no leak detector.
- **FR-016b** (server, always answers): Every RDMA_REQUEST MUST produce exactly one RDMA_STATUS
  whisper, including when the push is rejected before submission, so the requester never has to wait
  out its operation deadline to learn the outcome.
- **FR-017** (server, promotion failure): If after `promote_to_memory_tier` a key is still not
  `MemoryTier`-resident (promotion failed or the entry was evicted — the call is best-effort and
  does not propagate per-key errors), the actor MUST report that key as `KeyNoLongerAvailable`
  rather than attempting the push.
- **FR-018** (framing): All wire messages MUST use a `[version: u8][msg_type: u8]` header followed
  by an `op_id: u64` for forward compatibility and correlation. Two classes of non-actionable
  inbound frame MUST be **ignored** (dropped without processing and without aborting the poll
  loop): (a) frames whose `msg_type` is not recognized (decoded as `WireMessage::Unknown`), and
  (b) malformed/truncated frames that fail to decode (`WireMessage::decode` returns `Err`, e.g.
  a short buffer, bad tag, or bad UTF-8). Both classes MUST be **logged** before being dropped so a
  version/framing mismatch is diagnosable. Servers MUST echo `op_id` in KEY_RESPONSE and
  RDMA_STATUS. *(Backfilled 2026-08-20 — the malformed/truncated-frame ignore class (b) was
  previously unspecced; `src/actor.rs:314`. The **logging** half was aligned 2026-09-03: `on_wire`
  now logs both arms via the optional `ILogger` receptacle before dropping — the malformed-frame arm
  logs the sender, byte length, and decode error; the `Unknown` arm logs the sender plus the frame's
  `version`/`msg_type`/`op_id` (`src/actor.rs`). Both classes remain ignored (poll loop continues).)*
- **FR-019** (stale responses): A KEY_RESPONSE or RDMA_STATUS whose `op_id` is not in the
  active-operation map MUST be discarded without error.
- **FR-020** (concurrency): The actor MUST support multiple concurrent in-flight operations, each
  keyed by a unique `op_id`, interleaving event handling across them; a slow or timing-out
  operation MUST NOT block others. (`batch_lookup` blocks its calling thread until its own
  operation finalizes.)
- **FR-021** (self-filter): The actor MUST ignore any SHOUT whose peer id equals its own zyre uuid
  (defensive; zyre does not deliver self-SHOUTs).
- **FR-022** (configuration): The Phase-1 quorum percentage, Phase-1 timeout, overall operation
  deadline, retry-round cap, and max keys per KEY_QUERY MUST be configurable via a `LookupConfig`
  supplied to `IRemoteLookup::initialize(LookupConfig)` (mirroring `IDispatcher::initialize(
  DispatcherConfig)`). `LookupConfig` MUST be a public `interfaces` type deriving `Default` with a
  sensible default per field; the component MUST run on defaults if the integrating profile does
  not run an init hook. This keeps YAML-driven configuration robust: adding a knob is an additive
  `LookupConfig` field, and the `certus-server-yaml` `init_remote_lookup` hook builds it with
  `..Default::default()` so config growth never breaks the mainline.
- **FR-023** (receptacles): The component MUST declare receptacles for `IZyre`, `IDispatchMap`,
  `IMemoryTier`, `IDispatcher` (US4 disk promotion — see FR-016), `IRemoteLookupRdmaInitiator`,
  `IRemoteLookupRdmaResponder`, `IRemoteLookupRdmaResponderAdmin` (lifecycle: bind/init/shutdown of
  the responder — see FR-025), and `ILogger`.
- **FR-024** (no direct RDMA): The component MUST NOT contain RDMA transport logic. Outbound
  writes go through `IRemoteLookupRdmaInitiator`; the inbound accept side and its teardown go
  through `IRemoteLookupRdmaResponder`.
- **FR-025** (responder wiring & rkey): On startup the actor MUST initialize the responder via the
  `responder_admin: IRemoteLookupRdmaResponderAdmin` receptacle (bind IP set by mainline), read its
  bound `Endpoint` (to advertise) and its pool-wide `rkey` (to place in RDMA_REQUESTs), and open its
  control channel (via `responder: IRemoteLookupRdmaResponder`) to issue `Disconnect` and receive
  `DisconnectAck`/`Connected`/errors. *(This assumes the responder registers the memory-tier pool
  and exposes its rkey — see Dependencies.)*
- **FR-026** (single-flight): Concurrent `batch_lookup` operations requesting the same
  unsatisfied `(key, size)` MUST coalesce onto one in-flight fetch via the actor's per-key
  in-flight index; the later operation attaches as a follower and waits for the in-flight fill
  rather than issuing a duplicate remote fetch.
- **FR-027** (connection warming at discovery): The actor MUST advertise its responder endpoint in
  its zyre presence header, and on discovering a peer (zyre `Enter`) that advertises one, MUST
  proactively warm an RDMA connection to that peer's responder via
  `IRemoteLookupRdmaInitiator::connect`, so the cold `rdma_cm` connect (measured in seconds)
  happens at discovery time rather than inline on the first serve. Warming is best-effort: a failed
  warm caches nothing and is not surfaced as an error (the serve reconnects lazily).
- **FR-028** (poll-loop responsiveness): The actor MUST NOT run a blocking RDMA operation (a warm
  connect, or the one-sided write of a serve) inline on its poll-loop thread — a cold connect would
  otherwise stall all zyre event, status, and teardown processing. It MUST hand these to an
  off-loop worker and continue polling; the worker returns each serve's per-key statuses to the
  poll loop, which owns the zyre node and whispers the RDMA_STATUS.
- **FR-029** (out-of-interface lifecycle/test hooks): `RemoteLookupComponent` MAY expose a small
  set of `pub fn`s outside `IRemoteLookup` for lifecycle control and test coordination that cannot
  be expressed through the interface (which has no mutable-borrow or thread-join surface). These
  are implementation-facing, not part of the `IRemoteLookup` contract, and MUST NOT be relied on by
  other components except for actor teardown ordering:
  - `peers_seen() -> usize` — the count of peers currently visible to the actor's zyre node (ENTER
    minus EXIT). Used by tests as a discovery barrier before driving the protocol; not used in
    production wiring.
  - `signal_shutdown()` — signals the actor to stop polling **without** waiting for it to exit.
    Idempotent and safe to call before `initialize`. Needed for two-phase multi-actor teardown: all
    actors sharing a `zyre`/`czmq` context must stop polling before any one node is destroyed, or a
    live actor's `try_recv` on a torn-down context trips a `zpoller` assertion. Used in production
    wiring (e.g. `apps/certus-server`) when multiple zyre-backed components share a process.
  - `shutdown()` — calls `signal_shutdown()`, then joins the actor thread, then joins the initiator
    worker thread (in that order, so the worker's channel closes deterministically once the actor —
    the sole sender — has exited). Idempotent and safe to call before `initialize`; also invoked
    from `Drop`.

- **FR-030** *(Backfilled 2026-08-07 — documents implemented behavior beyond FR-020.)* The
  `LookupConfig.caller_wait` knob MUST decouple how long the calling thread blocks from the
  operation's own `op_deadline`. When set, `batch_lookup` MAY return to its caller (with the
  results known so far) while the underlying operation continues running in the actor to
  completion — a key resolved after the caller returns is still published to the local memory
  tier. This is a deliberate extension of FR-020 ("blocks until its operation finalizes"): the
  operation still finalizes on the FR-012 criteria; only the *caller's* blocking window is
  shortened. Tested in `tests/mesh.rs`.

- **FR-031** *(Backfilled 2026-08-07 — documents a memory-safety backstop beyond FR-014.)* In
  addition to the departure-triggered teardown-before-reclaim of FR-014, the actor MUST run a
  timer-driven force-reclaim backstop (`LookupConfig.connection_teardown_timeout`, `tick_orphans`)
  for a peer that neither reports a late RDMA_STATUS nor emits a zyre `Exit`. When the timeout
  elapses for an orphaned landing slot, the actor force-tears-down the connection and reclaims the
  slot, so a silently-vanished peer cannot pin a slot indefinitely. Tested in `tests/mesh.rs`.

- **FR-032** *(Backfilled 2026-08-07 — memory-safety detail, previously unspecced.)* The actor
  MUST NOT re-reserve a key whose landing slot is currently orphaned (awaiting teardown/reclaim);
  the orphan-reuse guard prevents a new fetch from aliasing DRAM that a late one-sided write from
  the prior peer could still touch. This complements FR-014/FR-031.

- **FR-033** *(Backfilled 2026-08-07; `bind_ip` added 2026-09-03 — completes the FR-022
  configuration surface.)* Beyond the
  quorum/phase-1/deadline/retry-cap/max-keys knobs of FR-022, `LookupConfig` MUST also carry
  `bind_ip` (the RoCE IPv4 the responder binds to, forwarded to the responder admin via
  `set_bind_ip` during `initialize` — `src/lib.rs:146`; `interfaces/src/iremote_lookup.rs:63`),
  `actor_cpu` (best-effort NUMA/CPU pinning of the actor thread), `discovery` (zyre gossip
  discovery configuration), and `node_endpoint` (the advertised node endpoint). All derive sensible
  defaults per FR-022's `Default` contract (`bind_ip` defaults to the empty string, deferring the
  bind address to the responder's own resolution — `interfaces/src/iremote_lookup.rs:92`).

- **FR-034** *(Backfilled 2026-08-07 — build plumbing, previously unspecced.)* The crate MUST expose
  an `integrity-check` Cargo feature that forwards to `interfaces/integrity-check`, enabling the
  checksum accessors on cache values. This is build configuration with no runtime-behavior
  requirement of its own.

### Key Entities

- **Operation**: One `batch_lookup` invocation, keyed by `op_id`. Holds the unsatisfied set,
  per-key status (unsatisfied / in-progress / satisfied), per-peer cached replies, current phase
  (Memory / DiskFallback), retry-round count, and the deadline.
- **PeerReply**: A cached KEY_RESPONSE from one peer: the keys it reported in memory and the keys
  it reported on disk (with stored sizes). Dropped when the peer exits.
- **LandingSlot**: A slot reserved *privately* in the pre-registered DRAM pool to receive one
  value; **not** published to dispatch-map until the fill succeeds. Expressed to serving peers as
  `RemoteRegion { addr, rkey, length }`. Published (`create_memory_tier_entry` + `release_write`)
  on success, discarded (`memory_tier.remove`) on failure.
- **WireMessage**: A framed message — `[version][msg_type]` + `op_id: u64` + payload — one of
  KEY_QUERY, KEY_RESPONSE, RDMA_REQUEST, RDMA_STATUS.
- **InitiatorWorker**: An off-loop worker (its own thread) that runs the two blocking initiator
  operations — *warm* (proactively connect to a discovered peer's responder) and *serve* (answer a
  peer's RDMA_REQUEST) — so the poll-loop thread never blocks on a cold `rdma_cm` connect. It
  receives commands over a channel and posts each serve's per-key statuses back to the poll loop.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A `(key, size)` held in a peer's memory tier becomes resident in the local memory
  tier within a single `batch_lookup`, with end-to-end added latency dominated by RDMA transfer
  time (sub-millisecond for typical entry sizes on RoCE/IB).
- **SC-002**: When no peer holds a key, `batch_lookup` reports `Err(NotFound)` within the
  configured deadline (`op_deadline`, default 50 ms; see research Decision 9) without hanging,
  leaving no landing slot in the memory tier.
- **SC-003**: Greedy Phase-1 dispatch is prompt: an RDMA_REQUEST is whispered on the same event as
  the KEY_RESPONSE that first reports a satisfiable memory hit, with no intervening poll cycle
  (validated structurally over a mock zyre/RDMA seam).
- **SC-004**: On an RDMA failure where an alternate cached peer holds the key, retry to the
  alternate succeeds on the first attempt in ≥ 95% of cases.
- **SC-005**: No landing slot is returned to the allocator while exposed to a departed peer
  without a preceding `DisconnectAck`; no read pin on a served value is leaked across a push
  (success or failure); no dispatch-map write reference is leaked on any completion path (the
  publish path takes `write_ref` only briefly, between `create_memory_tier_entry` and
  `release_write`, and never across an RDMA transfer).
- **SC-006**: Peer departure mid-operation completes the operation as soon as the remaining
  completion criteria are met — no spurious wait to the deadline.
- **SC-007**: A `(key, size)` request against a peer holding `key` at a different size yields a
  not-available reply and never a partial or mismatched write.
- **SC-008**: Concurrent `batch_lookup`s for the same missing key issue exactly one remote fetch
  (single-flight, via the actor's in-flight index); the follower observes the published value or
  the not-found result.

## Assumptions

- All Certus nodes share a LAN with zyre-compatible discovery, and a RoCE/IB fabric reachable at
  the mainline-supplied bind IP.
- The DRAM memory tier is allocated once at `initialize(pool_size, numa_node)` and **does not
  grow**; the responder registers the whole pool once with `REMOTE_WRITE` and the rkey is stable
  for the process lifetime. Growing the pool would require a re-registration protocol (out of
  scope).
- The dispatch-map distinguishes memory-tier from block/disk placement, reports stored size, and
  supports read/write reference pinning. remote-lookup relies only on its existing surface
  (`create_memory_tier_entry` sets `write_ref=1`; `release_write` makes the entry readable;
  `lookup` blocks while a writer is active) — publish-on-success requires no new dispatch-map
  behavior (D1 dropped, see Dependencies).
- The dispatcher performs the DRAM→GPU delivery for any key remote-lookup resolves; remote-lookup
  itself never touches GPU memory.
- Entry sizes are bounded (practical max ~128 MiB) and RDMA transfers are reliable within range.
- Zyre group membership is eventually consistent; the deadline is the safety net for stale views.
- All timing is local to the requesting node (no cross-node clock dependence).
- The protocol is unencrypted; security is provided at the network layer (trusted LAN).

## Out of Scope

- GPU delivery (memory-tier → device); owned by the dispatcher / gpu-services.
- Per-request RDMA memory registration (the whole tier is pre-registered once).
- Changing the dispatcher's local tier-check order (a future memory→remote-memory→disk→remote-disk
  ordering is a dispatcher concern, noted for later).
- Encryption/authentication of the control or data plane.
- The RDMA transport internals themselves (owned by the initiator/responder components).
- GPUDirect RDMA (peers writing directly into GPU memory) — explicitly not a goal.
