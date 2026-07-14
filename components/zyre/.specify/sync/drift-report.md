# Spec Drift Report

Generated: 2026-07-14T19:52:47Z
Project: zyre (Safe Rust bindings for the zyre C library)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 (6 artifacts) |
| Functional Requirements Checked | 12 |
| ✓ Aligned (FR) | 12 (100%) |
| ⚠️ Drifted (spec artifacts vs design) | 2 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 0 (material) |
| Success Criteria Verified | 2 of 5 (3 unverified) |

**Headline**: The **code** implements all 12 functional requirements and matches
the current factory / public-fields-config / single-frame design. Of the six
spec artifacts, `spec.md`, `data-model.md`, `contracts/izyre.md`, and
`quickstart.md` are aligned with the code. The remaining drift is confined to
two **stale planning artifacts** — `plan.md` (describes a builder API and a
6-module `event/builder/error/peer` layout that no longer exists) and `tasks.md`
(task bodies still name deleted source files). The previous report's headline
drift (D-1, multi-frame receive truncation) is now **resolved**: the `_multi`
send methods were removed and `data-model.md` no longer claims a `Vec<Vec<u8>>`
variant.

## Detailed Findings

### Spec: 001-zyre-bindings — Zyre Rust Bindings

Code sources: `components/interfaces/src/izyre.rs` (traits + value types),
`components/zyre/src/{lib,node,ffi}.rs`, `components/zyre/build.rs`,
`deps/build_zyre.sh`, `components/zyre/tests/{integration,api_safety}.rs`.

#### Aligned ✓ (functional requirements vs code)

- **FR-001**: No `unsafe` in user code; all unsafe encapsulated → `node.rs`, guarded by `tests/api_safety.rs`.
- **FR-002**: RAII lifecycle → `Drop for ZyreNode` calls `stop()` then `zyre_destroy()` (`node.rs:315`).
- **FR-003**: All 9 event types as an enum → `ZyreEvent` (`izyre.rs:107`), parsed in `parse_event` (`node.rs:354`).
- **FR-004**: Single-frame `&[u8]` whisper/shout, no `_multi` → `node.rs:144`,`162`.
- **FR-005**: `NodeConfig` public fields + `Default` + `#[non_exhaustive]`, validated on create → `izyre.rs:260`, `validate()` in `ZyreNode::new` (`node.rs:29`).
- **FR-006**: UDP beacon + gossip; gossip requires explicit `endpoint` → validation `izyre.rs:327`, `apply_config` `node.rs:76`.
- **FR-007**: Peer introspection → `peers`/`peers_by_group`/`own_groups`/`peer_groups`/`peer_address`/`peer_header_value` (`node.rs:239-312`).
- **FR-008**: Non-blocking `try_recv` via `zpoller` zero-timeout; no crate-spawned threads → `node.rs:196`. *(Minor wording note below.)*
- **FR-009**: Build clones/compiles libzmq v4.3.5, czmq v4.2.1, zyre v2.0.1 → `deps/zyre-build/` (`deps/build_zyre.sh`).
- **FR-010**: `bindgen` FFI in build script, links local libs → `build.rs`.
- **FR-011**: `Send` not `Sync` → `unsafe impl Send for ZyreNode` (`node.rs:21`); `IZyreNode: Send`; verified by `api_safety.rs`.
- **FR-012**: Typed `ZyreError` enum → `izyre.rs:16`.

#### Drifted ⚠️

- **D-1 — `plan.md` describes the obsolete builder API and pre-refactor module layout.** *(major — breadth of stale references; documentation only, no code impact)*
  - `plan.md:8` — "presents a Rust-native API with RAII, typed events, **builder configuration**, and Result-based errors." Actual: `NodeConfig` uses public fields + `Default`, **no builder** (`izyre.rs:260`; the builder was explicitly rejected — `research.md:56`).
  - `plan.md:34` — "6 focused modules (node, **event, builder, error, peer**, ffi)." Actual: the `zyre` crate has 3 files (`lib.rs`, `node.rs`, `ffi.rs`); the value types live in `interfaces/src/izyre.rs`.
  - `plan.md:63-67` — source tree lists `event.rs`, `builder.rs`, `error.rs`, `peer.rs`, none of which exist, and omits that the types now live in the `interfaces` crate.
  - `plan.md:70` — tests tree lists only `integration.rs`; the crate also has `tests/api_safety.rs`.
  - Note: CLAUDE.md directs contributors to "read the current plan at `specs/001-zyre-bindings/plan.md`", so this staleness is actively misleading. The Summary line's "`IZyre` … acts as a factory for `ZyreNode`" is, by contrast, correct.
  - Location: `specs/001-zyre-bindings/plan.md`
  - Severity: **major**

