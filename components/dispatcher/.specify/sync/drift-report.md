# Spec Drift Report — dispatcher

Generated: pending
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (51 active FR + 14 active SC) | 65 |
| ✓ Aligned | 65 |
| ⚠️ Drifted | 3 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 1 group |

Removed/superseded and therefore excluded from the active count: FR-020, FR-021,
FR-022 (prepare/commit/cancel_store direct-write workflow), FR-026, FR-027 (block
device / extent manager version selection), SC-008, and User Story 6.

The 2026-08-07 sync sweep resolved the three API-signature drifts that the prior
report flagged (FR-001 method inventory, FR-039 `Vec<IpcHandle>` signature, FR-042
`capacity` argument), backfilled the reserve/copy/complete/pin/unpin/flush/stats
primitives as FR-056, extended FR-033 with the partition-size and `backfill_delay_ms`
fields, and softened the nonexistent-Creusot-`verif/` claim in `idispatcher.rs`. All
of those are now aligned. Every active functional-requirement data path, eviction
path, lifecycle step, and success criterion is present and matches code. The residual
drift is (a) an **internal spec contradiction** on the batch cold-promotion queue
depth (User Story 11 narrative vs FR-039 / code), and (b) two stale path/name
references in the component `CLAUDE.md` after the `components/ → lib/` crate move.

## Detailed Findings

### ⚠️ Drifted

- **User Story 11 (Parallel Batch Cold Promotion) — per-thread queue depth** — severity: moderate.
  US11 narrative states: *"Each thread uses a reduced NVMe pipeline depth (`16 / num_queues`) to
  share the drive's submission queue capacity without overflow"*, and scenario 3 states
  *"each processed by a separate thread with `max_queue_depth = 8` (16/2), keeping total per-drive
  NVMe commands at ≤16."*
  Actual: `batch_lookup` sets `let queue_depth = 128;` per queue thread
  (`components/dispatcher/src/lib.rs:2217`) and passes it into `pipelined_multi_object_zero_copy`
  (`lib.rs:2318,2345`). This is a *deliberate* value — FR-039 step (5) and FR-019 both correctly
  say *"`max_queue_depth = 128` per thread"*. So US11's "16/num_queues (=8), ≤16 per drive" text is
  stale and directly contradicts FR-039/FR-019 within the same spec. The requirement (FR-039) is
  aligned; the user-story acceptance text is not.

- **CLAUDE.md stale crate path (`component-framework`)** — severity: minor.
  `components/dispatcher/CLAUDE.md:40` says *"`component-framework`, `component-core`,
  `component-macros` — at `../../component-framework/crates/`"*. After the repo move,
  component-framework now lives at `lib/component-framework` (verified: `lib/component-framework`
  exists, `components/component-framework` does not). The Cargo.toml no longer uses a literal path
  (`component-framework.workspace = true`, `components/dispatcher/Cargo.toml:9`), so the build is
  unaffected, but the doc path is wrong. `CLAUDE.md:41-42` (`../../interfaces`, `../../spdk-env`)
  remain correct (both still under `components/`).

- **CLAUDE.md stale crate names (`-v2` suffix)** — severity: minor.
  `components/dispatcher/CLAUDE.md:43-44,53` reference `block-device-spdk-nvme-v2` and
  `extent-manager-v2`. The actual dependency crate is `block-device-spdk-nvme`
  (`components/dispatcher/Cargo.toml:15`); there is no `-v2` suffix in the current workspace.
  Documentation-only drift.

### ✓ Aligned (representative evidence; all other active FR/SC verified present)

- FR-001 full `IDispatcher` inventory — `components/interfaces/src/idispatcher.rs:211-680` (all
  listed lifecycle/read/write/lifecycle-primitive/introspection methods present); note
  `create_eviction_channel`/`eviction_dropped_count` correctly documented as inherent methods on
  `DispatcherComponent` (`lib.rs:378,388`), not trait methods.
- FR-002 `DispatcherError` (7 variants) — `idispatcher.rs:151-167`.
- FR-003/004/005 populate + `IMemoryTier::insert`/`peek` write-through + slot retention —
  `lib.rs:2770`; write-through worker in `background.rs`.
- FR-006/007 lookup MemoryTier/BlockDevice + miss; warm-stream H2D — `lib.rs:1982` delegating to
  `lookup_async` (`lib.rs:2625`); per-device resolve via `set_batch_device` (`lib.rs:2653`).
- FR-008 check (`lib.rs:2702`); FR-009 remove (`lib.rs:2723`); FR-018 non-blocking remove.
- FR-010 `define_component!`; FR-011 receptacles incl. `IRemoteLookup` (`(key, size)` batch,
  `lib.rs:4209`) + `poller_base_cpu` → `set_actor_cpu`; FR-012 initialize validates
  dispatch_map+memory_tier (`lib.rs:1642`).
- FR-014 shutdown drain + extent-manager `checkpoint()` (`lib.rs:1892`); FR-015/016 N drives + N
  extent managers + FormatParams.
