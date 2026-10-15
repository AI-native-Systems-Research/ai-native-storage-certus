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
actors, channels), `components/interfaces` (`ILogger`, `IDispatchMap`, and a
block-device interface under the `spdk` feature — see research D13), `anyhow`. rdma-core (libibverbs/librdmacm) only under the `rdma`
feature, via a hand-written FFI layer and a C shim built by `cc`/`pkg-config`.

**Storage**: certus memory tier (DPDK hugepages) for landing slots and for memory-tier
serving; certus block tier via SPDK for block-tier serving, staged through a registered
bounce-buffer pool.

**Testing**: `cargo test` over an in-process transport seam (no rdma-core, no SPDK, no
zyre); `#[ignore]`d hardware loopback tests under the `rdma` feature, mirroring the existing
T034 pattern; Criterion benchmarks for the per-entry push path (CLAUDE.md requires them for
performance-sensitive code).

**Target Platform**: Linux, x86-64, RoCE-capable NIC. Two-node minimum for the real path;
single-host RDMA loopback for CI-adjacent verification.

**Project Type**: library component in `components/`, following the established component
model (receptacles + actor-on-dedicated-thread).

**Performance Goals**: saturate the available link on memory-tier-served transfers; per-entry
RDMA posting must not serialise behind device reads (hence the bounce-buffer decoupling).
No latency target is set at this stage — the research question is whether explicit transfer
beats recompute, and the measurement belongs to a later benchmark task.

**Constraints**: concurrently held read pins on a source must never exceed the configured
send-queue credit limit, regardless of request length (SC-004). Serving a block-tier entry
must leave the source's memory-tier occupancy and LRU order unchanged (SC-005). Default
build must succeed with neither rdma-core nor SPDK present (SC-003).

**Scale/Scope**: one endpoint per certus instance; a handful of peers per node in the
current rigs (2 nodes × 2 NUMA instances). Key runs up to a full model context — ~1,536
blocks / ~3.4 GiB at 2.25 MiB per block for Qwen3-8B at 24k tokens — which is why internal
chunking is mandatory rather than optional.

## Constitution Check

`.specify/memory/constitution.md` is an **unfilled template** (all placeholder sections), so
there is no ratified constitution to gate against. Checking instead against the repository's
de facto conventions in `CLAUDE.md`:

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
    ├── connection.rs# symmetric connection table, per-peer state machine, credits,
    │                #   wr_id slab, transport seam (real + mock)
    ├── message.rs   # request/reply framing, context IDs, message credits
    ├── serve.rs     # source side: per-entry lookup/pin/push, outcome accumulation
    ├── bounce.rs    # registered buffer pool for block-tier serving
    └── telemetry.rs # feature-gated metrics
```

**Structure Decision**: a single component rather than extending either existing crate.
`ffi.rs` is declared **once** here, which is the point of SC-008 — the two existing crates
currently declare 28 identical public items between them. `connection.rs` carries the
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
Bounce pool with dual registration, device read, early pin release, `hit_block_tier`
outcome. Gate: SC-005. Requires OQ3 resolved.

**Phase 5 — bounds and chunking (FR-014, FR-015).**
Three named credit domains; internal chunking on exhaustion of any. Gate: SC-007.

**Phase 6 — fail-loud deadline (FR-016).**
Detection plus the failing action; degrading action left unimplemented behind the flag.

**Phase 7 (later, separate change) — cutover.**
Re-point `remote-lookup` at this component; delete
`remote-lookup-rdma-initiator` and `remote-lookup-rdma-responder`. Explicitly **out of
scope** for FR-020 and not required for any P1 success criterion.

## Complexity Tracking

| Item | Why it is justified | Alternative rejected |
|---|---|---|
| New component merging two existing roles | `ffi.rs` is already duplicated (28 shared public items); a third copy is worse. Symmetric endpoints need the union of both crates' verbs usage, and both already have connect+listen+accept. | Extending the initiator in place — leaves the crate name a lie and the responder's accept loop stranded. |
| Symmetric QP usage | The destination connects and sends; the source must reply and push on that same connection. | Two connections per pair — doubles QPs and setups, and the reply still needs a direction. |
| Three credit domains | They bound genuinely different resources (pins, receive buffers, bounce buffers) and exhaust independently. | One combined counter — would over- or under-restrict two of the three. |
| Bounce pool with dual registration | RDMA needs `ibv_reg_mr`; the device read needs SPDK-DMA-able memory. Both are required of the same buffer. | Promotion into the memory tier — evicts the source's own working set (D8). |
| Internal chunking | A single request can reach ~3.4 GiB; and it doubles as credit-based pacing. | Failing the request on resource shortage — unacceptable for a cache path. |

## Next Step

OQ1 and OQ2 are resolved as research **D13** (receptacle set: look up through
`IDispatchMap`, not `IMemoryTier`, because only it gives tri-state residency plus an atomic
pin) and **D14** (feature matrix: `default` / `rdma` / `rdma,spdk`, with a no-`spdk` build
reporting block-tier entries as `miss`). Together they fix the `Cargo.toml` and the
`define_component!` receptacle block, so Phase 1 is unblocked. Only **OQ3** remains, and
only Phase 4 needs it.

Write `contracts/itransfer_endpoint.md` and the request/reply wire format, then generate
`tasks.md` via `.specify/scripts/bash/setup-tasks.sh` with
`SPECIFY_FEATURE_DIRECTORY` set, and write `contracts/itransfer_endpoint.md` plus the wire
format alongside.
