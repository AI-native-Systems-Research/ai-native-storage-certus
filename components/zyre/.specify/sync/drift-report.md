---
spec_sync_component: zyre
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-02T21:46:22Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 2b61d7c1c959af0c466d8fb2967b3247f457a192c8812c895dc3e5df215ae721
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report: zyre

**Generated**: 2026-09-02T21:46:22Z
**Project**: zyre
**Spec**: `specs/001-zyre-bindings`

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 17 (FR-001..FR-012, SC-001..SC-005) |
| Aligned | 17 |
| Drifted | 0 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

The zyre bindings are fully aligned with `001-zyre-bindings`. **CLEAN.**

Note on type location: `IZyre`, `IZyreNode`, and the value types (`NodeConfig`,
`GossipConfig`, `ZyreEvent`, `PeerId`, `ZyreError`) live in the `interfaces`
crate (`components/interfaces/src/izyre.rs`) and are re-exported by the `zyre`
crate (`src/lib.rs:32`), matching the 2026-07-09 spec clarification. Evidence
below cites `interfaces/src/izyre.rs` where the requirement is realized there;
`interfaces/**` is read-only and was not modified.

## Detailed Findings

### 001-zyre-bindings — Safe Rust Zyre Bindings

#### Functional Requirements

- ✓ **FR-001** (safe bindings, no `unsafe` in downstream code): public API in
  `src/lib.rs` and `src/node.rs`; all `unsafe` confined to the FFI calls inside
  `node.rs` and the generated bindings (`src/ffi.rs:7`). Compile-time proof that
  the public value types are constructible without `unsafe` in
  `tests/api_safety.rs:28-46`.
- ✓ **FR-002** (RAII lifecycle create/configure/start/stop; drop stops node):
  `ZyreNode::new` validates + configures (`src/node.rs:46-71`), `start`
  (`src/node.rs:133-143`), `stop` (`src/node.rs:146-155`), and `Drop` calls
  `stop()` then `zyre_destroy()` (`src/node.rs:354-359`).
- ✓ **FR-003** (all 9 event types as enum with data): `ZyreEvent` enum with 9
  variants (`interfaces/src/izyre.rs`, Enter/Exit/Evasive/Silent/Join/Leave/
  Whisper/Shout/Stop); `parse_event` maps every C event type at
  `src/node.rs:422-488`.
- ✓ **FR-004** (single-frame `&[u8]` whisper/shout, no multi-frame variants):
  `shout` (`src/node.rs:176-189`) and `whisper` (`src/node.rs:192-205`) take
  `&[u8]` and add one `zmsg` frame; no `_multi` methods exist. Matches the
  2026-07-09 supersession.
- ✓ **FR-005** (typed `NodeConfig`, public fields + `Default`,
  `#[non_exhaustive]`, validated at create): `NodeConfig`
  (`interfaces/src/izyre.rs`, `#[non_exhaustive]` + `Default` + `validate`);
  `create_node` → `ZyreNode::new` → `config.validate()` (`src/node.rs:47`).
- ✓ **FR-006** (UDP beacon + gossip; gossip requires explicit `endpoint`):
  `apply_config` sets `zyre_set_endpoint`/`zyre_gossip_bind`/
  `zyre_gossip_connect` (`src/node.rs:105-125`); validation requires
  `endpoint` when `gossip` is set (`interfaces/src/izyre.rs`
  `NodeConfig::validate`).
- ✓ **FR-007** (peer introspection): `peers` (`src/node.rs:278`),
  `peers_by_group` (`:288`), `own_groups` (`:299`), `peer_groups` (`:309`),
  `peer_address` (`:319`), `peer_header_value` (`:339`), plus `uuid`/`name`.
- ✓ **FR-008** (blocking `recv()` + non-blocking `try_recv()`, no bg threads,
  terminal `Stop` sentinel semantics): `State` enum
  Created/Running/Draining/Done (`src/node.rs:12-18`); `recv` wraps
  `zyre_event_new` and returns `NotStarted`/`Stopped` per state, advancing to
  `Done` on the `Stop` sentinel (`src/node.rs:208-228`); `try_recv` uses
  `zpoller` with zero timeout and returns `Ok(None)` once `Done`
  (`src/node.rs:233-258`). No threads spawned in the crate.
- ✓ **FR-009** (build clones zyre/libzmq/czmq to `deps/zyre-build/`):
  `deps/build_zyre.sh` clones libzmq `v4.3.5`, czmq `v4.2.1`, zyre `v2.0.1`
  and installs to `deps/zyre-build` (lines 12-23, 32, 60-64);
  `deps/install_zyre_deps.sh` for system prerequisites.
- ✓ **FR-010** (bindgen build script): `build.rs:45-73` runs `bindgen` against
  `zyre.h` and writes `bindings.rs`; `src/ffi.rs:7` includes it. Links against
  `deps/zyre-build/{lib,lib64}` (`build.rs:24-37`).
- ✓ **FR-011** (`Send`, not `Sync`): sole `unsafe impl Send for ZyreNode`
  (`src/node.rs:39`), no `Sync` impl; `IZyreNode: Send` supertrait
  (`interfaces/src/izyre.rs`); compile-time assertions in
  `tests/api_safety.rs:10-26`.
- ✓ **FR-012** (typed `ZyreError` enum): `ZyreError` with CreateFailed,
  StartFailed, NotStarted, InvalidConfig, SendFailed, RecvFailed, Stopped
  (`interfaces/src/izyre.rs`), returned throughout `src/node.rs`.

#### Success Criteria

- ✓ **SC-001** (round-trip discovery within 2s on localhost): runtime-measured;
  exercised by the integration path (`tests/integration.rs`). Not contradicted
  by code structure.
- ✓ **SC-002** (zero `unsafe` in public API surface): all `unsafe` is inside
  the crate's FFI layer (`src/node.rs`, `src/ffi.rs`); public re-exports in
  `src/lib.rs:32` expose only safe types. Asserted in `tests/api_safety.rs`.
- ✓ **SC-003** (clean-checkout build < 5 min): build-time/runtime-measured;
  driven by `deps/build_zyre.sh`. Not contradicted by code structure.
- ✓ **SC-004** (all 9 event types representable with no loss): `ZyreEvent` has
  exactly the 9 variants and `parse_event` (`src/node.rs:422-488`) maps each
  without information loss (headers, address, group, message preserved).
- ✓ **SC-005** (Miri/valgrind memory safety, zero errors): runtime-measured;
  supported by `run-valgrind.sh` + `valgrind.supp`. Not contradicted by code
  structure.

## Unspecced Features

None. The node self-introspection helpers (`uuid`, `name`) and `peers_by_group`
are all enumerated in the interface contract (`contracts/izyre.md`) and the
`IZyreNode` trait. The single-frame-only send API matches the superseded
multi-frame decision recorded in `spec.md` (Session 2026-07-09).

## Recommendations

None. No drift detected. Historical references in `tasks.md` (T050 `_multi`
removal, line 178) and `research.md` (rejected builder, lines 56-74) are
intentionally-preserved decision records, not drift.
