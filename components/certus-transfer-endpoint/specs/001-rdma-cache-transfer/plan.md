# Implementation Plan: Explicit certus-to-certus KV cache transfer over a symmetric RDMA endpoint

**Branch**: `001-rdma-cache-transfer` | **Date**: 2026-10-14 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from
`components/certus-transfer-endpoint/specs/001-rdma-cache-transfer/spec.md`

## Summary

A new component, `certus-transfer-endpoint`, providing a **symmetric** RDMA endpoint: one
queue pair per peer pair, either end able to post work requests. It adds a verbs
request/reply message layer so a destination can name a single peer and a key run and have
that peer push the values into pre-reserved landing slots — replacing zyre broadcast with an
explicit, point-to-point transfer.

It is designed to subsume `remote-lookup-rdma-initiator` and
`remote-lookup-rdma-responder`, exposing a messaging-free subset that `remote-lookup` can
use unchanged. Cutover and retirement of the old crates are deliberately **later** tasks, so
P1 lands without touching working code. Rationale for every design choice is in
[research.md](./research.md).

## Technical Context

**Language/Version**: Rust (workspace toolchain)

**Primary Dependencies**: `component-framework` (`define_component!`/`define_interface!`,
actors, channels), `components/interfaces` (`ILogger`, `IDispatchMap`, `IMemoryTier`, and `IDispatcher`
under the `spdk` feature — see research D13), `anyhow`. rdma-core (libibverbs/librdmacm) only under the `rdma`
feature, via a hand-written FFI layer and a C shim built by `cc`/`pkg-config`.

**Storage**: certus memory tier (DPDK hugepages) for landing slots and for memory-tier
serving; certus block tier for block-tier serving, read by the dispatcher into a bounce
reserve carved from the memory tier's already-registered pool (research D13, D15).

**Testing**: `cargo test` over an in-process transport seam (no rdma-core, no SPDK, no
zyre); `#[ignore]`d hardware loopback tests under the `rdma` feature, mirroring the existing
T034 pattern; Criterion benchmarks for the per-entry push path (CLAUDE.md requires them for
performance-sensitive code).

**Target Platform**: Linux, x86-64, RoCE-capable NIC. Two-node minimum for the real path;
single-host RDMA loopback for CI-adjacent verification.

**Project Type**: library component in `components/`, following the established component
model (receptacles + actor-on-dedicated-thread).

**Performance Goals**: saturate the available link on memory-tier-served transfers; per-entry
RDMA posting must not serialise behind device reads (hence the bounce-buffer decoupling and
the reader threads).
No latency target is set at this stage — the research question is whether explicit transfer
beats recompute, and the measurement belongs to a later benchmark task.

**Constraints**: concurrently held read pins on a source must never exceed the configured
send-queue credit limit, regardless of request length (SC-004). Serving a block-tier entry
must leave the source's memory-tier occupancy and LRU order unchanged (SC-005). Default
build must succeed with neither rdma-core nor SPDK present (SC-003). Peers are addressed at
instance granularity, `"ip:port"`, since several instances run per node (FR-021, D16).
Landing-region registration lives in a long-lived protection domain and outlives any queue
pair, so an `rkey` survives a reconnect (D17) — a deliberate departure from the predecessor
crates, which re-register per connection. A deadline must force the queue pair to ERROR and
drain it before reclaiming any slot, which aborts that connection's other contexts too
(FR-023, D18). Within a request the source must not phase-barrier: no draining the send
queue between entries, and no waiting on device reads before posting memory-tier writes
(FR-022, D19).

**Scale/Scope**: one endpoint per certus instance; a handful of peers per node in the
current rigs (2 nodes × 2 NUMA instances). Key runs up to a full model context — ~1,536
blocks / ~3.4 GiB at 2.25 MiB per block for Qwen3-8B at 24k tokens — which is why internal
chunking is mandatory rather than optional.

## Constitution Check

`.specify/memory/constitution.md` was **ratified at v1.0.0 on 2026-10-14** with 11 principles
— I–VII inherited from `remote-lookup`, and VIII–XI written for this component:
**VIII Hardware-Independent Default Build**, **IX Pin Discipline**, **X Bounded Resources,
Graceful Chunking**, and **XI Loud Failure Over Silent Degradation**. Those four are the
binding gates for this feature; IX and XI are the ones every task must respect, and Workflow
rule 6 requires any change touching pin acquisition or release to be reviewed against IX
explicitly.

The principles restate, in gate form, what the requirements and research already fix: VIII ⇒
SC-003 and D14, IX ⇒ FR-009…FR-011 and D6/D7, X ⇒ FR-014/FR-015 and D10, XI ⇒ FR-016 and D11.
Checking additionally against the repository's de facto conventions in `CLAUDE.md`:

