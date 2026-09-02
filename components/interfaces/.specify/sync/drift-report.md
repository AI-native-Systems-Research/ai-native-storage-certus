---
spec_sync_component: interfaces
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:52:26Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 81fcdba841429386633af524019c51b821d9e504408556a01c3f522c2d58834e
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

# Drift Report: interfaces

**Generated**: 2026-09-02T21:52:26Z
**Project**: interfaces

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 47 |
| Aligned | 42 |
| Drifted | 5 |
| Not Implemented | 0 |
| Unspecced Features | 1 |

Spec: `001-interfaces` — Shared Interface Trait Definitions (34 FR + 6 NFR + 7 SC).
"Implementation" here means the trait/type definitions themselves, verified
against the actual source in `components/interfaces/src/`.

**Since the last sync (2026-08-07):** the previously-deferred FR-014/FR-025
`IExtendedMetadataStore` orphaned-module drift is **RESOLVED** — the module is
now declared and re-exported in `src/lib.rs` (lines 78, 100). The stale "Deferred
(not applied)" note at the top of the spec no longer reflects the code and is
corrected in this pass. Five requirements drifted where the code grew ahead of
the spec, and one entirely new interface (`IIpcServer`) is unspecced **and**
repeats the exact orphaned-module pattern that FR-014 used to have.

## Detailed Findings

### Spec 001-interfaces — Shared Interface Trait Definitions

**Resolved since last sync ✓** (previously drifted, now aligned)
- FR-014 `IExtendedMetadataStore` interface — the trait (put/get/delete/iterate_all/force_flush) is defined at `src/iextended_metadata_store.rs:30` and is now **declared and re-exported**: `src/lib.rs:78` (`mod iextended_metadata_store;`, ungated) and `src/lib.rs:100` (`pub use iextended_metadata_store::{ExtendedMetadataStoreError, IExtendedMetadataStore};`). The consumer `extended-metadata-store` (in-workspace) can now resolve `interfaces::IExtendedMetadataStore`. Spec FR-014 body matches; only the top-of-file "Last Synced 2026-08-07 … Deferred (not applied)" note is stale.
- FR-025 `ExtendedMetadataStoreError` (4-variant: NotFound, StorageError, CapacityExhausted, ValueTooLarge) — defined `src/iextended_metadata_store.rs:5` and now exported via the same `pub use` at `src/lib.rs:100`. Aligned.

