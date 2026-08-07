Generated: 2026-08-07T15:31:25Z

# Drift Report: extended-metadata-store

Spec-vs-implementation drift analysis covering **two** specs:

- `001-extended-metadata-store/spec.md` — component behavior (backfilled from code)
- `002-ssd-integration-test/spec.md` — hardware integration test

Implementation analyzed: `src/{lib,flush,on_disk,recovery,block_io,test_support}.rs`,
`tests/{persistence,integration_ssd}.rs`, interface at
`components/interfaces/src/iextended_metadata_store.rs`, `Cargo.toml`.

## Summary

| Status | 001 | 002 | Total |
|--------|-----|-----|-------|
| Aligned | 28 | 16 | 44 |
| Drifted | 6 | 2 | 8 |
| Not Implemented | 0 | 0 | 0 |
| **Tracked requirements** | 34 | 18 | 52 |
| Unspecced code items | — | — | 3 |
| Conflicts / spec-reference issues | — | — | 4 |

001 tracked = FR-01..17 (17) + NFR-01..10 (10) + SC1..7 (7).
002 tracked = FR-001..012 (12) + SC-001..006 (6).

## Per-Requirement Table

| ID | Spec | Status | Location | Notes |
|----|------|--------|----------|-------|
| 001-FR-01 | 001 | Aligned | src/lib.rs:134-146 | put stores 0-128KiB by key; MAX_VALUE_SIZE=lib.rs:56 |
| 001-FR-02 | 001 | Aligned | src/lib.rs:148-156 | get clones value or NotFound |
| 001-FR-03 | 001 | Aligned | src/lib.rs:158-167 | delete removes; idempotent (HashMap::remove) |
| 001-FR-04 | 001 | Aligned | src/lib.rs:169-175 | iterate_all snapshot under read lock |
| 001-FR-05 | 001 | **Drifted (High)** | src/lib.rs:177-184 | force_flush is an unconditional no-op in ALL configs; never calls flush_to_disk/trigger_flush |
| 001-FR-06 | 001 | Aligned | src/lib.rs:135-137 | ValueTooLarge above 128KiB |
| 001-FR-07 | 001 | Aligned | src/flush.rs:20-53 | dual-region ping-pong; superblock is commit point |
| 001-FR-08 | 001 | Aligned | src/recovery.rs:23-53 | reads superblock, loads active region |
| 001-FR-09 | 001 | Aligned | src/recovery.rs:63-78 | falls back to inactive region on corruption |
| 001-FR-10 | 001 | Aligned | src/recovery.rs:27-33,82-103; src/lib.rs:106-113 | fresh partition detect + format |
| 001-FR-11 | 001 | Aligned | src/flush.rs:99-208 | FlushManager timer + dirty threshold |
| 001-FR-12 | 001 | Aligned | src/flush.rs:142-165 | coalescing at FlushManager level (not via interface force_flush — see FR-05) |
| 001-FR-13 | 001 | Aligned | src/flush.rs:239-253,187-190 | final flush on Drop |
| 001-FR-14 | 001 | Aligned | src/lib.rs:60-62,144,165 | AtomicU64 dirty count |
| 001-FR-15 | 001 | Aligned | src/lib.rs:40-53 | define_component! provides IExtendedMetadataStore |
| 001-FR-16 | 001 | Aligned | src/lib.rs:44-46 | optional ILogger receptacle, used throughout |
| 001-FR-17 | 001 | Aligned | src/lib.rs:60-130 | wiring API: initialize_from_client/snapshot_entries/mark_flushed/load_entries/dirty_count/flush_seq |
| 001-NFR-01 | 001 | Aligned | src/lib.rs:48 | RwLock<HashMap> |
| 001-NFR-02 | 001 | Aligned | src/on_disk.rs:357-370 | CRC32 on superblock/region/entry |
| 001-NFR-03 | 001 | Aligned | src/on_disk.rs:346-351 | sector-aligned pad_to_sector |
| 001-NFR-04 | 001 | Aligned | src/flush.rs:20-53; tests/persistence.rs:675 | crash consistency (ping-pong + fault-injection test) |
| 001-NFR-05 | 001 | Aligned | src/lib.rs:26-38 | block_io/flush/recovery/test_support gated; on_disk always compiled |
| 001-NFR-06 | 001 | Aligned | src/lib.rs default build | in-memory mode w/o SPDK |
| 001-NFR-07 | 001 | Aligned | src/test_support.rs:17-169 | MockBlockDevice + FaultConfig |
| 001-NFR-08 | 001 | Aligned | src/block_io.rs; src/test_support.rs:240-256 | DmaAllocFn abstraction |
| 001-NFR-09 | 001 | Aligned | src/on_disk.rs:372-403 | little-endian to_le_bytes/from_le_bytes |
| 001-NFR-10 | 001 | Aligned | src/on_disk.rs:6 | magic 0x4345525454454D5441 "CERTMETA" |
| 001-SC1 | 001 | **Drifted (High)** | src/lib.rs:187-304 | 9 unit tests exist & count matches, but crate is NOT in root Cargo.toml [workspace] members → not exercised by cargo test --all / CI |
| 001-SC2 | 001 | **Drifted (High)** | tests/persistence.rs | tests exist (--features testing) but unrunnable via workspace (not a member) |
| 001-SC3 | 001 | **Drifted (High)** | tests/integration_ssd.rs | tests exist (--features spdk) but unrunnable via workspace (not a member) |
| 001-SC4 | 001 | Aligned | tests/persistence.rs:675 | crash_mid_flush_recovers_previous_state |
| 001-SC5 | 001 | Aligned | tests/persistence.rs:417 | concurrent_stress_8_threads (8×1000) |
| 001-SC6 | 001 | **Drifted (High)** | — | clippy clean not verifiable via workspace (not a member) |
| 001-SC7 | 001 | **Drifted (High)** | — | cargo doc not verifiable via workspace (not a member) |
| 002-FR-001 | 002 | Aligned | tests/integration_ssd.rs:65-66 | real BlockDeviceSpdkNvmeComponent |
| 002-FR-002 | 002 | Aligned | tests/integration_ssd.rs:144-145 | PARTITION_INDEX=1 from DiskPartitionManager table |
| 002-FR-003 | 002 | Aligned | tests/integration_ssd.rs:227-267 | put validated |
| 002-FR-004 | 002 | Aligned | tests/integration_ssd.rs:238,252,266 | byte-for-byte compare |
| 002-FR-005 | 002 | Aligned | tests/integration_ssd.rs:307-351 | delete validated |
| 002-FR-006 | 002 | Aligned | tests/integration_ssd.rs:424-469 | iterate_all validated |
| 002-FR-007 | 002 | **Drifted (Medium)** | tests/integration_ssd.rs:198-213,357-387 | "persistence after force_flush": test uses internal flush_to_disk (flush_store helper), NOT force_flush (which is a no-op) |
| 002-FR-008 | 002 | Aligned | tests/integration_ssd.rs:227-267 | small/medium/max 128KiB sizes |
| 002-FR-009 | 002 | Aligned | tests/integration_ssd.rs:281-301 | overwrite replaces completely |
| 002-FR-010 | 002 | Aligned | tests/integration_ssd.rs:475-511 | bulk 500 entries |
| 002-FR-011 | 002 | **Drifted (High)** | tests/integration_ssd.rs:177-213,432,462 | "use standard interface, NOT internal APIs" violated: setup/persistence use initialize_from_client, snapshot_entries, mark_flushed, load_entries, flush::flush_to_disk |
| 002-FR-012 | 002 | Aligned | tests/integration_ssd.rs (asserts + eprintln) | per-test pass/fail + integrity eprintln |
| 002-SC-001 | 002 | Aligned | tests/integration_ssd.rs:227-301 | byte-for-byte round-trip |
| 002-SC-002 | 002 | Aligned | tests/integration_ssd.rs:357-387 | flushed entries survive restart (via internal flush) |
| 002-SC-003 | 002 | Aligned | tests/integration_ssd.rs:475-547 | no corruption asserted |
| 002-SC-004 | 002 | Aligned | tests/integration_ssd.rs:307-351 | delete non-retrievable + non-iterable |
| 002-SC-005 | 002 | Aligned | tests/integration_ssd.rs:475-511 | 500-entry bulk integrity |
| 002-SC-006 | 002 | Aligned | tests/integration_ssd.rs:235,249,263 | 3 distinct sizes (1B/4KiB/128KiB) |

