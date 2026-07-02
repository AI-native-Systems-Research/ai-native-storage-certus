# Spec Drift Report

Generated: 2026-07-09
Project: zyre (Rust bindings for zyre C library)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Functional Requirements Checked | 12 |
| ✓ Aligned | 12 (100%) |
| ⚠️ Drifted | 3 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 1 (minor) |
| Success Criteria Unverified | 3 of 5 |

**Headline**: The `zyre` implementation matches the current **factory design** in
`spec.md`, `data-model.md`, and `contracts/izyre.md` (the design introduced by
refactor `b45418d`, "make IZyre a node factory returning IZyreNode"). All 12
functional requirements are aligned. The drift is concentrated in **stale spec
artifacts** that were not updated during that refactor (`tasks.md`, the previous
`drift-report.md`) and in one **data-model invariant** describing a multi-frame
receive variant that was never implemented.

## Detailed Findings

### Spec: 001-zyre-bindings — Zyre Rust Bindings

#### Aligned ✓

Value types and the `IZyre`/`IZyreNode` traits live in
`components/interfaces/src/izyre.rs`; the concrete `ZyreNode` (crate-private FFI
wrapper) lives in `components/zyre/src/node.rs`. The `zyre` crate re-exports the
types from `lib.rs`.

- **FR-001**: Safe bindings, no `unsafe` in user code → `interfaces/src/izyre.rs` (safe public API), `src/node.rs` (unsafe encapsulated internally), `tests/api_safety.rs`
- **FR-002**: RAII lifecycle (drop stops + frees) → `src/node.rs:355` (`Drop for ZyreNode` calls `stop()` then `zyre_destroy()`)
- **FR-003**: All 9 event types as a Rust enum → `interfaces/src/izyre.rs:107` (`ZyreEvent` with Enter/Exit/Evasive/Silent/Join/Leave/Whisper/Shout/Stop)
- **FR-004**: Single-frame `&[u8]` primary + `_multi` variants → `src/node.rs:144` (`shout`/`whisper`), `src/node.rs:239` (`shout_multi`/`whisper_multi`) *(send side; see Drift D-1 for receive side)*
- **FR-005**: `NodeConfig` public fields + `Default` + `#[non_exhaustive]`, validated on create → `interfaces/src/izyre.rs:260` (`NodeConfig`), validation invoked in `src/node.rs:29` (`ZyreNode::new` → `config.validate()`)
- **FR-006**: UDP beacon + gossip; gossip requires explicit node endpoint → `src/node.rs:76` (`apply_config` gossip branch), `interfaces/src/izyre.rs:327` (validation requires `endpoint` when gossip set)
- **FR-007**: Peer introspection → `src/node.rs:279-352` (`peers`, `peers_by_group`, `own_groups`, `peer_groups`, `peer_address`, `peer_header_value`)
- **FR-008**: Blocking `recv()` + non-blocking `try_recv()`, no background threads → `src/node.rs:180`, `src/node.rs:196` (uses `zpoller_wait(…, 0)`; no threads spawned by the crate)
- **FR-009**: Build clones/compiles into `deps/zyre-build/` → `deps/build_zyre.sh` (pins libzmq v4.3.5, czmq v4.2.1, zyre v2.0.1), `deps/install_zyre_deps.sh`
- **FR-010**: Bindgen in build script → `build.rs:45` (linking `zyre`/`czmq`/`zmq` from `deps/zyre-build/`)
- **FR-011**: `Send` but not `Sync` → `src/node.rs:21` (`unsafe impl Send`, no `Sync` impl), `tests/api_safety.rs`
- **FR-012**: Typed `ZyreError` enum → `interfaces/src/izyre.rs:16`

#### Drifted ⚠️

- **D-1 — Multi-frame receive is lossy; `data-model.md` documents a variant that does not exist.**
  - Spec text: `data-model.md:55` — *"`message` in Whisper/Shout is the first frame payload (single-frame API). Multi-frame variant carries `Vec<Vec<u8>>`."*
  - Actual: `ZyreEvent::Whisper`/`ZyreEvent::Shout` carry a single `message: Vec<u8>` only (`interfaces/src/izyre.rs:134-146`); there is no `Vec<Vec<u8>>` variant anywhere. `parse_message` reads only `zmsg_first` (`src/node.rs:503-520`), so a message sent via `shout_multi`/`whisper_multi` (>1 frame) is **truncated to its first frame on receive**.
  - Location: `interfaces/src/izyre.rs:134`, `src/node.rs:503`
  - Impact: also weakens **SC-004** ("no loss of information compared to the C API") — event *types* are lossless, but multi-frame payloads are not.
  - Severity: **moderate**

