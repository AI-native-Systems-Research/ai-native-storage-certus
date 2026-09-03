---
spec_sync_component: extended-metadata-store
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T17:12:21Z
spec_sync_git_commit: 4167ebf8
spec_sync_inputs_sha256: 1ef96df8c3cc902b82582a2ba5632f68e011bb3983d979522624cbf818140c16
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report — extended-metadata-store

**Generated**: 2026-09-03
**Mode**: Read-only drift analysis. No code or spec behavior changes applied this
sweep; the only edit was recording the confirmation date in the spec's
`Last-Synced` line. One open ALIGN item is documented and parked by maintainer
decision.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (`001-extended-metadata-store`) |
| Requirements Checked | 18 FR + 11 NFR + 7 SC |
| Aligned | 36 |
| Drifted | 0 actionable |
| Not Implemented | 0 |
| Unspecced | 0 |
| Parked (open ALIGN, documented) | 1 |

This report supersedes the earlier stale artifact (which read "Generated:
pending", "Specs Analyzed: 2", and referenced a non-existent `002` spec). The
2026-08-20 Phase B backfill had already rewritten the spec to match the shipped
implementation; this sweep re-verifies that alignment against the current tree
and finds it holds. The `002` spec never existed — that reference was an
artifact of the stale report and is not re-introduced.

## Spec: 001-extended-metadata-store

### Aligned ✓ (verified this sweep)

| Req | Evidence |
|-----|----------|
| FR-01 put(key, value) stores bytes | `src/lib.rs:158-170` |
| FR-02 get(key) returns stored value | `src/lib.rs` get path; unit tests in `mod tests` |
| FR-03 delete(key) removes entry | `src/lib.rs`; covered by lib unit tests |
| FR-04 in-memory default mode (no device) | default build compiles without `testing`/`spdk`; 9 lib unit tests green |
| FR-05 force_flush() invokes durable flush trigger when attached, else no-op | `src/lib.rs:201-215` (`force_flush`), `:111` (`attach_flush_trigger`), `:68` (`FlushTrigger` alias) |
| FR-06 ValueTooLarge enforced at put() (128 KiB max) | `src/lib.rs:158-170` returns `ValueTooLarge`; only enforced error at put time |
| FR-07..FR-18 persistence/format/CRC/ping-pong behaviors | `src/flush.rs`, `src/on_disk.rs`; exercised by `--features testing` persistence suite (19 tests) |
| NFR-01..NFR-11 | dual-region ping-pong flush, CRC32 on-disk integrity, feature-gating (`testing = ["interfaces/spdk"]`), workspace membership (`Cargo.toml:23`, dep `:105`) |
| SC-1 9 unit tests in `src/lib.rs` | `mod tests` in `src/lib.rs` (default `cargo test` reports 15 lib unit = 9 in lib.rs + 6 always-compiled in `on_disk.rs`) |
| SC-2..SC-7 | MockBlockDevice-backed round-trip, crash-consistency, capacity accounting, CRC detection — all present and passing |

**Verification runs this sweep** (all green):
- default `cargo test -p extended-metadata-store` — 15 lib unit tests
- `cargo test -p extended-metadata-store --features testing` — 19 persistence tests
- `cargo clippy -p extended-metadata-store` — no warnings
- `cargo doc -p extended-metadata-store --no-deps` — warning-free

### Phase B blockers — confirmed resolved

The 2026-08-20 backfill claimed Phase B had cleared the MockBlockDevice and
workspace-membership blockers. Both claims verified true in the current tree:
- `MockBlockDevice impl IBlockDevice` — `src/test_support.rs:171`; stats accessor
  `read_write_stats` — `src/test_support.rs:223`; state helper
  `create_test_component_from_state` — `src/test_support.rs:272`.
- Workspace membership — `Cargo.toml:23` (member) and `Cargo.toml:105`
  (workspace dependency).

### Parked (open ALIGN — documented, no change this sweep)

- **`CapacityExhausted` is defined but never constructed.** The interface variant
  `ExtendedMetadataStoreError::CapacityExhausted`
  (`components/interfaces/src/iextended_metadata_store.rs:12`) is not produced by
  any `src/` path. `put()` enforces only `ValueTooLarge` (`src/lib.rs:158-170`);
  region-capacity overflow is surfaced at flush time as a `String`
  ("exceeds region capacity", `src/flush.rs:34-38`), not as the typed
  `CapacityExhausted` variant.
  - **Consequence:** the hardware integration test `test_capacity_exhaustion`
    (`tests/integration_ssd.rs:513-547`) is **vacuous** — its match arm expecting
    `CapacityExhausted` (`:530`) can never be taken, because `put()` never returns
    it. The corresponding persistence test `capacity_exhaustion_detected`
    (T051, `tests/persistence.rs:627`) is meaningful: it drives the flush-time
    `String` error and passes.
  - **Resolution class:** ALIGN (either construct `CapacityExhausted` on the
    capacity path and fix the test, or remove the dead variant and rewrite the
    test around the flush-time error).
  - **Decision (2026-09-03, maintainer):** keep parked and documented. No code
    change this sweep. Because it is a defined-but-unconstructed variant plus a
    non-exercised hardware-only test — not a behavioral discrepancy between spec
    and shipped behavior — it does not constitute actionable spec/impl drift, and
    the report is stamped `clean`. Revisit when the capacity error path is next
    touched.

### Not Implemented ✗

None.

## Unspecced Features

None. All implemented surface is captured in the spec.

## Recommendations

- When the capacity error path is next revisited, resolve the parked ALIGN item
  above (construct the typed variant + de-vacuum `integration_ssd.rs:513-547`, or
  retire the variant and rewrite the test).
- Commit this stamped `drift-report.md` together with the spec edit so the CI
  Spec-Sync Gate sees a fresh report whose input hash matches the tree.
