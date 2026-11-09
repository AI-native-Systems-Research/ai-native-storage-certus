# Implementation Plan: RDMA Lookup Responder

**Branch**: `001-rdma-lookup-responder` | **Date**: 2026-07-10 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/001-rdma-lookup-responder/spec.md`

## Summary

The RDMA lookup responder is the **passive accept side** of a remote RDMA lookup,
owned by the *requesting* Certus instance. It is an actor: a dedicated thread runs
an `rdma_cm` accept loop that binds an ephemeral port on the local RoCE IPv4,
accepts inbound connections from serving peers, and keys a per-node connection
table by the zyre `PeerId` the initiator stamps into the connect `private_data`.
Serving peers RDMA-**write** values one-sidedly into the pre-registered memory
tier, so the responder's CPU never touches value bytes — it manages **connections
only**.

The load-bearing behavior is **teardown-before-reclaim**: on `Disconnect { node }`
from `remote-lookup`, the responder drives that peer's RC queue pair into the
`ERROR` state (so late one-sided writes are NAKed and cannot land) **before**
emitting `DisconnectAck { node }`; `remote-lookup` blocks on that ack before
reclaiming the peer's locked landing slots. The QP→ERROR transition is asserted
(fail-stop on failure); the ack is an unconditional guarantee.

**Technical approach**: mirror the sibling initiator
(`remote-lookup-rdma-initiator`) crate-for-crate — a testable mock CM seam
(`CmListener`/`CmConnection` traits) analogous to its `RdmaTransport`/`RdmaConn`,
raw rdma-core FFI + a C wrapper behind `build.rs`, a feature-gated ZST telemetry
collector, a Criterion two-run overhead benchmark, and a hardware-gated
`#[ignore]` loopback test. Ship the **skeleton over the mock seam first**
(actor, control channel, lifecycle, `PeerId`-keyed `Active → Draining → Dead`
state machine, teardown ordering); the production `rdma_cm` accept loop
(`epoll`, real bind/listen/`get_src_port`, `private_data` read, QP teardown,
NUMA pin) is verified on RDMA hardware in a follow-up.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75 (workspace-inherited).

**Primary Dependencies**: `component-framework`, `component-core` (actor threads,
SPSC channels, `numa::CpuSet`/`set_thread_affinity`), `component-macros`
(`define_component!`/`define_interface!`), `interfaces` (the
`IRemoteLookupRdmaResponder` / `…Admin` traits and value types; the Admin trait
carries `set_bind_ip(ip)` alongside `set_actor_cpu` per FR-002a).
Hardware path links **rdma-core** (`libibverbs`, `librdmacm`) via `pkg-config`
and a `cc`-compiled `wrapper.c`, matching the initiator. `criterion` (dev) for
the overhead benchmark.

**Storage**: no on-disk state. The responder registers the whole DRAM memory-tier
pool once with `ibv_reg_mr` (`REMOTE_WRITE`) at `initialize()` — read via the
`memory_tier` receptacle's `IMemoryTier::pool_info()` — and deregisters it on
shutdown; there is no per-request registration.

**Testing**: `cargo test -p remote-lookup-rdma-responder` (unit tests over the
mock CM seam, no hardware, `rdma` feature off); `--features telemetry` for
telemetry-wiring tests; `--features rdma` to compile the real `rdma_cm` path
(required for the hardware-gated `#[ignore]` loopback tests); `cargo bench` for
SC-006.

**Target Platform**: Linux (RHEL/Fedora), RDMA-capable NIC (RoCE/IB) on an
isolated, trusted fabric. Not a workspace default member (links rdma-core);
built/tested explicitly with `-p remote-lookup-rdma-responder`. The `rdma`
Cargo feature gates the entire real `rdma_cm`/`ibv_reg_mr` path (see spec.md
"Build & Feature Flags"); without it, `initialize()` returns `Bind` as a
build-configuration failure.

**Project Type**: Single Rust component crate within the Certus COM-style
component workspace.

**Performance Goals**: Telemetry overhead < 5% versus the feature-off build
(SC-006), measured by the two-run Criterion workflow. Prompt command servicing
(SC-003) is validated **structurally**, not by a numeric bound: over the mock CM
seam, an enqueued `Disconnect` is acked on an event-driven wake with no
intervening connection event and no poll cycle. No data-path throughput goal —
there is no data path here.

**Constraints**: The QP→ERROR transition MUST be ordered before `DisconnectAck`
and is asserted (fail-stop on a fatal HCA/programming fault). The responder MUST
NOT touch value bytes and MUST NOT `ibv_reg_mr`. The device MUST NOT be pinned by
name — binding by IP implies the NIC/NUMA path. Diagnostics via an optional
`ILogger` receptacle; a missing logger is never an error.

**Scale/Scope**: One connection entry per remote peer (a handful to low hundreds
of zyre nodes). One actor thread per instance; one instance per NUMA domain,
co-resident instances may share one NIC (hence the ephemeral port).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

