# certus-transfer-endpoint

A **symmetric** RDMA endpoint for explicit certus-to-certus KV cache transfer. A
destination reserves landing slots, names one peer (chosen externally by llm-d),
and asks for a run of block keys; the source RDMA-writes the values it holds into
those slots and replies with a per-key outcome. It replaces zyre broadcast/reap —
no broadcast, no polling, no reap.

Design documents live with the component, not at repo root:
`specs/001-rdma-cache-transfer/` (spec, plan, research D1–D24, contracts, tasks)
and `.specify/memory/constitution.md`.

## The invariants that matter

Read these four before changing anything here. They are constitution principles
VIII–XI, and a review that does not check them is not a review.

- **VIII — the default build needs no hardware.** No rdma-core, no SPDK. The
  connection table, framing, credits and pin logic are all exercised over an
  in-process seam, and this crate **is** a workspace `default-member` (unlike
  `remote-lookup-rdma-initiator`/`-responder`, which link rdma-core
  unconditionally). Hardware tests are `#[ignore]`d. If a behaviour can only be
  tested on a NIC, the seam is wrong.
- **IX — pin discipline.** Residency and pin come from **one**
  `IDispatchMap::lookup`; never probe-then-pin. Release exactly what it pinned,
  exactly once: never an unpinned answer (`NotExist`, `MismatchSize`) — that
  decrements another reader's pin and no ordinary test catches it — but always a
  pinned one you do not serve. `lookup` takes no size, so a **wrong-size entry
  comes back pinned** and must be released; the current map never returns
  `MismatchSize` at all. Pins are per
  entry, never per batch, released by the completion reaper owning their
  `wr_id`. A landing slot is not reclaimed or published until queue-pair
  teardown guarantees no write can still land.
- **X — bounded resources, graceful chunking.** Three *separately named* credit
  domains: send-queue credits, message credits, bounce-buffer occupancy. They
  bound different things and exhaust independently; conflating them over- or
  under-restricts two of the three. On exhaustion, chunk — never fail a request
  for want of a transient local resource. Serving a peer must not degrade the
  server's own cache: block-tier entries are **never** promoted to serve a
  remote request.
- **XI — loud failure over silent degradation.** This component exists to answer
  a research question by measurement, so a safety net that masks a defect and
  still yields a plausible number is worse than a crash. Deadlines default to
  logging full state and failing. Metrics must not make an unexercised path look
  like a working one — count attempts alongside completions.

## Things that are easy to get wrong

- **A peer is an instance, not a host.** Several instances run per node, one per
  NUMA domain, so identity is `"host:port"` and a missing port is an error,
  never a default (FR-021, D16). Keyed by host, NUMA siblings collapse into one
  connection.
- **The protection domain belongs to the endpoint, not the connection.** One PD
  per device, registrations outliving every queue pair, so an `rkey` already on
  the wire survives a reconnect (D17). The predecessor crates do the opposite —
  do not copy them here. `ibv_dealloc_pd` needs every QP and MR gone first.
- **A deadline must change queue-pair state, not just log it.** On expiry: QP to
  ERROR, drain completions, *then* reclaim slots, then reconnect lazily (FR-023,
  D18). Logging the state is not changing it, and a late write into a reclaimed
  slot corrupts an unrelated entry. Expiry aborts every context on that
  connection; that blast radius is intended and logged.
- **Do not add phase barriers.** Memory-tier writes post as soon as they
  resolve; each block-tier write posts when its own device read completes. Never
  lease-all/read-all/post-all (FR-022, D19).
- **Do not build cross-message pipelining.** It is worth ~0.1% and is already
  free via message credits. D19 has the arithmetic; the only barrier worth
  removing is the block-tier one, whose cost is milliseconds.
- **The mock seam implements the interface; it is not a bypass of it.** Do not
  copy the initiator's `#[cfg(not(feature = "rdma"))] -> Err(NotInitialized)`
  pattern, which makes the interface untestable without hardware.
- **`OP_HELLO`, `OP_HELLO_REPLY` and `OP_ERROR` can never be versioned.** They are frozen at version 0,
  permanently, because a peer that cannot parse version N must still parse them —
  otherwise an unparseable message is answered with an unparseable error. Extend them
  through their *content* (the open reason-code space, the counted capability list),
  never their layout. Bytes 0-15 of every header are frozen for the same reason
  (D20).
- **`Protocol` and `Deadline` are both loud, with deliberately different blast
  radius.** An `OP_ERROR` for our request means the peer is alive and acted on
  nothing: fail that one context, leave the queue pair alone, let siblings finish. A
  deadline means the peer's state is unknown: force the QP to ERROR, which
  necessarily aborts the siblings. Routing an `OP_ERROR` through the deadline's
  cleanup would destroy a healthy connection (FR-024 vs FR-023). A *bad reply* is
  the third case and does tear down: its sender may have written into our slots.
- **Chunk against the *peer's* `max_entries`, not your own.** Receive-buffer sizes are
  not assumed equal; `OP_HELLO` advertises them precisely because nothing checked that
  before (FR-025).