- FR-017 silent write-through drop; FR-019 MDTS + `max_queue_depth`; single-entry `promote_and_serve`
  uses `16` (`lib.rs:673-682`), `batch_lookup` uses `128` (`lib.rs:2217`).
- FR-023 touch (`lib.rs:3042`); FR-024 pin-safe `evict_for_space`/`evict_one_clean` +
  `MAX_SCAN` + `evict_and_insert` fallback (`lib.rs:849,889,942`); `target_key` unused.
- FR-025 `format_on_init` recovery via `for_each_extent`/`recover_extent` (`lib.rs:1642` block).
- FR-028 promotion re-register; FR-029..033 SSD evictor + all `DispatcherConfig` fields incl.
  `metadata_partition_size`, `extended_metadata_partition_size`, `backfill_delay_ms`
  (`idispatcher.rs:26-113`).
- FR-034 `register_host_memory` + per-drive `ChannelPool`/`checkout`/`ChannelLease`
  (`lib.rs:124,139,159`); FR-035 `unregister_host_memory` (shutdown, `lib.rs:1933` region).
- FR-036 `lookup_async` → `GpuStream`; FR-037 `warm_stream` `AtomicU64` (`lib.rs:253`) +
  `DEVICE_STREAMS`/`device_streams_for` (`lib.rs:287,294`).
- FR-038 `clear_memory_tier` (`lib.rs:3208`); FR-040 `promote_to_memory_tier` (`lib.rs:3060`);
  FR-041 `pipelined_ssd_to_dram_only`/`pipelined_multi_ssd_to_dram_only`
  (`pipeline.rs:784,897`).
- FR-042 `create_eviction_channel(capacity)` + `emit_eviction`/`eviction_dropped_count` +
  bounded `try_send` drop-count (`lib.rs:378,392-396`); emitted from `evict_one_clean`
  (`lib.rs:859,868`).
- FR-043 `PipelineMetrics` trait (`metrics.rs:12`) + injectable `set_pipeline_metrics`;
  FR-044 `ColdReadPool` (`cold_pool.rs:45`); FR-045 remote-lookup merge (`remote_probe.rs`).
- FR-046..050 memory-tier evictor + quadratic pressure + `try_evict_to_block` + `EvictionEvent`
  (`background.rs:425` region, `lib.rs:859,868`).
- FR-051 `serve_concurrently_promoted` (`lib.rs:1107`); FR-052 per-device streams +
  `device_of_ptr`/`set_batch_device` (`lib.rs:294,328`), `ColdReadRequest.gpu_device`
  (`cold_pool.rs:28,189`).
- FR-053 `StagingPool`/`StagingLease`/`serve_cold_staged` (`pipeline.rs:75,141`, `lib.rs:718`);
  staging fallback in `promote_and_serve` (`lib.rs:588-606`).
- FR-054 drain-all/no-early-break + `stop_submitting` (`pipeline.rs`); FR-055 `EXPECTED_PARTITIONS`
  guard (`lib.rs:1531-1535`).
- FR-056 reserve/copy/complete/release/pin/unpin + flush/stats — `lib.rs:2817,2862,2913,2995,3010,3027,3241,3256`.
- SC-001..015 (excl. SC-008) exercised by tests in `lib.rs`
  (`populate_*`, `lookup_*`, `remove_*`, `evict_for_space_*`, `populate_triggers_eviction_on_full_pool`,
  `batch_lookup_recovers_from_concurrent_promotion_race:4892`) and `tests/{integration,lazy_migration,reserve_memory_tests}.rs`.

### ✗ Not Implemented

None. All active requirements have corresponding implementation.

## Unspecced Code

| Feature | Location | Lines | Suggested spec |
|---------|----------|-------|----------------|
| Dependency-injection / test hooks: `set_block_device_factory`, `set_extent_manager_factory`, `set_pipeline_metrics` | `components/dispatcher/src/lib.rs` | 360-372 | Public inherent methods on `DispatcherComponent` for injecting mock block-device / extent-manager factories and a `PipelineMetrics` impl. Not referenced by any FR (FR-043 covers the metrics *trait* but not the injection setter). Add a short "test/DI surface" note, or mark them `#[cfg(test)]`-only if not intended as public API. |

## Recommendations

1. Fix the intra-spec contradiction: update User Story 11's narrative and scenario 3 to
   `max_queue_depth = 128` per thread (matching FR-039 / FR-019 and `lib.rs:2217`), or delete the
   stale "16 / num_queues" / "≤16 per drive" wording.
2. Update `components/dispatcher/CLAUDE.md:40` to `lib/component-framework` and correct the
   `-v2` crate names (`block-device-spdk-nvme`, `extent-manager`) at lines 43-44, 53.
3. Optionally add a one-line FR (or note under FR-043) covering the three DI/test factory hooks, or
   gate them behind `#[cfg(test)]`.