| Convention | Compliance |
|---|---|
| Default members build without SPDK; plain `cargo build`/`cargo test --all` works | **Must hold.** Feature matrix per research D14: default = memory-tier serving over the mock seam; `spdk` only for block-tier serving. |
| `define_component!`/`define_interface!`, `IUnknown`, receptacles, actor-per-thread | Followed; endpoint owns a dedicated thread, as both existing crates do. |
| `rustfmt` default, `clippy -D warnings` | Followed. |
| Public APIs need doc comments with runnable examples; `cargo doc --no-deps` warning-free | Followed; the existing initiator's `lib.rs` example is the model. |
| Performance-sensitive code needs Criterion benchmarks | Required for the per-entry push path; responder crate already carries a Criterion dev-dependency to copy from. |
| `unsafe` requires `// SAFETY:` justification | Followed; all verbs FFI is unsafe and must carry it. |
| Component `CLAUDE.md` in sub-crates | Add one for this component. |

**No violations identified.** The one judgement call is scope: this component is larger than
a typical single-purpose crate because it merges two existing roles — justified in
Complexity Tracking below.

## Project Structure

### Documentation (this feature)

```
components/certus-transfer-endpoint/specs/001-rdma-cache-transfer/
├── spec.md          # what and why (done)
├── research.md      # settled decisions D1-D12 + open questions OQ1-OQ5 (done)
├── plan.md          # this file
├── contracts/       # ITransferEndpoint; wire format for request/reply
├── checklists/      # review checklists
├── evidence/        # measurement artifacts
└── tasks.md         # generated from this plan
```

### Source Code

```
components/certus-transfer-endpoint/
├── Cargo.toml       # features: default = [], rdma = [], spdk = [], telemetry = []
├── CLAUDE.md
├── build.rs         # cc + pkg-config for the verbs shim, under `rdma`
└── src/
    ├── lib.rs       # define_component!, receptacles, ITransferEndpoint impl
    ├── ffi.rs       # verbs FFI declarations — THE single copy (SC-008)
    ├── rdma.rs      # safe wrappers: QP lifecycle, reg_mr, post_send/recv, poll
    ├── connection/  # symmetric connection table, per-peer state machine, credits,
    │   │            #   wr_id slab (mod.rs)
    │   ├── transport.rs  # the verbs-shaped transport seam and per-build selection
    │   └── mock.rs       # in-process transport, compiled in every build (D14)
    ├── message.rs   # request/reply framing, context IDs, message credits
    ├── serve.rs     # source side: per-entry lookup/pin/push, outcome accumulation
    ├── bounce.rs    # block-tier reads: reader threads over IDispatcher::read_to_bounce (spdk)
    └── telemetry.rs # feature-gated metrics
```

**Structure Decision**: a single component rather than extending either existing crate.
`ffi.rs` is declared **once** here, which is the point of SC-008 — the two existing crates
currently declare 52 public names in common. `connection.rs` carries the
symmetric table because the existing initiator's equivalent is already the largest and most
developed piece (2,128 lines) and is the natural base.

## Implementation Phasing

Ordered so that `remote-lookup` is untouched until the new engine is proven.

**Phase 1 — engine, no messaging (satisfies P2 first, deliberately).**
FFI + safe wrappers + symmetric connection table + credits + `wr_id` slab + mock transport
seam + the messaging-free `push` entry point of FR-019. Target: the existing
`remote-lookup` test layer passes against this component when pointed at it *in a test
harness only* (SC-002), with `remote-lookup` itself unmodified.

**Phase 2 — per-entry pin discipline (D6, D7).**
Atomic lookup-and-pin, per-entry release via `wr_id` reaping, never releasing an unpinned
answer. Verifiable without any messaging. Gate: SC-004.

**Phase 3 — verbs messaging (FR-005 … FR-007).**
Unconditional posted receives, request/reply framing, context IDs, message credits with
blocking at zero. Gate: SC-001 — a fetch with zero zyre messages and zero third-node
messages.

**Phase 4 — block-tier serving (P3).**
A bounce reserve in the memory tier (`memory-tier`), `IDispatcher::read_to_bounce`
(`dispatcher`), and the endpoint's reader threads, early pin release and `hit_block_tier`
outcome. Three components, because reading an extent belongs to the dispatcher (D13). Gate:
SC-005.

**Phase 5 — bounds and chunking (FR-014, FR-015).**
Three named credit domains; internal chunking on exhaustion of any. Gate: SC-007.

**Phase 6 — fail-loud deadline (FR-016, FR-023).**
Detection plus the failing action; degrading action left unimplemented behind the flag. The
cleanup ordering is part of the gate, not an afterthought: queue pair to ERROR, drain, then
reclaim, then lazy reconnect, with the peer-wide abort logged (D18).

