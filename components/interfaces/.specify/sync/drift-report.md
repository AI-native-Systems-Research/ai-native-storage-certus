# Spec Drift Report — `interfaces`

**Generated**: 2026-07-22T22:41:32Z
**Spec**: `components/interfaces/specs/001-interfaces/spec.md` (FR-001..FR-028, backfilled + previously sync-applied through FR-027/028)
**Head**: `bb9569dde029cc7cd98306e88f7904b8cd4cdbee` (branch `feat/speckit-backfill-remaining-components`)
**Code**: `components/interfaces/src/**`, `Cargo.toml`, `README.md`

## Summary

| Metric | Count |
|--------|-------|
| Specs analyzed | 1 |
| Requirements checked (FR-001..FR-028) | 28 |
| Aligned | 17 |
| Drifted | 8 |
| Not implemented | 3 |
| Unspecced features | 8 |
| Conflicts | 0 |

The spec was last brought up to date for the `IGpuServices` multi-GPU routing
methods and the dispatcher cold-load staging fields (see prior
`drift-report.md`/`apply-report.md` in this directory, applied against an
earlier `HEAD`). Since that pass, the crate has grown substantially:
`zyre`, `remote-lookup-rdma-initiator`, and `remote-lookup-rdma-responder`
landed as brand-new interfaces; `IDispatchMap`, `IDispatcher`, `IMemoryTier`,
and `IBlockDevice` each gained new methods; `IRemoteLookup` was reshaped for a
002 rewrite; and one committed source file (`iextended_metadata_store.rs`) was
never wired into the crate at all.

## Per-Spec Classification

### `components/interfaces/specs/001-interfaces/spec.md`

