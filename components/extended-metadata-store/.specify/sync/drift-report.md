# Spec-Drift Report: extended-metadata-store

**Generated**: 2026-07-22T21:30:28Z
**Spec analyzed**: `specs/001-extended-metadata-store/spec.md` (Status: *Backfilled*, i.e. generated from existing code — drift is expected to be low by construction, so findings here represent real gaps between the *documented contract* and *actual wiring*, not stylistic nitpicks).

## Summary

| Metric | Count |
|---|---|
| Specs analyzed | 1 |
| Requirements checked (16 FR + 10 NFR + 7 Success Criteria) | 33 |
| Aligned | 28 |
| Drifted | 5 |
| Not implemented | 0 |
| Unspecced features | 1 |
| Spec-internal conflicts | 1 |

## Headline Finding

**The crate is not a member of the Cargo workspace.** `components/extended-metadata-store` does not appear anywhere in the root `Cargo.toml` `[workspace] members` list (verified via `grep` and via `git log -p -- Cargo.toml`, which shows zero history of the string ever appearing). Consequences, verified live:

```
$ cargo test -p extended-metadata-store
error: package ID specification `extended-metadata-store` did not match any packages

$ cd components/extended-metadata-store && cargo test
error: current package believes it's in a workspace when it's not:
current:   .../components/extended-metadata-store/Cargo.toml
workspace: .../Cargo.toml
```

This means none of the crate's tests, `cargo clippy`, or `cargo doc` are exercised by `cargo build`/`cargo test --all` or by CI (`.github/workflows/rust.yml`), despite the spec's Success Criteria explicitly claiming they pass. This is a build-wiring gap, not a code-correctness gap — the code itself looks correct wherever it was manually inspected — but it means **zero automated verification currently covers this component**.

## Per-Spec Findings — `001-extended-metadata-store` "Extended Metadata Store"

### Aligned (28)

All of FR-01 through FR-04, FR-06 through FR-04, FR-07 through FR-16, and NFR-01 through NFR-10 (except NFR-05, see Conflicts) are backed by matching code:

- `put`/`get`/`delete`/`iterate_all` — `src/lib.rs:134-175`, correct semantics (size check, `NotFound`, idempotent delete, RwLock snapshot).
- 128 KiB limit — `MAX_VALUE_SIZE` (`src/lib.rs:56`), enforced in `put()`.
- Dual-region ping-pong flush — `flush_to_disk()` (`src/flush.rs:20-53`): writes inactive region, flips `active_region`, bumps `flush_seq`, writes superblock as atomic commit point.
- Recovery with inactive-region fallback and fresh-format detection — `src/recovery.rs:23-96`, verified against test `crash_mid_flush_recovers_previous_state` (`tests/persistence.rs:674-746`), which reproduces "old data intact if crash happens mid-write" correctly.
- `FlushManager` background thread, timer + dirty-threshold triggers, coalesced `trigger_flush()`, final flush on `Drop` — `src/flush.rs:76-253` — correctly implemented and tested (`tests/persistence.rs:488-624`).
- CRC32/sector-alignment/little-endian on-disk format, magic number `0x4345_5254_4D45_5441` — `src/on_disk.rs`, round-trip and corruption tests present.
- `ILogger` receptacle used in all mutating/reading ops — `src/lib.rs`.
- `MockBlockDevice` + `FaultConfig` test infra — `src/test_support.rs`.

### Drifted (5)

| Requirement | Spec text | Actual | Location | Severity |
|---|---|---|---|---|
| FR-05 (`force_flush` durability) | "`force_flush()` ensures all mutations are durable on disk — Implemented" (spec.md:159); interface doc: "Flush all pending writes to persistent storage. Returns when all data is durable." | `IExtendedMetadataStore::force_flush()` is an **unconditional no-op in every build configuration** (default, `testing`, `spdk`) — it never calls `flush::flush_to_disk` or `FlushManager::trigger_flush`. Every persistence/SSD test that needs durability calls `flush::flush_to_disk()` or `FlushManager::trigger_flush()` **directly**, bypassing the trait method entirely (`grep` confirms the only call site of `.force_flush()` in the whole crate is the no-op unit test). A production caller that only knows the `IExtendedMetadataStore` interface gets silent data loss on "flush". | `src/lib.rs:177-184` | **High** |
| SC-1, SC-2, SC-3, SC-6, SC-7 (build/test/lint/doc verification) | "1. All 8 unit tests… pass… 2. All persistence tests… pass… 3. All SSD integration tests… pass… 6. `cargo clippy -- -D warnings` passes clean… 7. …`cargo doc --no-deps` is warning-free" (spec.md:235-241) | Crate is absent from workspace `members`; none of these commands can currently target the crate (see Headline Finding). Criteria are unverifiable/unverified as written. | `Cargo.toml` (workspace root); `components/extended-metadata-store/Cargo.toml` | **High** |
| SC-1 (unit test count) | "All 8 unit tests in `src/lib.rs` pass" (spec.md:235) | `src/lib.rs` has **9** `#[test]` functions — `dirty_count_increments` (`src/lib.rs:290-303`) is not accounted for in the stated count or in User Story 1's test list. | `src/lib.rs:290-303` | Low |
| plan.md test inventory (`on_disk.rs`) | "5 tests in `on_disk.rs`: superblock round-trip, corruption detection, entry round-trip, region round-trip, padding" (plan.md:138) | `src/on_disk.rs` has **6** `#[test]` functions (corruption detection is actually two separate tests: `superblock_corrupt_rejected` and `entry_record_corrupt_rejected`). | `src/on_disk.rs:405-473` | Low |
| plan.md test inventory (`integration_ssd.rs`) | "12 tests: put/get varied sizes, overwrite, delete, persistence after flush, iterate, bulk integrity, capacity" (plan.md:144) | `tests/integration_ssd.rs` has **14** `#[test]` functions. | `tests/integration_ssd.rs` | Low |

