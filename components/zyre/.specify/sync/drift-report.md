# Spec Drift Report — zyre

Generated: 2026-07-22T22:33:44Z
Project: `components/zyre` (safe Rust bindings for the [zyre](https://github.com/zeromq/zyre) C library — zero-configuration LAN peer discovery and group messaging)

## Summary

| Category | Count |
|---|---|
| Specs analyzed | 1 (`001-zyre-bindings`) |
| Requirements checked (FR + SC) | 17 (12 FR + 5 SC) |
| Aligned | 17 |
| Drifted | 0 |
| Not implemented | 0 |
| Unspecced features found | 1 (minor) |

**Headline**: The implementation (`components/zyre/src/{lib,node,ffi}.rs`, `components/interfaces/src/izyre.rs`, `build.rs`, `deps/build_zyre.sh`) matches `specs/001-zyre-bindings/spec.md` exactly across all 12 functional requirements and all 5 success criteria, including both post-2026-07-01 clarification sessions (factory-based `IZyre`, no `_multi` send variants, direct blocking `recv()`/`try_recv()` with no internal threads). **This resolves the previous (2026-07-14) report's headline drift**: that report flagged `plan.md` (stale builder API / 6-module layout) and `tasks.md` (task bodies naming deleted source files) as drifted against the code. Both are now current — `plan.md`'s Project Structure section correctly lists `lib.rs`/`node.rs`/`ffi.rs` plus `interfaces/src/izyre.rs`, and `tasks.md` carries an explicit superseded-design note (`tasks.md:38-45`) with no remaining stale file references. Only one small, low-severity, unspecced behavior was found (a silent no-op in `ZyreNode::stop()` outside the `Running` state), plus one incomplete polish task (`T056`, quickstart end-to-end validation) that is a task-tracking gap rather than a code/spec divergence.

## Spec: 001-zyre-bindings — Zyre Rust Bindings

Code sources reviewed: `components/interfaces/src/izyre.rs` (traits + value types), `components/zyre/src/lib.rs`, `components/zyre/src/node.rs`, `components/zyre/src/ffi.rs`, `components/zyre/build.rs`, `deps/build_zyre.sh`, `deps/install_zyre_deps.sh`, `components/zyre/tests/integration.rs`, `components/zyre/tests/api_safety.rs`, `components/zyre/run-valgrind.sh`, `components/zyre/valgrind.supp`, root `Cargo.toml`.

### Aligned — Functional Requirements

| Req | Spec text (summary) | Evidence |
|---|---|---|
| FR-001 | No `unsafe` required in downstream user code | All `unsafe` is confined to `components/zyre/src/node.rs` (33 occurrences) and `ffi.rs`; public trait `IZyreNode` and re-exported types (`lib.rs:32`) expose no unsafe signatures. Enforced by `tests/api_safety.rs::public_api_has_no_unsafe_exposure`. |
| FR-002 | RAII: dropping a node stops it and frees resources | `impl Drop for ZyreNode` (`node.rs:354-359`) calls `self.stop()` then `ffi::zyre_destroy(&mut self.ptr)`. |
| FR-003 | All 9 zyre event types as a Rust enum with associated data | `ZyreEvent` (`components/interfaces/src/izyre.rs:112-156`) has exactly `Enter, Exit, Evasive, Silent, Join, Leave, Whisper, Shout, Stop`; parsed from the C API in `node.rs::parse_event` (`node.rs:393-489`). |
| FR-004 | Single-frame `&[u8]` whisper/shout; no multi-frame variants | `shout`/`whisper` (`node.rs:176-205`) take `data: &[u8]`; no `_multi` methods exist anywhere in `node.rs`, `izyre.rs`, or `contracts/izyre.md` (removed per the 2026-07-09 clarification). |
| FR-005 | `NodeConfig` public fields + `Default`, `#[non_exhaustive]`, validated at creation | `izyre.rs:267-288` (`#[non_exhaustive]` struct, public fields, `Default` impl); validated via `NodeConfig::validate()` called from `ZyreNode::new` (`node.rs:47`). |
| FR-006 | UDP beacon + gossip discovery; gossip requires explicit node `endpoint` distinct from the gossip hub | `apply_config` (`node.rs:105-125`) applies `zyre_set_endpoint`/`zyre_gossip_bind`/`zyre_gossip_connect`; `NodeConfig::validate` (`izyre.rs:334-344`) rejects gossip config without `endpoint`. Exercised by `tests/integration.rs::gossip_discovery`. |
| FR-007 | Peer introspection: list peers, list groups, get peer headers/address | `peers`, `peers_by_group`, `own_groups`, `peer_groups`, `peer_address`, `peer_header_value` (`node.rs:278-351`); exact signatures match `contracts/izyre.md:73-84`. |
| FR-008 | Blocking `recv()` (via `zyre_event_new`) + non-blocking `try_recv()`; no internal threads; post-`stop()` draining ends in a single terminal `Stop` sentinel, then `Stopped`/`Ok(None)` | `recv`/`try_recv` (`node.rs:208-258`) implement exactly this state machine via the `State` enum (`Created → Running → Draining → Done`, `node.rs:13-18`); `try_recv` uses `zpoller_wait(..., 0)` for non-blocking polling. Verified by `tests/integration.rs::stop_delivers_terminal_stop_event`. |
| FR-009 | Build clones/compiles zyre + libzmq + czmq from source into `deps/zyre-build/` at workspace root, mirroring `deps/spdk-build/` | `deps/build_zyre.sh:11-23` pins `LIBZMQ_TAG=v4.3.5`, `CZMQ_TAG=v4.2.1`, `ZYRE_TAG=v2.0.1`, installs to `${SCRIPT_DIR}/zyre-build`. |
| FR-010 | `bindgen`-generated FFI bindings in a build script, linking against locally-built libs | `build.rs:45-73` (bindgen `Builder`), `build.rs:24-37` (link search paths + rpath into `deps/zyre-build/lib{,64}`). |
| FR-011 | `Send` but not `Sync` | `unsafe impl Send for ZyreNode` (`node.rs:39`) with SAFETY comment; no `Sync` impl anywhere; `IZyreNode: Send` (`izyre.rs:362`). Verified by `tests/api_safety.rs::zyre_node_handle_is_send`. |
| FR-012 | Typed `ZyreError` enum covering start failure, invalid config, network errors | `ZyreError` (`izyre.rs:16-32`): `CreateFailed`, `StartFailed`, `NotStarted`, `InvalidConfig`, `SendFailed`, `RecvFailed`, `Stopped`. |

### Aligned — Success Criteria

| Criterion | Evidence |
|---|---|
| SC-001 (2s round-trip on localhost) | `tests/integration.rs::round_trip_within_two_seconds` asserts `elapsed < Duration::from_secs(2)` on a real localhost ping/pong exchange; skips itself only under `ZYRE_TEST_TIMEOUT_SCALE>1` (valgrind runs), not under normal `cargo test`. |
| SC-002 (zero `unsafe` in public API) | No `unsafe` token in `lib.rs`; public `IZyreNode`/`IZyre` trait method signatures contain no unsafe fns; asserted by `tests/api_safety.rs`. |
| SC-003 (<5 min clean build) | `specs/001-zyre-bindings/tasks.md:129` (T042) records a measured from-scratch `cargo build -p zyre` of 2.27s once C deps are pre-built; the one-time C-dependency build is the SPDK-precedent cost and was not re-measured this pass — task is marked `[~]` (partially verified) rather than fully closed. |
| SC-004 (9 event types, no information loss vs C API) | Same enum cited under FR-003; each variant carries peer/name/group/message/headers/address as applicable. |
| SC-005 (memory safety: Miri or valgrind, zero errors) | Miri cannot cross the FFI boundary (documented in `run-valgrind.sh:6-7`), so `components/zyre/run-valgrind.sh` + `valgrind.supp` run memcheck over the test binaries; `tasks.md:184` (T057) records 0 errors / 0 bytes lost attributable to the bindings (C-library-internal reachable/leak reports suppressed and documented). |

### Not implemented

None.

### Resolved since the 2026-07-14 report

- **`plan.md` staleness (was D-1, major)** — RESOLVED. `plan.md`'s Summary and Project Structure sections now describe public-field `NodeConfig`/`GossipConfig` (no builder) and the actual 3-file `zyre` crate (`lib.rs`/`node.rs`/`ffi.rs`) plus `interfaces/src/izyre.rs`; the source tree lists `tests/api_safety.rs`.
- **`tasks.md` stale file references (was D-2, moderate)** — RESOLVED. The superseded-design note at `tasks.md:38-45` now explicitly names the earlier `event.rs`/`builder.rs`/`error.rs`/`peer.rs`/`NodeConfigBuilder` design and states it was replaced by the factory refactor (commit `b45418d`); no task body elsewhere in the file still references those deleted files.
- **`research.md:84` "builder validation failure" phrasing** — checked; `research.md` §R6 (Error Handling) no longer uses builder terminology in the current text.

## Unspecced Code

| Feature | Location | Lines | Suggested spec addition |
|---|---|---|---|
| `ZyreNode::stop()` is a silent no-op when called on a node that is not `Running` (i.e., before `start()`, or a second `stop()` call while already `Draining`/`Done`) | `components/zyre/src/node.rs:146-155` (`if self.state.get() == State::Running { ... }` — no `else` branch, no error returned) | ~10 | Add a note to `data-model.md`'s "ZyreNode Lifecycle" state-transition diagram: calling `stop()` before `start()`, or calling it more than once, is idempotent/no-op rather than an error — the diagram currently documents only the `Running --stop()--> Draining` edge. |

No other implementation surface (public types, methods, build steps, or test infrastructure) was found without a corresponding spec/contract/task reference.

## Conflicts

None found between spec artifacts, contracts, data model, quickstart, and tasks — all are mutually consistent with each other and with the code as of this review.

## Recommendations

1. **(Optional, low priority)** Document `stop()`'s no-op behavior outside the `Running` state in `data-model.md` (see Unspecced Code above). This is a documentation completeness improvement, not a functional gap — the current behavior is reasonable and doesn't need to change.
2. **(Optional, tracking only)** Close out `tasks.md` T056 ("Validate quickstart.md instructions end-to-end") and fully verify T042/SC-003 by re-running `deps/build_zyre.sh` from a clean `deps/zyre-build/` and recording the total wall-clock time, so SC-003 has a complete (not partial) verification record.
3. No code or spec changes are required to resolve drift — there is none outstanding. The component is in a well-synchronized state and can be treated as a clean baseline for future spec/code changes.
