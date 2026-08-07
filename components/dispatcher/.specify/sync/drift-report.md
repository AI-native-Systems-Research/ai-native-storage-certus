# Spec Drift Report — dispatcher

Generated: 2026-08-07T15:31:21Z
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Functional Requirements (active, excl. 5 removed) | 50 |
| Success Criteria (active, excl. SC-008 removed) | 14 |
| ✓ Aligned | 61 |
| ⚠️ Drifted | 3 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 3 groups |

Overall the implementation closely tracks the spec: every functional data-path,
eviction, and lifecycle requirement is present in code. The drift is confined to
(a) the interface-method inventory in FR-001, (b) the `batch_lookup` signature in
FR-039, and (c) the `create_eviction_channel` signature in FR-042 — all cases where
the spec text lags a since-evolved API. Plus unspecced config fields, injection
hooks, and the reserve/copy/complete primitive set, and an interface doc block that
references a nonexistent Creusot `verif/` proof tree.

## Detailed Findings

### Drifted

- **FR-001 — Interface method inventory** — severity: moderate.
  Spec lists the `IDispatcher` methods as `initialize, shutdown, lookup, lookup_async,
  batch_lookup, check, remove, populate, touch, promote_to_memory_tier`.
  The actual interface (`components/interfaces/src/idispatcher.rs:200-668`) additionally
  defines: `reserve_memory` (`:501`), `copy_gpu_to_memory_async` (`:526`),
  `copy_gpu_to_memory_completed` (`:540`), `release_memory` (`:549`), `pin` (`:561`),
  `unpin` (`:572`), `flush_to_ssd` (`:660`), `read_write_stats` (`:667`). (`clear_memory_tier`
  is separately covered by FR-038.) These 8 methods are implemented
  (`components/dispatcher/src/lib.rs:2817-3266`) but absent from FR-001's enumerated list.

- **FR-039 — `batch_lookup` signature** — severity: moderate.
  Spec: `batch_lookup(entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>`.
  Code: `batch_lookup(entries: &[(CacheKey, Vec<IpcHandle>)])`
  (`idispatcher.rs:377-380`, impl `lib.rs:2009-2012`). Each key now carries a *vector* of
  IPC regions (multi-region scatter for vLLM 0.23+ per-layer allocations); the single-handle
  form in the spec is stale. FR-039 step (2) also says hot-path copies use "the warm stream
  (FR-037)", but FR-052 supersedes this with per-device streams (internal, minor).

- **FR-042 — `create_eviction_channel` signature** — severity: minor.
  Spec: `create_eviction_channel() -> crossbeam_channel::Receiver<EvictionEvent>` (no arg).
  Code: `create_eviction_channel(&self, capacity: usize) -> Receiver<EvictionEvent>`
  (`lib.rs:378-385`). The channel is bounded by the caller-supplied `capacity`; the spec
  omits the parameter. Drop-and-count semantics and `eviction_dropped_count()` (`lib.rs:388`)
  match the spec otherwise.

### Aligned (representative evidence; all other active FR/SC verified present)