- **D-2 — `tasks.md` describes the obsolete pre-refactor architecture.**
  - Spec text: `tasks.md` T008–T013, T048 reference separate source files (`src/error.rs`, `src/peer.rs`, `src/event.rs`, `src/builder.rs`), a **"builder API" / `NodeConfigBuilder`**, and *"circular dep prevents `create_node` here; consumers use `zyre::ZyreNode::new()` directly"* with `IZyre` exposing only `ping()`.
  - Actual: value types + traits are in `interfaces/src/izyre.rs` (single file, no `error.rs`/`peer.rs`/`event.rs`/`builder.rs`); config uses **public fields + `Default`** (no builder); `IZyre` **is** a factory with `create_node` (`lib.rs:51`); `ZyreNode::new` is **crate-private** so callers cannot bypass the interface.
  - Location: `tasks.md` (T008–T013, T048–T049)
  - Impact: superseded by the clarification recorded in `spec.md:127` and commit `b45418d`. `spec.md`, `data-model.md`, and `contracts/izyre.md` were updated; `tasks.md` was not. Documentation/traceability only — no code impact.
  - Severity: **major** (breadth of stale references)

- **D-3 — Previous `drift-report.md` is itself stale (self-correcting).**
  - The prior report (2026-07-01) cited `src/builder.rs`/`src/event.rs` and described FR-005 as *"Builder pattern for configuration → NodeConfigBuilder"*. Those files and the builder no longer exist. Superseded by this regeneration.
  - Severity: **minor** (resolved by this run)

#### Not Implemented ✗

(none — all 12 functional requirements have corresponding implementation)

#### Success Criteria — Verification Status

- **SC-001** (round-trip < 2s on localhost): ⚠️ Unverified — `tests/integration.rs:68` uses a 5s deadline and does not assert the 2s bound.
- **SC-002** (zero `unsafe` in public API): ✓ Verified — public surface is safe; `tests/api_safety.rs` guards it.
- **SC-003** (clean build < 5 min): ⚠️ Unverified — `tasks.md` T042/T055 (end-to-end build check) remain unchecked.
- **SC-004** (all event types, no information loss): ⚠️ Partial — 9 event types represented losslessly, but multi-frame payloads are truncated on receive (see D-1).
- **SC-005** (Miri/valgrind clean): ⚠️ Unverified — no evidence the suite has been run under Miri/valgrind.

#### Edge-Case Note (minor)

`spec.md:81` states a message sent to a departed UUID "completes silently
(fire-and-forget)". `whisper`/`shout` return `ZyreError::SendFailed` when the C
call returns non-zero (`src/node.rs:173`, `:155`). In practice `zyre_whisper`
returns 0 for an unknown peer, so behavior likely matches — but the contract is
not asserted by any test. Severity: minor.

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `ZyreEvent` accessor helpers `peer()` / `peer_name()` / `group()` | `interfaces/src/izyre.rs:151-191` | ~40 | 001-zyre-bindings (document as convenience API on the event enum) |

These are ergonomic accessors not called out in the spec's Key Entities. Low
risk; recommend a one-line mention in `data-model.md` rather than a new spec.

## Inter-Spec Conflicts

Within 001-zyre-bindings, `tasks.md` (pre-refactor: builder API, `ping()`-only
`IZyre`, per-type source files) contradicts `spec.md`/`data-model.md`/
`contracts/izyre.md` (post-refactor: factory `create_node`, public-fields config,
types in `interfaces`). See D-2. No conflicts with other components' specs.

## Recommendations

1. **Resolve D-1 (multi-frame receive)** — decide the intended contract and align both sides:
   - If multi-frame receive is in scope: add a multi-frame representation (e.g. `Vec<Vec<u8>>` on Whisper/Shout, or a `frames()` accessor) and have `parse_message` collect all frames via `zmsg_next`.
   - If out of scope: correct `data-model.md:55` to drop the `Vec<Vec<u8>>` claim and state that receive surfaces the first frame only, and note the limitation next to `shout_multi`/`whisper_multi`.
2. **Refresh `tasks.md` (D-2)** — rewrite T008–T013/T048–T049 to the factory design: types in `interfaces/src/izyre.rs`, `NodeConfig` public-fields + `Default`, `IZyre::create_node`, crate-private `ZyreNode`. Update file paths (`node.rs`/`lib.rs`/`ffi.rs`) throughout.
3. **Close SC verification gaps** — run and record the unchecked tasks: T035/T036 (`cargo doc`/`clippy`), T042/T055 (clean build < 5 min → SC-003), and a Miri/valgrind pass (SC-005). Tighten the SC-001 assertion or add a timed round-trip test.
4. **Document the edge case** — add a test (or a doc note) covering whisper-to-departed-UUID to lock in the fire-and-forget contract (`spec.md:81`).
5. **Note the accessor helpers** in `data-model.md` to eliminate the minor unspecced-code finding.