## Detailed Findings

### 001-FR-05 — `force_flush()` is a no-op (High) [self-documented in spec Known Gaps]
`IExtendedMetadataStore::force_flush()` (`src/lib.rs:177-184`) logs and returns `Ok(())`
in every build configuration (default, `testing`, `spdk`). It never invokes
`flush::flush_to_disk` or `FlushManager::trigger_flush`. The interface doc comment
(`components/interfaces/src/iextended_metadata_store.rs:44`) states "Returns when all
data is durable" — a contract the implementation does not meet under `testing`/`spdk`.
A caller holding only the interface receives silent data loss on "flush". The spec's
own Known Gaps section (spec.md:175-177) and `.specify/sync/align-tasks.md` (ALIGN-002)
acknowledge this as a code defect to fix (not a spec change). Every persistence/SSD
test achieves durability by calling `flush::flush_to_disk`/`FlushManager::trigger_flush`
directly instead.

### 001-SC1/2/3/6/7 — crate absent from workspace (High) [self-documented]
Confirmed: `extended-metadata-store` appears nowhere in the root `Cargo.toml`
`[workspace] members` array. Consequently the 9 unit tests, persistence tests, SSD
tests, `cargo clippy -D warnings`, and `cargo doc --no-deps` cannot be exercised by
`cargo build` / `cargo test --all` / CI. The spec note (spec.md:251) and ALIGN-001
document this as a build-wiring defect. All the referenced tests do exist in the tree
and unit-test names/count (9) match the spec.