**Phase 7 (later, separate change) — cutover.**
Re-point `remote-lookup` at this component; delete
`remote-lookup-rdma-initiator` and `remote-lookup-rdma-responder`. Out of scope for FR-020
and not required for any P1 success criterion; designed and deferred in
[cutover.md](./cutover.md).

## Complexity Tracking

| Item | Why it is justified | Alternative rejected |
|---|---|---|
| New component merging two existing roles | `ffi.rs` is already duplicated (52 shared public names); a third copy is worse. Symmetric endpoints need the union of both crates' verbs usage, and both already have connect+listen+accept. | Extending the initiator in place — leaves the crate name a lie and the responder's accept loop stranded. |
| Symmetric QP usage | The destination connects and sends; the source must reply and push on that same connection. | Two connections per pair — doubles QPs and setups, and the reply still needs a direction. |
| Three credit domains | They bound genuinely different resources (pins, receive buffers, bounce space) and exhaust independently. | One combined counter — would over- or under-restrict two of the three. |
| Bounce reserve carved from the memory tier's pool | RDMA needs a registered buffer and the device read needs SPDK-DMA-able memory; the pool is already both (D15). | Promotion into the memory tier — evicts the source's own working set (D8). A separate dual-registered pool — a whole 1 GiB hugepage for 32 MiB, and a second registration. |
| Internal chunking | A single request can reach ~3.4 GiB; and it doubles as credit-based pacing. | Failing the request on resource shortage — unacceptable for a cache path. |
| Long-lived protection domain, registrations outliving queue pairs | The contract promises the landing pool is registered once, and in-flight requests have already advertised per-entry rkeys; a per-connection PD makes those stale on every reconnect (D17). | The predecessor crates' per-connection PD with re-registration on each connect — fine when you only push from your own pool, wrong when a peer holds your rkeys. |
| Peer-wide abort on deadline | One queue pair per instance pair (D5) means forcing it to ERROR is the only thing that makes a late write impossible, and it necessarily takes the siblings with it (D18). | A queue pair per request, isolating expiry — multiplies QPs by request concurrency, already rejected in D5 for a weaker reason. |

## Amendments (2026-10-15, post-review)

Four review findings were folded in after the task list was first generated, before any
transfer logic was written: **FR-021** (instance-granular peer identity — a host alone is not
an address when several instances run per node), **FR-022** (no phase barriers inside a
request), **FR-023** (deadline cleanup ordering — ERROR, drain, reclaim, reconnect, and the
peer-wide blast radius), a tightened **FR-016**, new criteria **SC-009/010/011**, and
decisions **D16-D19**. D19 is as much a decision *not* to build something: cross-message
pipelining buys ~0.1% because the inter-message bubble is smaller than a single entry's
write, so the only barrier worth engineering away is the block-tier read barrier, whose cost
is measured in milliseconds.

### Protocol versioning (D20)

Settled after the task list was generated: **one** protocol version governs the wire, each end
advertises the range it supports, and the connection runs at `min(a.max, b.max)` — exchanged
**eagerly** once per connection (`OP_HELLO`, answered by a contentless `OP_HELLO_REPLY` that
only balances credits), with a frozen `OP_ERROR` answering any unprocessable request —
replies are never answered; a bad one tears the connection down. The fused opcode is kept as a structural guard on the negotiated version rather
than as the versioning mechanism. (A first draft versioned each kind independently; D20's
*Revision* records why that was dropped — it only ever made sense while discovery was lazy,
and it can represent a v2 request answered by a v1 reply.)

The version is a **validated header field**, not part of the opcode (D21): fusing would mint a
new opcode for every kind on each bump, including unchanged ones, and the handler would mask
the version back off. One connection serves an instance pair in **both** directions (D22),
which requires per-direction context namespaces and credits and a receive-buffer partition so
inbound requests cannot starve awaited replies; simultaneous connect converges by a
deterministic rule on `PeerId` order, resolved at accept (D23). All three reuse one property:
a decision both ends compute identically from values they already hold, rather than an
arbitrator. Both of those messages are permanently unversioned
— a peer that cannot parse version N must still parse them — and bytes 0-15 of the header are
a frozen prefix for the same reason. This supersedes the wire contract's original "no
negotiation" stance; the reasoning, including the two whole-protocol alternatives rejected, is
in research D20. Version-selection logic beyond `min(local, peer)` is deferred until a second
version exists.

## Next Step

Phases 1–10 of `tasks.md` (T001–T083) are complete; Phase 11 (cutover and retirement,
T065–T067) is deferred: T065/T066 designed in cutover.md, T067 a separate design task.