The project constitution at `.specify/memory/constitution.md` is an **unfilled
template** (placeholder principles only), so it defines no concrete gates. In its
absence the plan is gated against the repository's de-facto rules in
`CLAUDE.md` and the conventions of the sibling initiator component:

| Gate (from CLAUDE.md / sibling conventions)                     | Status | Notes |
|-----------------------------------------------------------------|--------|-------|
| `rustfmt` default formatting                                    | PASS   | No formatting deviations planned. |
| `clippy -D warnings`                                            | PASS   | ZST no-op telemetry uses `#[allow(clippy::unused_self)]` exactly as the initiator does. |
| Public APIs documented; `cargo doc --no-deps` warning-free      | PASS   | Interface is already documented in `interfaces`; new public items (telemetry, seam traits exposed for benches) get doc comments + examples. |
| Performance-sensitive code has Criterion benchmarks             | PASS   | SC-006 telemetry-overhead benchmark under `benches/`, mirroring `push_telemetry`. |
| `unsafe` requires `// SAFETY:` justification                    | PASS   | All rdma-core FFI calls carry SAFETY comments, matching `rdma.rs`. |
| Component accessed only through its interface (no struct leak)  | PASS   | `remote-lookup` drives it solely via `IRemoteLookupRdmaResponder(+Admin)` + `ControlChannel`; verifiable with `component-check-leakage`. |
| Not a default workspace member (links rdma-core)                | PASS   | Excluded from `default-members`, like the initiator and SPDK crates. |

**Result**: No violations; Complexity Tracking is empty. (If the constitution is
later filled in, re-run this gate.)

## Project Structure

### Documentation (this feature)

```text
specs/001-rdma-lookup-responder/
├── plan.md              # This file (/speckit-plan output)
├── research.md          # Phase 0 output — design decisions
├── data-model.md        # Phase 1 output — entities & state machine
├── quickstart.md        # Phase 1 output — validation guide
├── contracts/
│   └── responder-control-interface.md   # Phase 1 output — interface + protocol contract
├── checklists/          # (pre-existing)
└── tasks.md             # Phase 2 output (/speckit-tasks — NOT created here)
```

### Source Code (repository root)

The crate mirrors the sibling initiator `components/remote-lookup-rdma-initiator/`
file-for-file so the two components stay consistent:

```text
components/remote-lookup-rdma-responder/
├── Cargo.toml           # add [features] telemetry, build-deps (cc, pkg-config),
│                        #   dev-dep criterion, [[bench]] connection_telemetry
├── build.rs             # link libibverbs + librdmacm, compile wrapper.c (mirror)
├── README.md            # component overview (impl phase)
├── info/
│   └── DESIGN.md        # design notes referenced by lib.rs (impl phase)
├── src/
│   ├── lib.rs           # define_component!, actor lifecycle, control channel,
│   │                    #   accept-loop wiring (skeleton already present)
│   ├── ffi.rs           # raw rdma-core bindings; ADDS responder-side calls:
│   │                    #   rdma_bind_addr, rdma_listen, rdma_get_src_port,
│   │                    #   rdma_accept, rdma_reject, ibv_modify_qp (QP→ERROR)
│   ├── rdma.rs          # RealCmSeam: bind/listen/get_src_port, accept loop,
│   │                    #   private_data read, QP→ERROR + destroy
│   ├── connection.rs    # CmListener/CmConnection seam + MockCmSeam;
│   │                    #   PeerId-keyed ConnectionTable + Active→Draining→Dead
│   ├── telemetry.rs     # feature-gated ZST TelemetryCollector (mirror)
│   ├── wrapper.c        # C shims for inline ibverbs (e.g. ibv_modify_qp helper)
│   └── loopback_test.rs # hardware-gated #[ignore] real-accept test
└── benches/
    └── connection_telemetry.rs   # SC-006 two-run overhead benchmark (mirror)
```

Also touched outside the crate:
`components/interfaces/src/iremote_lookup_rdma_responder.rs` (interface — gained
`set_bind_ip(ip)` on the Admin trait per FR-002a),
`components/interfaces/src/lib.rs` (re-exports, unchanged — `set_bind_ip` is a
method on the already-exported trait), and workspace `Cargo.toml` membership. The
skeleton `src/lib.rs` implements `set_bind_ip` (stored in a `bind_ip` field) and
`initialize()` now returns `Bind` when no IP was supplied.

**Structure Decision**: Single component crate, laid out identically to
`remote-lookup-rdma-initiator`. The one intentional divergence is the seam
direction — the initiator abstracts an *outbound* connector
(`RdmaTransport::connect`), whereas the responder abstracts an *inbound* listener
(`CmListener` yielding connect/teardown events + `CmConnection` carrying the QP);
both exist so the connection table and telemetry are unit-testable and
benchmarkable without RDMA hardware.

## Complexity Tracking

> No constitution violations — section intentionally empty.