### Not Implemented (0)

None — every FR/NFR has corresponding code, even where (as with FR-05) it is not correctly wired to the public interface.

## Unspecced Code

| Feature | Location | Lines | Suggested Spec Coverage |
|---|---|---|---|
| External persistence-wiring API required to actually get durability: `ExtendedMetadataStoreComponent::initialize_from_client`, `snapshot_entries`, `mark_flushed`, `load_entries`, `dirty_count()`, `flush_seq()` inherent methods. These are the *real* mechanism by which a caller achieves persistence (recovery on startup, flush on demand), since the `IExtendedMetadataStore::force_flush()` trait method does not do it (see drift above). None of this wiring contract is described as a formal FR/NFR — it only appears informally in plan.md's "Data Flow" diagrams. | `src/lib.rs` | 58–131 (~70 lines) | Add an FR describing the startup/runtime wiring contract for persistent-mode deployments: who calls `initialize_from_client`, who owns/drives a `FlushManager`, and an explicit note that `force_flush()` on the public interface is currently a no-op and must not be relied upon for durability. |

## Spec-Internal Conflicts

1. **NFR-05 vs. plan.md module graph vs. tasks.md.** `spec.md` NFR-05 states "Persistence modules gated behind `testing`/`spdk` feature flags — Implemented" (spec.md:180). But `plan.md`'s own "Module Dependency Graph" documents `on_disk.rs` — which defines the `Superblock`/`RegionHeader`/`EntryRecord` on-disk format, unambiguously a "persistence module" — as `[always compiled]` (plan.md:63-64), and `src/lib.rs:26` confirms `pub mod on_disk;` carries no `#[cfg(feature = ...)]` gate at all (unlike `block_io`, `flush`, `recovery`, `test_support`, which are all `#[cfg(feature = "testing")]`). `tasks.md` itself lists this exact ambiguity as an open backlog item (T057: "Consider whether `on_disk.rs` should remain always-compiled or be gated behind a feature flag"), confirming this is a genuinely unresolved inconsistency between the "Implemented" claim in spec.md and the documented/actual design in plan.md, rather than a stylistic slip.

## Recommendations

1. **Add `components/extended-metadata-store` to the workspace `members` array** in the root `Cargo.toml` (and, if hardware/SPDK dependent tests should stay opt-in, to `default-members` for the non-`spdk` default build only). This is the single highest-leverage fix — it makes every other Success Criterion checkable by CI for the first time.
2. **Wire `IExtendedMetadataStore::force_flush()` to actual persistence.** Either (a) have `force_flush()` call into a `FlushManager`/`flush_to_disk` when the `testing`/`spdk` feature is enabled and the component has been initialized with a `BlockDeviceClient`, or (b) if the intended architecture really is "the interface method is a hint only, real durability requires holding a `FlushManager`," update FR-05, the interface doc comment, and User Story 6's acceptance criteria to say so explicitly instead of "Implemented."
3. Resolve the NFR-05 / `on_disk.rs` gating conflict per tasks.md T057, then update spec.md NFR-05 wording to match whatever decision is made.
4. Correct the test counts in spec.md (SC-1: 9, not 8) and plan.md (`on_disk.rs`: 6, not 5; `integration_ssd.rs`: 14, not 12) — low effort, low severity, but easy to fix while other spec edits are made.
5. Once (1) is done, actually run `cargo test -p extended-metadata-store --features testing`, `cargo clippy -p extended-metadata-store -- -D warnings`, and `cargo doc -p extended-metadata-store --no-deps` to confirm SC-2, SC-6, SC-7 for real (not yet possible in this read-only analysis).