- FR-002 `DispatcherError` variants — `idispatcher.rs:151-167` (7 variants covering all modes).
- FR-003/004/005 populate + write-through + slot retention — `lib.rs:2770`; `peek()`/`convert_to_storage` in `background.rs`.
- FR-006/007 lookup MemoryTier/BlockDevice + miss — `lib.rs:1982` → `lookup_async` `:2625`; `memcpy_h2d_async` warm-stream path present.
- FR-008 check — `lib.rs:2702`. FR-009 remove — `lib.rs:2723`.
- FR-010 `define_component!` single interface; FR-011/012 receptacles + `initialize` validation — `lib.rs:1642`; `poller_base_cpu`→`set_actor_cpu` at `lib.rs:1319,1370`; `IRemoteLookup` receptacle `lib.rs:243`, `remote_probe.rs`.
- FR-014 shutdown drain + extent-manager `checkpoint()` — `lib.rs:1892`. FR-015/016 N block devices + extent managers + FormatParams.
- FR-017 silent write-through drop; FR-018 non-blocking remove; FR-019 MDTS segmentation + zero-copy pipelines (`pipeline.rs:9,14`, `max_queue_depth` `:33`).
- FR-023 touch — `lib.rs:3042`. FR-024 pin-safe `evict_for_space`/`evict_one_clean` + `MAX_SCAN` — `lib.rs:849,897`; `try_evict_to_block` `pins.rs:10`.
- FR-025 `format_on_init` recovery + `for_each_extent`/`recover_extent` — `lib.rs:1678,1679`.
- FR-028 promotion re-register; FR-029..033 SSD evictor + all `DispatcherConfig` eviction/staging fields present — `idispatcher.rs:34-111`.
- FR-034 `register_host_memory` + per-drive `ChannelPool`/`checkout` — `lib.rs:112`, `pipeline.rs:226`; FR-035 `unregister_host_memory` `lib.rs:1927`.
- FR-036 `lookup_async` returning `GpuStream`; FR-037 warm stream + `DEVICE_STREAMS`/`device_streams_for` — `lib.rs:253,287`, `cold_pool.rs:192`.
- FR-038 `clear_memory_tier` — `lib.rs:3208`. FR-040/041 `promote_to_memory_tier` + `pipelined_ssd_to_dram_only`/`_multi_` — `lib.rs:3060`, `pipeline.rs:784,897`.
- FR-043 `PipelineMetrics` — `remote_probe.rs:17`, `metrics.rs`. FR-044 `ColdReadPool` — `cold_pool.rs:45`. FR-045 remote-lookup batch fallback — `remote_probe.rs`.
- FR-046..050 memory-tier evictor + quadratic pressure + `try_evict_to_block` + `EvictionEvent` — `background.rs:11`.
- FR-051 `serve_concurrently_promoted` — `lib.rs:1107`. FR-052 per-device streams + `device_of_ptr` — `pipeline.rs:620`.
- FR-053 `StagingPool`/`StagingLease`/`serve_cold_staged` — `pipeline.rs:75,77`, `lib.rs:718`.
- FR-054 drain-all-no-early-break + `stop_submitting` — `pipeline.rs:308`. FR-055 `EXPECTED_PARTITIONS` guard — `lib.rs:1531`.
- SC-001..015 (excl. SC-008 removed) exercised by tests in `lib.rs` (`populate_*`, `lookup_*`, `remove_*`, `batch_lookup_recovers_from_concurrent_promotion_race` `:4897`, `populate_triggers_eviction_on_full_pool` `:5393`) and `tests/`.

### Not Implemented

None. All active requirements have corresponding implementation.

## Unspecced Code

| Item | Location | Notes |
|------|----------|-------|
| `reserve_memory(key,size,session_id)`, `copy_gpu_to_memory_async(regions,stream)`, `copy_gpu_to_memory_completed`, `release_memory` | `idispatcher.rs:501-549`; impl `lib.rs:2817-2995` | reserve→copy→complete primitive set + `session_id` field; no FR describes them (US6/FR-020..022 direct-store workflow was *removed*, but these replacement primitives were never re-specced). |
| `pin` / `unpin` eviction-protection refs | `idispatcher.rs:561,572`; impl `lib.rs:3010,3027` | Ref-count pin API; unspecced. |
| `flush_to_ssd`, `read_write_stats` | `idispatcher.rs:660,667`; impl `lib.rs:3241,3256` | Barrier + telemetry; unspecced. |
| Config `metadata_partition_size`, `extended_metadata_partition_size` | `idispatcher.rs:62-67`; used `lib.rs:1500,1505` | Not listed in FR-033; drive partition sizing. |
| Config `backfill_delay_ms` | `idispatcher.rs:57-61` | Belongs to dispatcher-p2p (its FR-014); present but unused by the standard dispatcher — shared `DispatcherConfig`. |
| `set_block_device_factory`, `set_extent_manager_factory`, `set_pipeline_metrics` | `lib.rs:360-372` | Public test/DI injection hooks on the component struct; not in spec. |

## Conflicts / Spec-Referencing-Nonexistent

- **Nonexistent Creusot proof tree.** `idispatcher.rs:185-198` documents "Verified Properties (see `components/dispatcher/verif/`)" claiming P1–P10 and "10 properties, 24 verification conditions discharged by SMT solvers", and per-method `# Verified: Pn` doc tags throughout. The directory `components/dispatcher/verif/` does **not exist**. Either the proofs were removed or never committed; the interface doc overstates the current verification state.

## Recommendations

1. Update FR-001 to enumerate the full current interface, or fold the reserve/copy/complete/pin/unpin/flush/stats primitives into new FRs (backfill).
2. Fix FR-039 to `&[(CacheKey, Vec<IpcHandle>)]` and describe the multi-region scatter contract.
3. Fix FR-042 to include the `capacity: usize` parameter.
4. Add FR coverage (or a config-fields FR) for `metadata_partition_size` / `extended_metadata_partition_size`; note `backfill_delay_ms` is p2p-only in the shared config.
5. Reconcile the `verif/` proof references in `idispatcher.rs`: restore the proof tree or soften the doc claims to match reality.