- **One protocol version governs the connection, not one per opcode.** Each end
  advertises a `[min, max]` range and the connection runs at `min(a.max, b.max)`.
  Per-kind negotiation is specifically rejected: it can represent a v2 request answered
  by a v1 reply, and a reply's meaning is tied to its request's version. The header's
  `version` field is a structural guard on the negotiated value, not the mechanism
  (D20 and its *Revision*, D21).
- **Never answer an unprocessable request with silence — and never answer a reply.**
  Silence to a request is indistinguishable from a hang, so it gets diagnosed as one
  and tears down a healthy connection; a stale binary on one node is the common
  cause. A reply is never answered, even a bad one: its sender holds no state to act
  on, and answering replies would allow an error loop. Report it loudly instead.
- **One connection carries both directions, so keep separate books.** An instance is
  destination for one context and source for another on the same queue pair. Outbound
  and inbound context tables must be *separate* — the requester allocates ids, so the
  same id can be live both ways — credits are per direction, and posted receive buffers
  are partitioned so a peer at full inbound-request quota cannot starve a reply you are
  waiting on (that one is an RNR deadlock, FR-026/D22).
- **Both ends may connect at once; exactly one connection must survive.** Keep the one
  whose initiator has the lower `PeerId`, resolved at accept via the connect
  `private_data`, before `OP_HELLO`. Two live connections defeat `disconnect`: tearing
  one down leaves the other able to write into a reclaimed slot (FR-027, D23).
- **The connect `private_data` identity is frozen, like `OP_HELLO`.** It travels
  before the version exchange, so a peer must always be able to parse it: extend it
  by a new format byte, never by changing format 1 (D24). Received private data is
  zero-padded, hence the explicit length.
- **Name every instance by the same string everywhere.** The table checks that an
  acceptor answers to the name it was dialed by and fails the link otherwise;
  `10.0.0.2:7000` and `node2:7000` are different instances to it, and there is no
  resolver (D16, D24).