| FR | Subject | Status | Detail |
|----|---------|--------|--------|
| FR-001 | IGreeter | Aligned | `greeting_prefix` unchanged. `src/igreeter.rs:5`. |
| FR-002 | ILogger | Aligned | `error/warn/info/debug` unchanged. `src/ilogger.rs:5-8`. |
| FR-003 | ISPDKEnv | Aligned | `src/ispdk_env.rs:9-25`. |
| FR-004 | IBlockDevice | **Drifted** | Actual interface has an 11th method, `read_write_stats(&self) -> ReadWriteStats`, not listed. `src/iblock_device.rs:494`. New `ReadWriteStats` type (`:134-176`) also absent from FR-017. |
| FR-005 | IBlockDeviceAdmin | Aligned | `src/iblock_device.rs:506-538`. |
| FR-006 | IEvictionPolicy | Aligned | `src/ieviction_policy.rs:54-80`. |
| FR-007 | IDispatchMap | **Drifted** | Two methods exist beyond the spec's 16: `promote_block_to_memory_tier(key, pointer, size)` (`src/idispatch_map.rs:234-239`) and `try_evict_to_block(key)` (`:262`). Together they implement an in-place block↔memory-tier promotion/demotion path (preserving ref counts and eviction handle) that the spec does not describe. |
| FR-008 | IDispatcher | **Drifted** | Three methods exist beyond the spec's 16: `pin(key)` / `unpin(key)` (eviction-protection references, `src/idispatcher.rs:546,557`) and `read_write_stats(&self) -> ReadWriteStats` (`:652`, cumulative SSD telemetry). |
| FR-009 | IMemoryTier | **Drifted** | `telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot` (`src/imemory_tier.rs:206`) is not listed; `MemoryTierTelemetrySnapshot` (`:12`) is exported from `lib.rs` but has no FR entry (spec's supporting-types section, FR-019, mentions only `MemoryTierError`). |
| FR-010 | IExtentManager | Aligned | All 14 methods match. `src/iextent_manager.rs:197-260`. |
| FR-011 | IGpuServices | Aligned | 23 methods incl. `set_device`/`device_of_ptr` match the (previously backfilled) spec text exactly. `src/igpu_services.rs:271-782`. |
| FR-012 | IRemoteLookup | **Drifted** | Spec describes `batch_lookup(&self, entries: &[(CacheKey, IpcHandle)])`; actual signature is `batch_lookup(&self, entries: &[(CacheKey, u32)])` — a size hint, not a GPU IPC handle (`src/iremote_lookup.rs:141-144`), consistent with remote-lookup now being CPU/DRAM-only. A new `initialize(&self, config: LookupConfig) -> Result<(), RemoteLookupError>` method (`:118`) is entirely unlisted, as is the `LookupConfig` type (`:27-59`, 10 fields incl. zyre group, quorum %, phase timeouts, gossip discovery). This is the aftermath of the "002 remote-lookup rewrite" (commit `9516f2d`). |
| FR-013 | IRemoteRequestHandler | **Not implemented** | The interface (`handle_lookup`, `handle_check`, `handle_batch_lookup`, `release_lookup`) does not exist anywhere in the crate. It was removed by commit `29902a2` ("refactor: split remote-request-handler into remote-lookup-rdma-{initiator,responder}") and replaced by the unspecced `IRemoteLookupRdmaInitiator`/`IRemoteLookupRdmaResponder` pair (see Unspecced table). |
| FR-014 | IExtendedMetadataStore | **Not implemented (unreachable)** | `src/iextended_metadata_store.rs` exists with a full, correct implementation of `put/get/delete/iterate_all/force_flush` (added in commit `9f1f6d5`), but `src/lib.rs` has no `mod iextended_metadata_store;` declaration and no re-exports — the trait and its error type are not part of the compiled `interfaces` crate at all. The consuming component `components/extended-metadata-store` (`use interfaces::{ExtendedMetadataStoreError, IExtendedMetadataStore, ILogger};`, `src/lib.rs:21`) additionally is **not listed in the workspace `members`** in the root `Cargo.toml`, so neither crate builds today. This is a broken integration, not merely a doc-drift. |
| FR-015 | IPartitionTable | Aligned | `src/ipartition_table.rs:118-127`. |
| FR-016 | Supporting Types — SPDK | Aligned | `SpdkEnvError` 7 variants, `BlockDeviceError` 10 variants — both match. `src/spdk_types.rs`. |
| FR-017 | Supporting Types — Block Device | **Drifted** | `Command` now has 12 variants (spec says 11) and `Completion` now has 11 (spec says 10) — `src/iblock_device.rs:241-436`. Both variant counts have been off since the file's initial commit (not a new regression), but the spec text is currently inaccurate. `ReadWriteStats` (see FR-004) is also missing from this FR. |
| FR-018 | Supporting Types — Dispatcher | **Drifted** | Spec (last amended to) "16-field configuration"; `DispatcherConfig` actually has **18 fields** (`src/idispatcher.rs:26-88`). Four fields undocumented: `memory_tier_eviction_threshold`, `memory_tier_eviction_low_watermark`, `memory_tier_eviction_batch_size`, `memory_tier_eviction_interval_secs` (DRAM→SSD proactive demotion sweep) and `extended_metadata_partition_size`. |
| FR-019 | Supporting Types — Memory Tier | Aligned | `MemoryTierError` 7 variants match. `src/imemory_tier.rs:20-32`. (Method/type additions tracked under FR-009.) |
| FR-020 | Supporting Types — Eviction Policy | Aligned | `EvictionHandle`, `EvictionPolicyError` (2 variants), `PoolId`. `src/ieviction_policy.rs`. |
| FR-021 | Supporting Types — Extent Manager | Aligned | `src/iextent_manager.rs`. |
| FR-022 | Supporting Types — GPU Services | Aligned | `src/igpu_services.rs`. |
| FR-023 | Supporting Types — Remote | **Drifted (partially removed)** | `RemoteLookupError` (2 variants) still matches (`src/iremote_lookup.rs:80-85`). `LookupRef` and `RemoteRequestHandlerError` no longer exist anywhere in the crate — removed along with FR-013's `IRemoteRequestHandler`. |
| FR-024 | Supporting Types — Partition Table | Aligned | `src/ipartition_table.rs`. |
| FR-025 | Supporting Types — Extended Metadata Store | **Not implemented (unreachable)** | `ExtendedMetadataStoreError` (4 variants) is correctly defined in `src/iextended_metadata_store.rs:6-16` but, per FR-014, the module is not compiled into the crate. |
| FR-026 | Supporting Types — Dispatch Map | Aligned | `DispatchMapError` has exactly 11 variants as specified. `src/idispatch_map.rs:39-62`. (New methods using it tracked under FR-007.) |
| FR-027 | IDispatcher Cold-Load Staging Configuration | Aligned | `cold_staging_slots` (default 64), `cold_staging_buf_bytes` (default 4 MiB). `src/idispatcher.rs:81-87,109-110`. |
| FR-028 | IGpuServices Multi-GPU Device Routing | Aligned | `set_device`, `device_of_ptr`. `src/igpu_services.rs:555,577`. |

## Unspecced Code

| Feature | Location | Suggested Spec Treatment |
|---------|----------|---------------------------|
| `IZyre` (factory) + `IZyreNode` (handle) interfaces, plus `PeerId`, `ZyreEvent`, `NodeConfig`, `GossipConfig`, `ZyreError` — LAN/gossip peer discovery and messaging (zyre/ZeroMQ bindings) | `src/izyre.rs:1-438` | New FR (e.g. FR-029) documenting the zyre interface pair as a new user story ("Peer Discovery and Messaging"). |
| `IRemoteLookupRdmaInitiator` interface + `RemoteRegion`, `PushStatus`, `RemoteLookupRdmaInitiatorError` — outbound RDMA push of local cache values to remote hosts | `src/iremote_lookup_rdma_initiator.rs:1-170` | New FR replacing part of the removed FR-013; note this is the "push" (serving) side. |
| `IRemoteLookupRdmaResponder` + `IRemoteLookupRdmaResponderAdmin` interfaces + `ControlChannel`, `Endpoint`, `LocalRegion`, `RemoteLookupRdmaResponderError`, `ResponderCommand`, `ResponderEvent` — accept side of RDMA lookups (control-plane only; writes are one-sided) | `src/iremote_lookup_rdma_responder.rs:1-285` | New FR replacing the other part of the removed FR-013; note the teardown-before-reclaim handshake. |
| `LookupConfig` type + `IRemoteLookup::initialize(config)` method | `src/iremote_lookup.rs:9-118` | Fold into amended FR-012. |
| `IDispatchMap::promote_block_to_memory_tier` / `try_evict_to_block` | `src/idispatch_map.rs:234-262` | Fold into amended FR-007. |
| `IDispatcher::pin` / `unpin` (eviction-protection ref-counting) | `src/idispatcher.rs:546,557` | Fold into amended FR-008. |
| `IDispatcher::read_write_stats` / `IBlockDevice::read_write_stats` + `ReadWriteStats` type | `src/idispatcher.rs:652`, `src/iblock_device.rs:134-176,494` | Fold into amended FR-004/FR-008/FR-017. |
| `IMemoryTier::telemetry_snapshot` + `MemoryTierTelemetrySnapshot` type | `src/imemory_tier.rs:12-19,206` | Fold into amended FR-009/FR-019. |

## Conflicts

None found between specs (only one spec, `001-interfaces`, exists for this component).

## Recommendations

1. **Fix the broken integration first (severity: critical).** Add `mod iextended_metadata_store;` plus the corresponding `pub use` lines to `src/lib.rs`, and add `"components/extended-metadata-store"` to the root `Cargo.toml` workspace `members`. Until both are done, FR-014/FR-025 describe code that cannot be built, and `components/extended-metadata-store` is dead weight in the tree.
2. **Backfill the three new interface modules** (`izyre`, `iremote_lookup_rdma_initiator`, `iremote_lookup_rdma_responder`) as new FRs/user stories — they are substantial, tested, documented interfaces with no spec coverage at all.
3. **Amend FR-012/FR-013/FR-023** to reflect the 002 remote-lookup rewrite: replace the removed `IRemoteRequestHandler`/`LookupRef`/`RemoteRequestHandlerError` narrative with the new initiator/responder split, and update `IRemoteLookup`'s `batch_lookup` signature and `initialize`/`LookupConfig` addition.
4. **Amend FR-004, FR-007, FR-008, FR-009** to add the newly-shipped methods (`read_write_stats`, `promote_block_to_memory_tier`, `try_evict_to_block`, `pin`, `unpin`, `telemetry_snapshot`) and their supporting types (`ReadWriteStats`, `MemoryTierTelemetrySnapshot`).
5. **Correct FR-017 and FR-018 counts**: `Command` is 12 variants, `Completion` is 11, `DispatcherConfig` is 18 fields (list the 4 memory-tier-eviction fields and `extended_metadata_partition_size` explicitly).
