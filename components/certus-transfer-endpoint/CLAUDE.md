# certus-transfer-endpoint

A **symmetric** RDMA endpoint for explicit certus-to-certus KV cache transfer. A
destination reserves landing slots, names one peer (chosen externally by llm-d),
and asks for a run of block keys; the source RDMA-writes the values it holds into
those slots and replies with a per-key outcome. It replaces zyre broadcast/reap —
no broadcast, no polling, no reap.

Design documents live with the component, not at repo root:
`specs/001-rdma-cache-transfer/` (spec, plan, research D1–D19, contracts, tasks)
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
  `IDispatchMap::lookup`; never probe-then-pin. Never release a read reference
  for an answer that was not pinned (`NotExist`, `MismatchSize`) — that
  decrements another reader's pin and no ordinary test catches it. Pins are per
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
- **`OP_HELLO` and `OP_ERROR` can never be versioned.** They are frozen at version 0,
  permanently, because a peer that cannot parse version N must still parse them —
  otherwise an unparseable message is answered with an unparseable error. Extend them
  through their *content* (the open reason-code space, the counted capability list),
  never their layout. Bytes 0-15 of every header are frozen for the same reason
  (D20).
- **`Protocol` and `Deadline` are both loud, with deliberately different blast
  radius.** A protocol error means the peer is alive and answered: fail that one
  context, leave the queue pair alone, let siblings finish. A deadline means the
  peer's state is unknown: force the QP to ERROR, which necessarily aborts the
  siblings. Routing a protocol error through the deadline's cleanup would destroy a
  healthy connection (FR-024 vs FR-023).
- **Chunk against the *peer's* `max_entries`, not your own.** Receive-buffer sizes are
  not assumed equal; `OP_HELLO` advertises them precisely because nothing checked that
  before (FR-025).
- **One protocol version governs the connection, not one per opcode.** Each end
  advertises a `[min, max]` range and the connection runs at `min(a.max, b.max)`.
  Per-kind negotiation is specifically rejected: it can represent a v2 request answered
  by a v1 reply, and a reply's meaning is tied to its request's version. The version in
  the fused opcode is a structural guard on the negotiated value, not the mechanism
  (D20 and its *Revision*).
- **Never answer an unprocessable message with silence.** Silence is
  indistinguishable from a hang, so it gets diagnosed as one — and then tears down a
  healthy connection. A stale binary on one node is the common cause.
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
- **Nobody decides the version and there is no `OP_HELLO_REPLY`.** Both ends send
  unsolicited and compute `min(a.max, b.max)` against floor `max(a.min, b.min)`. The
  rule is commutative, which is *why* the two ends cannot disagree — never replace it
  with an asymmetric rule or a reply message (D20).
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

Scaffolding. T001–T009 and T068 are done: the crate builds as a default member,
peer identity is implemented, `ITransferEndpoint` is defined in `interfaces`, and
the verbs FFI (`ffi.rs`), C shim (`wrapper.c`) and safe wrappers (`rdma.rs`, T009)
exist behind `rdma`. Nothing above them calls them yet; the seam is T010 and the
engine is Phase 2. See `specs/001-rdma-cache-transfer/tasks.md` for the task list and its
traceability matrix.
