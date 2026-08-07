# Drift Report — `zyre`

Generated: 2026-08-07T15:31:01Z

Component: `components/zyre`
Spec analyzed: `specs/001-zyre-bindings/spec.md` (Draft)

## Summary

| Category | Aligned | Drifted | Not Implemented | Unspecced |
|----------|---------|---------|-----------------|-----------|
| FR (001-012) | 12 | 0 | 0 | — |
| Success Criteria (001-005) | 5 | 0 | 0 | — |

Result: **clean**. Safe idiomatic Rust bindings match the spec, including the 2026-07-09 clarifications (no `_multi` send variants; `IZyre` factory + `ping()`; direct `recv()` with no background thread; `Stop` sentinel drain semantics).

## Detailed Findings

### Functional Requirements

| ID | Status | Evidence |
|----|--------|----------|
| FR-001 (no unsafe in user code) | Aligned | All FFI encapsulated in `src/node.rs` / `src/ffi.rs`; verified by `tests/api_safety.rs:29` |
| FR-002 (RAII lifecycle) | Aligned | `Drop` calls `stop()` + `zyre_destroy` — `src/node.rs:354-359` |
| FR-003 (all 9 events as enum) | Aligned | `ZyreEvent` enum (`interfaces/src/izyre.rs:112-155`); parse for all 9 — `src/node.rs:422-488` |
| FR-004 (single-frame `&[u8]` send, no multi) | Aligned | `shout`/`whisper` take `&[u8]`, no `_multi` — `src/node.rs:176-205` |
| FR-005 (typed `NodeConfig`, public fields + Default + non_exhaustive, validated) | Aligned | `interfaces/src/izyre.rs:268-269` (`#[non_exhaustive]`), `validate()` at `:313`; called in `ZyreNode::new` — `src/node.rs:47` |
| FR-006 (UDP beacon + gossip; gossip needs endpoint) | Aligned | gossip endpoint/bind/connect wiring — `src/node.rs:105-125`; endpoint requirement enforced in `validate()` — `interfaces/src/izyre.rs:313-340` |
| FR-007 (peer introspection) | Aligned | `peers`, `peers_by_group`, `own_groups`, `peer_groups`, `peer_address`, `peer_header_value` — `src/node.rs:278-351` |
| FR-008 (blocking `recv` + `try_recv`, no bg thread, Stop drain) | Aligned | `recv` wraps `zyre_event_new` — `src/node.rs:208-228`; `try_recv` via zpoller — `:233-258`; state machine (Created/Running/Draining/Done) implements drain-then-`Stop`-then-`Stopped` — `src/node.rs:13-18,146-155,207-228` |
| FR-009 (build clones/compiles into `deps/zyre-build/`) | Aligned | `deps/build_zyre.sh` clones libzmq/czmq/zyre; `deps/zyre-build/` exists; `build.rs` resolves it — `build.rs:5-18` |
| FR-010 (bindgen in build script, link local libs) | Aligned | `build.rs:45-73`; rpath + link zyre/czmq/zmq — `build.rs:24-37` |
| FR-011 (`Send` not `Sync`) | Aligned | `unsafe impl Send for ZyreNode` only — `src/node.rs:39`; `IZyreNode: Send` — `interfaces/src/izyre.rs:362`; verified `tests/api_safety.rs:11,21` |
| FR-012 (typed `ZyreError`) | Aligned | `ZyreError` enum (CreateFailed/StartFailed/NotStarted/InvalidConfig/SendFailed/RecvFailed/Stopped) — `interfaces/src/izyre.rs:16-31` |

### Success Criteria

| ID | Status | Evidence |
|----|--------|----------|
| SC-001 (2 nodes round-trip <2s) | Aligned | `tests/integration.rs:54` (`two_nodes_discover_and_shout`), `:108` (`two_nodes_whisper`) |
| SC-002 (zero unsafe in public API) | Aligned | `tests/api_safety.rs:29`; public surface is `IZyre`/`IZyreNode` traits |
| SC-003 (clean-checkout build <5min) | Aligned (build-perf, not statically verifiable) | `deps/build_zyre.sh` shallow-clones pinned tags |
| SC-004 (9 event types, no info loss) | Aligned | `ZyreEvent` carries peer/name/group/message/headers/address — `interfaces/src/izyre.rs:112-155` |
| SC-005 (Miri/valgrind clean) | Not statically verifiable here | Bindings free every C object (`zyre_event_destroy`, `zlist_destroy`, `zpoller_destroy`) — `src/node.rs:222,251,283,294`; no evidence of leak/UB in review |

### Clarifications compliance

- 2026-07-09 "remove `_multi`": no `_multi` methods exist. Compliant.
- 2026-07-09 "`IZyre` = `create_node` + `ping()`": `IZyre` trait has both — `src/lib.rs:46-54`. Compliant.
- 2026-07-09 "top-level `deps/zyre-build/`": build.rs targets it. Compliant.
- Version pins (zyre v2.0.1, czmq v4.2.1, libzmq v4.3.5): `deps/build_zyre.sh:15,19,23`. Compliant.

## Unspecced Code

| Item | Location | Assessment |
|------|----------|------------|
| `uuid()` / `name()` self-introspection accessors | `src/node.rs:261-275` | Reasonable handle accessors; adjacent to FR-007 peer introspection, no drift |

No unspecced public component interface beyond the spec.

## Recommendations

None required. Optionally add a CI job running the test suite under valgrind/Miri to make SC-005 verifiable rather than review-only.
