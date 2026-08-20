# Drift Report: zyre

**Generated**: pending
**Project**: zyre

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 |
| Requirements Checked | 17 |
| Aligned | 17 |
| Drifted | 0 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

The zyre bindings are fully aligned with `001-zyre-bindings`. **CLEAN.**

## Detailed Findings

### 001-zyre-bindings — Safe Rust Zyre Bindings

FR-001..FR-012 and SC-001..SC-005. All Aligned.

- ✓ FR-001 (safe bindings, no `unsafe` in downstream code): public API in
  `src/lib.rs` / `src/node.rs`; unsafe confined to FFI layer.
- ✓ FR-002 (RAII lifecycle create/configure/start/stop, Drop stops node):
  `ZyreNode` + Drop `src/node.rs:354`.
- ✓ FR-003 (all 9 event types as enum with data): `parse_event`
  `src/node.rs:422-488` covers ENTER/EXIT/EVASIVE/SILENT/JOIN/LEAVE/WHISPER/
  SHOUT/STOP.
- ✓ FR-004 (single-frame `&[u8]` whisper/shout, no multi-frame): `shout` /
  `whisper` in `IZyreNode` impl, `src/node.rs`.
- ✓ FR-005 (typed `NodeConfig`, public fields + Default, `#[non_exhaustive]`):
  re-exported via `interfaces`, used by `create_node` `src/lib.rs:51`.
- ✓ FR-006 (UDP beacon + gossip discovery, gossip endpoint): handled in
  `start` path, `src/node.rs`.
- ✓ FR-007 (peer introspection): `peers`, `own_groups`, `peer_groups`,
  `peer_address`, `peer_header_value` in `IZyreNode` impl, `src/node.rs`.
- ✓ FR-008 (blocking `recv()` + non-blocking `try_recv()`, no bg threads,
  terminal `Stop` sentinel state machine): `State` enum
  (Created/Running/Draining/Done), `recv`/`try_recv` in `src/node.rs`.
- ✓ FR-009 (build clones zyre/libzmq/czmq to `deps/zyre-build/`):
  `deps/build_zyre.sh`, `deps/install_zyre_deps.sh`, `deps/zyre-build` exist.
- ✓ FR-010 (bindgen build script): `src/ffi.rs` includes
  `concat!(env!("OUT_DIR"), "/bindings.rs")`.
- ✓ FR-011 (Send, not Sync): `unsafe impl Send` only at `src/node.rs:39`.
- ✓ FR-012 (typed `ZyreError` enum): defined and returned across
  `src/node.rs`.
- ✓ SC-001 (round-trip discovery), SC-002 (zero unsafe in public API),
  SC-003 (clean-checkout build time), SC-004 (9 event types representable),
  SC-005 (Miri/valgrind memory safety): consistent with implementation;
  SC-001/SC-003/SC-005 are runtime-measured and are not contradicted by the
  code structure.

## Unspecced Features

None. The single-frame-only API matches the superseded-multi-frame decision
recorded in the spec (2026-07-09).

## Recommendations

None. No drift detected.