- **D-2 — `tasks.md` task bodies still name deleted source files and the builder.** *(moderate — partially reconciled)*
  - `tasks.md:39-44` carries a correct superseded-design note, and T050 (`shout_multi`/`whisper_multi`) is properly struck (`tasks.md:178`). **However**, individual task lines still reference `src/event.rs`, `src/builder.rs`, `src/error.rs`, `src/peer.rs` and a "builder pattern": e.g. T015 (`:67`), T027/T028 (`:95-96`), T031-T033 (`:102-104`), T043/T046 (`:143`,`:149`), phase goal (`:89`,`:91`), examples (`:218`,`:233`,`:237`).
  - Actual: types are in `interfaces/src/izyre.rs`; config has no builder; the crate files are `lib.rs`/`node.rs`/`ffi.rs`.
  - Location: `specs/001-zyre-bindings/tasks.md`
  - Severity: **moderate**

#### Resolved since last report ✓

- **(was D-1) Multi-frame receive truncation** — RESOLVED. The `_multi` send methods were removed from `node.rs` (per the Session 2026-07-09 clarification, `spec.md:129`), and `data-model.md:54` now states the payload is a single frame "bounded only by memory; there is no multi-frame representation." Send/receive are symmetric again; SC-004 no longer weakened.
- **(was D-2/D-3) `data-model.md` builder + accessor gaps** — RESOLVED. `data-model.md:63-66` documents the public-fields/no-builder `NodeConfig`; `:57` documents the `peer()/peer_name()/group()` accessors (so they are no longer "unspecced code").

#### Minor observations (not counted as requirement drift)

- **FR-008 wording**: the spec calls `recv()` a "thin wrapper over `zyre_recv`", but the code uses the higher-level `zyre_event_new`/`zyre_event_destroy` (`node.rs:180-191`) — which internally calls `zyre_recv` and yields a typed event. Functionally equivalent; consider rewording FR-008 to "wraps `zyre_event_new`".
- **`ZyreEvent::Stop` reachability**: `recv`/`try_recv` return `NotStarted` once `stop()` flips `started` (`node.rs:114-119` vs `:180-183`), so a STOP event can never be surfaced. Representable (SC-004 holds) but effectively undeliverable. Document as internal-only or drain before stopping.
- **`research.md:84`**: "`InvalidConfig(String)` — builder validation failure" — stale phrasing ("builder"); should read "config validation failure". Minor.
- **Fire-and-forget edge case** (`spec.md:81`): whisper/shout to a departed UUID is untested; `whisper`/`shout` return `SendFailed` on non-zero rc (`node.rs:173`,`:155`). `zyre_whisper` returns 0 for unknown peers so behavior likely matches, but no test locks the contract.

#### Success Criteria — Verification Status

- **SC-001** (round-trip < 2 s on localhost): ⚠️ Unverified — `integration.rs` sleeps 500 ms then polls a 5 s deadline; the 2 s bound is not asserted.
- **SC-002** (zero `unsafe` in public API): ✓ Verified — safe public surface; `tests/api_safety.rs` guards it.
- **SC-003** (clean build < 5 min): ⚠️ Unverified — no automated measurement or CI gate.
- **SC-004** (all event types, no info loss): ✓ Verified — 9 event types lossless; single-frame payload is unbounded (multi-frame gap resolved).
- **SC-005** (Miri/valgrind clean): ⚠️ Unverified — no Miri/valgrind harness. Miri cannot cross the FFI boundary; a valgrind job over the serialized integration tests is the realistic vehicle.

### Unspecced Code 🆕

None material. The `ZyreEvent` accessors, `PeerId` `Display`/`From` conversions,
and `GossipConfig::bind`/`connect` constructors are all now documented in
`data-model.md` / `izyre.rs` and consistent with US2 (idiomatic Rust API).

## Inter-Spec Conflicts

Within 001-zyre-bindings, `plan.md` and (in its task bodies) `tasks.md` still
describe the pre-refactor builder / per-type-file design, contradicting
`spec.md`, `data-model.md`, and `contracts/izyre.md`, which describe the current
factory / public-fields-config design (commit `b45418d` and the Session
2026-07-09 clarifications). No conflicts with other components' specs.

## Recommendations

1. **Refresh `plan.md` (D-1)** — the highest-value fix, since CLAUDE.md points contributors here. Drop "builder configuration"; correct the module count/layout to `lib.rs`/`node.rs`/`ffi.rs` with value types in `interfaces/src/izyre.rs`; fix the source tree (remove `event.rs`/`builder.rs`/`error.rs`/`peer.rs`, add `api_safety.rs` to the tests tree).
2. **Reconcile `tasks.md` bodies (D-2)** — rewrite the task lines that name `event.rs`/`builder.rs`/`error.rs`/`peer.rs` and "builder pattern" to the factory/public-fields design, matching the note already at the top of the file.
3. **Close SC verification gaps** — tighten SC-001 into a timed assertion (or relax its wording); add a valgrind CI job for SC-005 (note Miri can't cover FFI); record a clean-build timing for SC-003.
4. **Minor cleanups** — reword FR-008 (`zyre_event_new`), fix `research.md:84` ("config validation failure"), document/close the `ZyreEvent::Stop` reachability gap, and add a whisper-to-departed-UUID test to lock the fire-and-forget contract.
