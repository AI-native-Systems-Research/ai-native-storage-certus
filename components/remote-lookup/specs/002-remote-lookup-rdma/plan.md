# Implementation Plan: Remote Lookup over Zyre + RDMA

**Branch**: `002-remote-lookup-rdma` | **Date**: 2026-07-13 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/002-remote-lookup-rdma/spec.md`

## Summary

Turn the `remote-lookup` placeholder into the real cache-fill client/server: an actor that owns a
zyre node, an RDMA **responder** (its registrar/accept side), and an RDMA **initiator** (to serve
peers). As a client it SHOUTs a KEY_QUERY for `(key, size)` misses, greedily whispers RDMA_REQUESTs
to peers reporting memory hits, and publishes private landing slots to the memory tier on
RDMA_STATUS(Success) (publish-on-success). As a
server it answers KEY_QUERYs and delegates RDMA_REQUESTs to the initiator. All data movement is
one-sided RDMA into the requester's pre-registered pool; the whisper status vector is the completion
signal. Scope covers memory-tier hits (US1–3, 6, 7), disk fallback (US4, via the existing
`IDispatcher::promote_to_memory_tier` — research D3 resolved), and multi-round retry (US5);
delivery is incremental (memory path first) but US4/US5 are in scope. See [research.md](./research.md),
[data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md).

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**: `component-framework`/`component-core`/`component-macros`; `interfaces`
(traits only — `IRemoteLookup`, `IZyre`/`IZyreNode`, `IDispatchMap`, `IMemoryTier`, `IDispatcher`,
`IRemoteLookupRdmaInitiator`, `IRemoteLookupRdmaResponder(+Admin)`, `ILogger`). No `serde` (hand-rolled
wire codec); no impl-crate deps. **SPDK gating (research Decision 10 / D4)**: `IDispatchMap` and
`IDispatcher` are `spdk`-gated in `interfaces`, so remote-lookup enables `interfaces/spdk` (which
pulls `spdk-sys`) and is therefore **removed from `default-members`** and built explicitly
(`cargo build -p remote-lookup`). The crate still touches no SPDK *type* (opaque `*mut u8` only); the
SPDK-orthogonal end state (ungate the trait defs, drop the feature, rejoin `default-members`) is
tracked as future work.

**Storage**: none on-disk. Landing slots are private reservations inside the responder-registered
DRAM pool, published to the memory tier only on RDMA success; no per-request `ibv_reg_mr`.

**Testing**: `cargo test -p remote-lookup` (unit + doc + two-instance protocol tests over mock zyre/RDMA
seams, no hardware); Criterion bench for the correlation/greedy-dispatch path; hardware two-instance
loopback under `--features rdma` (`#[ignore]`).

**Target Platform**: Linux (RHEL 9 / Fedora); RoCE/IB fabric for the real path.

**Project Type**: single Rust component crate (actor).

**Performance Goals**: added latency dominated by the RDMA transfer, sub-ms for typical entries on
RoCE/IB (SC-001); greedy Phase-1 dispatch with no intervening poll cycle (SC-003); actor poll tick
≈0.5–1 ms when idle.

**Constraints**: control-plane only (no data touch, no GPU memory); lossless memory-safety on peer
departure (teardown-before-reclaim, SC-005); default op deadline 50 ms (SC-002).

**Scale/Scope**: one instance per NUMA domain; groups of up to ~tens of peers; batches split at
`max_keys_per_query` (256).

## Constitution Check

*GATE: re-checked after Phase 1 design — PASS.*

| Principle | Status | How the design satisfies it |
|-----------|--------|------------------------------|
| I. Interface-Only Exposure | PASS | Only `IRemoteLookup` is provided; all collaboration is via interface receptacles; `define_component!`/`define_interface!`. New wire types are crate-internal, not at the boundary. |
| II. Comprehensive Unit Testing | PASS | Mock zyre + RDMA seams make the full protocol, single-flight, timeout, and peer-exit paths unit-testable without hardware. |
| III. Documentation Tests | PASS | `IRemoteLookup` doc examples compile under `cargo test --doc`; `cargo doc --no-deps` warning-free. |
| IV. Performance Testing | PASS | Criterion bench for the correlation/greedy-dispatch hot path (SC-003). |
| V. Code Correctness | PASS | `clippy -D warnings`; `Result`-returning fallible APIs; `// SAFETY:` on any raw-pointer slot handling. |
| VI. Maintainability | PASS | Actor keeps state single-threaded; hand-rolled codec avoids speculative deps; builds independently of impl crates. |
| VII. Linux Commitment | PASS | Linux-only; zyre/RDMA are Linux facilities. |

No violations → Complexity Tracking empty.

## Project Structure

### Documentation (this feature)