- **A duplicate the race cannot explain means the peer lost its link: newer wins.**
  The old link goes to ERROR first. The one undecidable case — a crashed-and-
  restarted peer that out-ranks nobody — is rejected until a deadline notices the
  stale link (D24's known gap). Do not "fix" it by accepting every duplicate.
- **Nobody decides the version, and `OP_HELLO_REPLY` must stay empty.** Both ends
  send `OP_HELLO` unsolicited and compute `min(a.max, b.max)` against floor
  `max(a.min, b.min)`. The rule is commutative, which is *why* the two ends cannot
  disagree. `OP_HELLO_REPLY` exists only so every request draws one reply and the
  credits balance; giving it content would create a second decision point (D20).
- **The version is a header field at offset 6, validated once, not fused into the
  opcode.** Opcodes name kinds. Fusing mints a new opcode per kind per version,
  including unchanged kinds, and the handler ends up masking the version off anyway
  (D21).
- **`ffi.rs` struct layouts are pinned, on both sides.** A `const` block in
  `ffi.rs` asserts Rust sizes/offsets against fixed numbers, and `wrapper.c`
  `_Static_assert`s the real headers against the same numbers. Change one, change
  both. Do not "fix" a layout by copying a predecessor crate's `ffi.rs` — four of
  its definitions were wrong (see `ffi.rs` module docs), including the `IBV_WR_*`
  numbering, under which a send is posted as an RDMA write.
- **The mock is a model of RC verbs, not a stub.** Sends execute in order and an
  RNR stall blocks everything behind it; writes are checked against the target's
  registrations; ERROR flushes; unsignaled requests hold their slot. If the engine
  passes over the mock only because the mock is lenient, the seam is wrong —
  tighten the mock, never loosen it to make a test pass.
- **`select_transport(BuildDefault)` must never fall back to the mock in an
  `rdma` build.** An in-process run reports plausible numbers from no fabric.
- **Pin state lives in the link's `Slab` and leaves it only through `complete`**
  (or a partial post's `unposted` list). Do not add another way out: a second
  path is a double release, which decrements another reader's pin. Post only via
  the table's `post_sends`/`post_recvs` (over `post_{send,recv}_tracked`) — they
  force `signaled` on, because an unsignaled success never completes and its pin
  would never come back.
- **Send credits count every send-queue request, not just RDMA writes.** FR-014
  says "writes", but requests and replies share the queue; counting only writes
  lets messages fill it under writes that think they have room. Receives hold no
  send credit — they are T080's partition. And poll a live link only through
  `reap`: it is what returns credits, so polling around it leaks them.
- **`stats()` counts in every build; only extras are feature-gated.** Never move a
  `TransferStats` field behind `telemetry`: the default build would report zeros
  for work it did. Send-queue gauges are read from the live credits, not counted
  twice. A counter still unfilled names its task in `telemetry.rs`.
- **A fetch with no usable reply fails as a whole; a push never does.** A fetch's
  completion is `Err` on deadline, refusal, teardown or size, and every slot of it is to
  be aborted; a push reports per entry, because some of its writes may have landed. Only
  a successful completion is a hit — a write that may have landed is reported as not
  landed, and the caller throws the slot out. Fire a fetch's `Err` only after the
  teardown barrier, never before.
- **The reply deadline runs from the send, not the `fetch` call.** A fetch waiting for a
  credit or the capability exchange is not overdue; timing it from the call turns
  back-pressure into spurious teardowns.
- **`Failed` is not `Miss`.** `Miss` means the source does not serve the entry
  (absent, wrong size, a tier this build cannot serve). Anything lost to a fault —
  connection, refused or flushed write, failed lookup — is `Failed`. Reporting a
  fault as a miss makes a broken path look like a cold cache.
- **The source replies only after its writes complete.** That is the whole of
  the ordering guarantee: a hit in a reply means the write landed, so the caller
  may commit the slot. A serving push finishes with `Finish::Reply`, queued only
  when its last write's completion is reaped — never sooner.
- **No data message before the capability exchange completes.** A link comes
  up with one bootstrap message credit, spent on `OP_HELLO`; data waits for both
  the peer's `OP_HELLO` and our `OP_HELLO_REPLY`. Pushes check the session is
  `Ready` before resolving a single entry.
- **Look up only when a credit is free.** A push resolves an entry — and so
  pins it — only when its write can post at once. Looking ahead "to keep the
  pipe full" breaks SC-004: pins held would exceed the credits.
- **Serving a peer still touches the source's LRU** (D6): the dispatch map's
  `lookup` refreshes the entry as it pins. Invariant 6 is unmet for memory-tier
  entries until a non-touching lookup exists. Do not claim otherwise.
- **A panic on the endpoint thread aborts the process.** The framework actor would
  catch it and carry on with a corrupt table, risking a silent double release.
  Never call table code on that thread outside `fatal_on_panic`.
- **Only `shutdown` stops an endpoint.** `define_component!` holds `Arc`s of the
  component in its own interface map, so dropping every handle does not drop it.
  And `shutdown` must drain before it drops the table, or the table's own drop
  aborts.
- **A `wr_id` carries a generation for a reason.** Draining links are polled after
  ERROR, so a slot can be reused while its previous occupant's completion is still
  in flight. Never strip it to "simplify" back to the predecessor's bare index.
- **Ask the device for QP state, never `ibv_qp::state`.** That field is
  libibverbs' cache of the last state software set; it stays RTS after the device
  forces ERROR. `QueuePair::state` uses `ibv_query_qp`. The predecessors read the
  cache.
- **Verbs teardown order is enforced by `Arc`, not by care.** Regions, CQs and QPs
  hold what they depend on; a release that fails anyway panics. Do not add a
  manual `close()` that bypasses this.
- **`RemoteRegion`'s field is `length`, not `size`**, and on the wire the
  trailing two `u32`s are `size` then `rkey` — the opposite order from the
  struct. Both are `u32`, so a transposition silently swaps a length for a key.
  Name fields explicitly in the codec.

## Build and test

```sh
cargo build                                   # default members, no NIC, no NVMe
cargo test -p certus-transfer-endpoint
cargo clippy -p certus-transfer-endpoint --all-targets -- -D warnings
cargo doc -p certus-transfer-endpoint --no-deps
cargo test -p certus-transfer-endpoint --features rdma -- --ignored   # needs a NIC
```

`rdma` and `spdk` are independently selectable. Without `spdk`, a block-tier
lookup answer is reported as a **miss**, never an error (D14).

## Current state

Scaffolding. T001–T036, T049–T052, T068, T070, T071 and T074–T082 are done, except T011 (Phases 3, 5 and 8 complete: US2 legacy push and US1 fetch work end to end over the mock, with a fail-loud reply deadline): the crate builds as a default member,
peer identity is implemented, `ITransferEndpoint` is defined in `interfaces`, and
the verbs FFI (`ffi.rs`), C shim (`wrapper.c`) and safe wrappers (`rdma.rs`, T009)
exist behind `rdma`, and the transport seam plus an in-process mock exist in every
build (`connection/transport.rs`, `connection/mock.rs`, T010), driven by the
connection table (`connection/table.rs`, T012, which also resolves simultaneous
connect), with a `wr_id` slab per link (`connection/slab.rs`, T013) and send-queue
credits (`connection/credits.rs`, T014), and counters (`telemetry.rs`, T015).
The component (`CertusTransferEndpoint`, T016) owns the table on its endpoint
thread (`endpoint.rs`) behind `ITransferEndpointAdmin`. `push_async` (T017) serves
memory-tier entries end to end with the full pin discipline (`serve.rs`, T022-T026),
and `push` (T018) waits on it; landing regions register once for the endpoint's
life (T020);
`disconnect` and `stats` work; the rest say which task they land in. The verbs side of the
seam is T053. See `specs/001-rdma-cache-transfer/tasks.md` for the task list and its
traceability matrix.