### 002-FR-011 — integration test relies on internal APIs (High)
FR-011 mandates the test use only the standard `IExtendedMetadataStore` interface.
put/get/delete/iterate_all do go through the interface, but store creation and all
persistence go through inherent/internal APIs: `initialize_from_client`
(integration_ssd.rs:188), `snapshot_entries`/`mark_flushed`/`flush::flush_to_disk`
(flush_store, 198-213), and `load_entries` (432,462). This is a direct consequence of
FR-05: since `force_flush()` does nothing, the test cannot obtain durability through
the interface and must bypass it.

### 002-FR-007 — persistence validated without force_flush (Medium)
US3 acceptance ("After force_flush()... re-initialize... entries retrievable") is
validated by `test_persistence_after_flush` (357-387), which calls `flush_store`
(internal `flush_to_disk`) rather than `force_flush()`. Correct data survives, but the
scenario as written (force_flush) is not the code path actually exercised.

### Capacity handling — `CapacityExhausted` never produced (Medium) [see conflicts]
`store.put()` never returns `ExtendedMetadataStoreError::CapacityExhausted`; it only
enforces `ValueTooLarge`. Capacity is enforced solely in `flush::flush_to_disk`
(`src/flush.rs:33-38`) as a `String` error ("exceeds region capacity"). Grep confirms
`CapacityExhausted` and `StorageError` are never constructed anywhere in `src/`. The
SSD test `test_capacity_exhaustion` (integration_ssd.rs:513-547) loops expecting
`put()` to return `CapacityExhausted`; that branch is unreachable, so the test passes
trivially without ever exercising capacity exhaustion (US5 scenario 2 / the "partition
too small" edge case are effectively unvalidated).

## Unspecced Code

| Item | Location | Notes |
|------|----------|-------|
| `Superblock::region_capacity_bytes()` | src/on_disk.rs:142-144 | public method, never called anywhere in src/ or tests/ (dead public API) |
| `create_test_component_from_state()` | src/test_support.rs:268-274 | public test helper, unused by any test |
| `ExtendedMetadataStoreError::{CapacityExhausted, StorageError}` | interfaces/src/iextended_metadata_store.rs:10-12 | public error variants provided by this component's interface but never returned by any code path in src/ |

## Conflicts / Spec-Reference Issues

1. **plan.md references nonexistent receptacles.** `002/plan.md` (line ~68) and
   `tasks.md` T007 state the store is wired via "IBlockDevice and IPartitionTable
   receptacles" and a `create_store_instance()` helper. The actual
   `ExtendedMetadataStoreComponent` declares only an `ILogger` receptacle
   (`src/lib.rs:44-46`); the test wires a manually-constructed `BlockDeviceClient`
   (`make_client`, integration_ssd.rs:164-173) and the helper is named `create_store`,
   not `create_store_instance`.
2. **CapacityExhausted contract mismatch** (see finding above): 002 US5 scenario 2 /
   edge cases and `test_capacity_exhaustion` assume the interface surfaces capacity
   exhaustion; the implementation surfaces it only as a `String` error inside
   `flush_to_disk`.
3. **Dev-dependency name mismatch.** `002/plan.md` and `tasks.md` T001 call for
   `console-logger`; `Cargo.toml:20` declares `logger` (and the test uses
   `logger::LoggerComponent`). Cosmetic.
4. **force_flush doc contract** (`interfaces/src/iextended_metadata_store.rs:44`)
   promises durability the no-op implementation does not deliver (mirror of FR-05).

## Recommendations

1. **Fix `force_flush()` (ALIGN-002)** — wire it to `flush_to_disk`/`FlushManager` when
   a `BlockDeviceClient` has been provided, or explicitly redefine the interface
   contract as hint-only. This unblocks 002-FR-007 and 002-FR-011.
2. **Add the crate to root `Cargo.toml` members (ALIGN-001)** so the 9 unit tests,
   persistence tests, clippy, and doc checks run in CI (unblocks 001-SC1/2/3/6/7).
3. **Enforce capacity via the interface** — have `put()` (or an interface-level flush)
   return `CapacityExhausted` so `test_capacity_exhaustion` and US5 are actually
   exercised, or update 002 spec to describe flush-time `String` capacity errors.
4. **Reconcile 002 plan/tasks** with reality: remove references to IBlockDevice/
   IPartitionTable receptacles and `create_store_instance`; fix `console-logger` →
   `logger`.
5. **Remove or wire up** the dead public `region_capacity_bytes()` and
   `create_test_component_from_state()`.