```text
specs/002-remote-lookup-rdma/
├── plan.md          # this file
├── research.md      # Phase 0: decisions + cross-component dependencies (D1–D3)
├── data-model.md    # Phase 1: Operation/PeerReply/Placeholder/WireMessage + component decl
├── quickstart.md    # Phase 1: wiring + CI/hardware test recipes
├── contracts/
│   ├── iremote_lookup.md    # provided-interface contract
│   └── wire-protocol.md     # peer-to-peer zyre protocol (v1)
└── tasks.md         # Phase 2 (/speckit-tasks — not created here)
```

### Source Code (`components/remote-lookup/`)

```text
components/remote-lookup/
├── Cargo.toml               # add rdma feature (forwards to initiator/responder rdma); no spdk
├── src/
│   ├── lib.rs               # define_component!, IRemoteLookup impl, actor spawn/lifecycle
│   ├── actor.rs             # poll loop: submissions + zyre events + responder events + deadlines
│   ├── operation.rs         # Operation state machine, completion criteria, single-flight
│   ├── wire.rs              # hand-rolled encode/decode of the v1 framed messages
│   ├── server.rs            # server role: answer KEY_QUERY, serve RDMA_REQUEST via initiator
│   └── seams.rs             # test seams: mock IZyre/IZyreNode + mock initiator/responder wrappers
├── benches/
│   └── correlation.rs       # SC-003 greedy-dispatch / correlation micro-benchmark
└── tests/
    └── two_instance.rs      # in-process two-node protocol tests (mock seams) + #[ignore] hardware
```

**Structure Decision**: Single component crate. The actor loop (`actor.rs`) is the only thread that
mutates operation state; `operation.rs` holds the per-`op_id` state machine; `wire.rs` is a pure,
independently-tested codec; `server.rs` is the peer-serving half. `lib.rs` stays thin (component
declaration + `IRemoteLookup` glue + lifecycle). The crate enables `interfaces/spdk` (for the
`IDispatchMap`/`IDispatcher` trait defs — research Decision 10) so it is built explicitly
(`cargo build -p remote-lookup`), **not** via the SPDK-free default build; the `rdma` feature only
forwards to the initiator/responder crates for the hardware test.

## Complexity Tracking

No constitution violations — no entries.

## Cross-component prerequisites (from research)

- **D1** — dispatch-map placeholder abort/notify: **NOT NEEDED.** Publish-on-success (research
  Decision 5) keeps unfilled slots private to remote-lookup, so dispatch-map only ever holds a
  fully-filled entry and the failure path never removes one — no dispatch-map change is made.
- **D2** — initiator: stamp the local zyre `PeerId` into the `rdma_cm` connect `private_data`.
  **Done** on this branch (commit `e77c4a5`); satisfies identity correlation + teardown (FR-014).
- **D3** — serving-node disk→memory promotion. **RESOLVED**: use the existing
  `IDispatcher::promote_to_memory_tier` (idispatcher.rs:657, implemented by dispatcher +
  dispatcher-p2p). remote-lookup adds a `dispatcher: IDispatcher` receptacle; no interface or
  dispatcher code change. Mainline must `disconnect()` one side of the `dispatcher-p2p ⇄
  remote-lookup` Arc cycle at teardown (research Decision 7).

- **Interface prerequisite (config)** — add `IRemoteLookup::initialize(LookupConfig)` and a public
  `LookupConfig` (derives `Default`) to the `interfaces` crate (mirrors
  `IDispatcher::initialize(DispatcherConfig)`; FR-022). Only remote-lookup's own impl exists, so the
  blast radius is contained. Lands as its own `interfaces` commit before the remote-lookup impl.
- **Integration (certus-server-yaml)** — add an `init_remote_lookup` hook (`src/hooks.rs`) that
  builds `LookupConfig { ..Default::default() }` and calls `initialize`, and wire `remote_lookup`'s
  `init_hook`/`init_order` (+ the `dispatcher` receptacle and the teardown `disconnect`) in
  `profiles/full-remote.yaml`. App-level; keeps config YAML-robust.

- **D4 (SPDK gating of consumed traits)** — `IDispatchMap`/`LookupResult`/`IDispatcher` are
  `spdk`-gated in `interfaces`. **Short-term resolution (research Decision 10)**: remote-lookup
  enables `interfaces/spdk` and leaves `default-members` (a Cargo-manifest + workspace edit, not a
  shared-crate code change). **Future work**: ungate those pure-Rust trait defs (as `IMemoryTier`
  was in `db7f70a`) so remote-lookup can drop the feature and rejoin `default-members`.

**No outstanding cross-component code prerequisite for the protocol itself** (D1 dropped, D2 done,
D3 satisfied by an existing method). The `interfaces` changes are (1) the `initialize`/`LookupConfig`
addition above (done, `225bf5b`) and (2) enabling the pre-existing `spdk` feature from remote-lookup
(D4 short-term — no `interfaces` *source* change). US4 and US5 are in scope, not deferred.