**Aligned ✓** (spot-verified against source)
- FR-001..FR-005 (IGreeter/ILogger/ISPDKEnv/IBlockDevice/IBlockDeviceAdmin method lists) — verified against `src/igreeter.rs`, `ilogger.rs`, `ispdk_env.rs`, `iblock_device.rs`.
- FR-006/FR-020 `IEvictionPolicy::track(..., semantics: BlockSemantics)`, `BlockSemantics`/`SessionId` — `src/ieviction_policy.rs`; `EvictionPolicyError` 2-variant.
- FR-007/FR-026 `IDispatchMap` full method list incl. `set_checksum`/`get_checksum` behind `integrity-check` (`src/idispatch_map.rs:205-214`); `DispatchMapError` 11-variant (`src/idispatch_map.rs:39-62`).
- FR-008 (partial) — `DispatcherConfig` 18-field matches exactly (`src/idispatcher.rs:26-88`); `DispatcherError` 7-variant matches; `pin`/`unpin`/`read_write_stats` present and documented. (Method-signature/coverage drift below.)
- FR-009/FR-019 `IMemoryTier` 17-method list + `MemoryTierTelemetrySnapshot` (3-field, Copy+Default) + `MemoryTierError` 7-variant — `src/imemory_tier.rs`.
- FR-010 `IExtentManager` 14-method list + `ExtentManagerError` 5-variant — `src/iextent_manager.rs`.
- FR-011/FR-022/FR-028 `IGpuServices` 24 methods incl. `set_device`/`device_of_ptr` + GPU types — `src/igpu_services.rs`.
- FR-012 `IRemoteLookup` (initialize/batch_lookup/join_cluster/leave_cluster) — `src/iremote_lookup.rs`.
- FR-013 superseded pointer — `IRemoteRequestHandler` confirmed absent from source.
- FR-015/FR-024 `IPartitionTable` + partition types — `src/ipartition_table.rs`.
- FR-016 SPDK supporting types (PciAddress/PciId/VfioDevice/DmaBuffer/DmaAllocFn/env flags, SpdkEnvError 7-variant, BlockDeviceError 10-variant) — `src/spdk_types.rs`.
- FR-027 cold-load staging config — matches `DispatcherConfig` fields.
- FR-029/FR-032 `IZyre`/`IZyreNode` + Zyre types (ZyreError 7, ZyreEvent 9, NodeConfig 9-field #[non_exhaustive], GossipConfig 2-field, PeerId) — `src/izyre.rs`.
- FR-030/FR-033 RDMA initiator (push/connect/disconnect/disconnect_all/set_local_peer_id/push_async; RemoteRegion, PushStatus 4-variant, PushCompletion, RemoteLookupRdmaInitiatorError 2-variant) — `src/iremote_lookup_rdma_initiator.rs`.
- FR-031/FR-034 RDMA responder (RemoteLookupRdmaResponderError 6-variant, Endpoint, LocalRegion, ControlChannel, ResponderCommand 1-variant, ResponderEvent 3-variant) — `src/iremote_lookup_rdma_responder.rs`.
- NFR-001..006, SC-1..7 — feature gating, Send/Sync, error-trait, doc conventions consistent with source.

**Drifted ⚠️** (code ahead of spec; spec text stale)

- **FR-008** `IDispatcher` — method-signature and coverage drift — **moderate**
  - **D1 (moderate)** `batch_lookup` signature: spec says `&[(CacheKey, IpcHandle)]` (spec.md:175); actual is `&[(CacheKey, Vec<IpcHandle>)]` — each key now carries one *or more* GPU destination regions (per-layer allocations scattered across N regions). `src/idispatcher.rs:338-341`.
  - **D2 (moderate)** `copy_gpu_to_memory_async` signature: spec says `ipc_handle: IpcHandle` (spec.md:180); actual is `regions: &[IpcHandle]` — N regions gathered contiguously into one slot. `src/idispatcher.rs:460`.
  - **D3 (minor)** `batch_populate(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` exists in code but is absent from FR-008. `src/idispatcher.rs:427`.
  - **D4 (minor)** `tier_event_stats(&self) -> crate::TierEventStats` exists in code but is absent from FR-008. `src/idispatcher.rs:575`.
- **FR-017** `Supporting Types - Block Device` — enum-variant and struct-shape drift — **minor**
  - **D5 (minor)** `Command` documented as "12-variant" (spec.md:311); actual is **13** — adds `FlushSync { ns_id: u32 }`. `src/iblock_device.rs:322` (enum), `:411` (variant).
  - **D6 (minor)** `Completion` documented as "11-variant" (spec.md:312); actual is **12** — adds `FlushDone { handle: OpHandle, result: Result<(), NvmeBlockError> }`. `src/iblock_device.rs:439` (enum), `:501` (variant).
  - **D7 (minor)** `ReadWriteStats` gained per-transfer-size histograms `read_size_buckets`/`write_size_buckets: [u64; IO_SIZE_BUCKETS]` plus `size_bucket`/`bucket_lower_bound`/`merge_from` helpers and a new public const `IO_SIZE_BUCKETS: usize = 25` (also re-exported at `src/lib.rs:94`) — none of this is in FR-017. `src/iblock_device.rs:139,142,159-161,177,193,218`.
- **FR-018** `Supporting Types - Dispatcher` — missing supporting type — **minor**
  - **D8 (minor)** `TierEventStats` (`Copy + Default + PartialEq + Eq`, 4 `u64` fields: `promotions_to_memory`, `promotions_to_gpu`, `evictions_from_memory`, `evictions_from_ssd`) is defined and re-exported (`src/lib.rs:33`) but not listed in FR-018. `src/idispatcher.rs:190-202`.
- **FR-021** `Supporting Types - Extent Manager` — wrong field count — **minor**
  - **D9 (minor)** `FormatParams` documented as "10-field" (spec.md:338); actual is **9** fields (data_disk_size, slab_size, max_extent_size, sector_size, region_count, metadata_alignment, instance_id, metadata_disk_ns_id, metadata_region_size). `src/iextent_manager.rs:42-67`.
- **FR-023** `Supporting Types - Remote` — wrong field count + missing fields — **minor**
  - **D10 (minor)** `LookupConfig` documented as "10-field" (spec.md:349); actual is **12** fields — adds `caller_wait: Option<Duration>` (`src/iremote_lookup.rs:50`) and `connection_teardown_timeout: Duration` (`:57`). `src/iremote_lookup.rs:29`.

**Not Implemented ✗**
- None.

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `IIpcServer` interface + `IpcServerConfig` (4-field) + `IpcError` (4-variant) + `IpcMetricsSnapshot` (5-field) | `src/iipc.rs` | 40-192 | New FR-035 (interface + supporting types), documented with an orphaned-module caveat |

**U1 — `IIpcServer` (unspecced AND orphaned):** `src/iipc.rs` defines a transport-neutral IPC-server interface — `initialize(IpcServerConfig)`, `serve()`, `shutdown()`, `metrics_snapshot() -> IpcMetricsSnapshot`, all `Result<_, IpcError>` — plus `IpcServerConfig` (listen_addr, tls_cert, tls_key, eviction_channel_capacity), `IpcError` (NotInitialized, AlreadyInitialized, Config, Transport), and `IpcMetricsSnapshot` (populates, lookup_hits, lookup_misses, evictions, gpu_bytes_transferred). None of this appears in the spec. **Critically, the `iipc` module is never declared (`mod`) or re-exported (`pub use`) in `src/lib.rs`**, so these types are not part of the compiled `interfaces` crate. The consumer `components/ipc-component/src/lib.rs:33` does `use interfaces::{IDispatcher, IIpcServer, ILogger, IpcError, IpcMetricsSnapshot, IpcServerConfig};` and would fail to build — the break is masked only because `ipc-component` is excluded from the workspace (not in root `Cargo.toml` members) and both files are untracked. This is the **identical latent-build-break pattern** that FR-014/FR-025 had before this pass.

## Recommendations
1. **Backfill (spec → matches code)** the five drifted requirements (FR-008, FR-017, FR-018, FR-021, FR-023) and refresh the stale FR-014/FR-025 "Last Synced" note. These are pure documentation updates to `spec.md` to match the trait/type definitions as they exist.
2. **Backfill a new FR** documenting `IIpcServer` and its supporting types, carrying an explicit orphaned-module caveat (mirroring how FR-014 was historically documented).
3. **Align (code fix, human-owned)** wire `mod iipc;` + `pub use iipc::{...}` into `src/lib.rs` so `IIpcServer` becomes part of the compiled/exported API and the `ipc-component` consumer stops being a latent build break — see `align-tasks.md` (ALIGN-IFACE-001). This is a `.rs` change and is left for a human/code pass, not applied here.
