# Drift Report: extended-metadata-store

**Generated**: pending
**Project**: extended-metadata-store

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 52 |
| Aligned | 41 |
| Drifted | 5 |
| Not Implemented | 0 |
| Unspecced Features | 3 |

Specs: `001-extended-metadata-store` (17 FR + 10 NFR + 7 SC), `002-ssd-integration-test` (12 FR + 6 SC). Both specs were self-synced on 2026-08-07 and honestly document most of the divergences below; they are still reported here because the underlying spec-intent vs working-code gaps persist.

## Detailed Findings

### Spec 001-extended-metadata-store — Extended Metadata Store

**Aligned ✓**
- FR-01 `put(key,value)` 0–128 KiB — `src/lib.rs:158`
- FR-02 `get(key)` returns clone / `NotFound` — `src/lib.rs:172`
- FR-03 `delete(key)` idempotent — `src/lib.rs:182`
- FR-04 `iterate_all()` snapshot — `src/lib.rs:193`
- FR-06 128 KiB `ValueTooLarge` enforcement — `src/lib.rs:159`, `MAX_VALUE_SIZE` `src/lib.rs:63`
- FR-07..FR-14 dual-region ping-pong flush, recovery, fresh-format, FlushManager, dirty count — `src/flush.rs`, `src/recovery.rs`, `src/on_disk.rs` (all `testing`-gated; present, not runtime-verified here)
- FR-15 `define_component!` provides `IExtendedMetadataStore` — `src/lib.rs:40`
- FR-16 optional `ILogger` receptacle — `src/lib.rs:44`
- FR-17 persistence-wiring API (`initialize_from_client`, `snapshot_entries`, `mark_flushed`, `load_entries`, `dirty_count`, `flush_seq`) — `src/lib.rs:70-155`
- NFR-01 `RwLock` store; NFR-05 `on_disk` always compiled, I/O modules `testing`-gated (`src/lib.rs:26-38`); NFR-06 in-memory default build; NFR-09/NFR-10 format constants — aligned

**Drifted ⚠️**
- FR-05 `force_flush()` durability — **minor**
  - Spec: FR-05 table row states the fix is "Fix drafted (branch `sync/spec-drift-sweep-20260807`)".
  - Actual: the trigger-based fix is already present in the working tree — `force_flush()` invokes an installed `FlushTrigger` and blocks (`src/lib.rs:201-215`), with `attach_flush_trigger` at `src/lib.rs:111`. Spec text is stale relative to the code. The substantive gap remains: interface-level durability is a no-op unless a trigger is wired, and it is unverified under `testing`/`spdk` (see next item).
- NFR-07 / test build — **major**
  - Spec: `MockBlockDevice` provides fault-injection for deterministic testing; SC-002/SC-003 (001) and all 002 SCs rely on it.
  - Actual: `MockBlockDevice`'s `impl IBlockDevice` (`src/test_support.rs:171`) does **not** implement `read_write_stats` (only `telemetry` at `:212`). The current `IBlockDevice` trait requires `read_write_stats` (`../interfaces/src/iblock_device.rs:589`), so the `testing`/`spdk` test build does not compile — 001 persistence tests and all 002 SSD tests cannot run. Documented in the 001 Known Gaps (ALIGN-001).
- Workspace membership — **moderate**
  - Spec: SC 1/2/3/6/7 (001) and SC-001..006 (002) presuppose `cargo test`/CI can build the crate.
  - Actual: `extended-metadata-store` is absent from the root `Cargo.toml` `members`/`default-members` (only `logger` present at `Cargo.toml:23,70`). CI never exercises the crate. Documented (ALIGN-001).
- 002 FR-011 interface-only usage — **moderate**
  - Spec: test MUST use the standard `IExtendedMetadataStore` interface, not internal APIs.
  - Actual: store creation and durability go through inherent/internal APIs (`initialize_from_client`, `snapshot_entries`, `mark_flushed`, `load_entries`, `flush::flush_to_disk`), a direct consequence of the FR-05 no-op history. `put/get/delete/iterate_all` do use the interface. Documented in the sync note.
- 002 capacity scenario (US5 / FR/edge) — **moderate**
  - Spec/test: `test_capacity_exhaustion` expects a `put()`-level capacity error.
  - Actual: `put()` enforces only `ValueTooLarge`; capacity is enforced solely at flush time inside `flush::flush_to_disk` as a `String` error and is never mapped to `CapacityExhausted` on any interface method, so the test passes trivially without reaching the exhaustion branch. Documented in the 002 capacity note.

**Not Implemented ✗**
- None. (FR-05 durability under `testing`/`spdk` is unverified rather than absent — code exists.)

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `Superblock::region_capacity_bytes()` never-called public accessor | `src/on_disk.rs` | 142 | Document as public format API or remove |
| `create_test_component_from_state()` unused public test helper (`testing`) | `src/test_support.rs` | 268 | Document as test surface or remove |
| `ExtendedMetadataStoreError::CapacityExhausted` variant never constructed | `../interfaces/src/iextended_metadata_store.rs` | 12 | Either surface via an interface method or drop the variant |

(All three are already acknowledged in the 001 spec "Dead public API surface" note.)

## Recommendations
1. Fix `MockBlockDevice` to implement `read_write_stats` so the `testing`/`spdk` test build compiles (ALIGN-001) — this is the highest-value unblock; without it FR-05 verification and every persistence/SSD SC stay unexercised.
2. Add `extended-metadata-store` to the workspace `members`/`default-members` once the mock compiles, so SC 1/2/3/6/7 and 002 run in CI.
3. Update the FR-05 table row: the trigger fix is in-tree, not a branch draft. Then verify `force_flush()` durability under `testing` and re-point 002 FR-007/FR-011 at the interface.
4. Resolve the capacity story: either map region-capacity exhaustion to `CapacityExhausted` on an interface method, or rewrite `test_capacity_exhaustion` to assert a flush-time error against an undersized region.
