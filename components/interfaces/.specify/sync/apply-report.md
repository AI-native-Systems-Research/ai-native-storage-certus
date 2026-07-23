# Spec Sync Apply Report — `interfaces`

**Applied**: 2026-07-22
**Spec**: `components/interfaces/specs/001-interfaces/spec.md`
**Drift report**: `components/interfaces/.specify/sync/drift-report.md` / `drift-report.json`
**Head**: `bb9569dde029cc7cd98306e88f7904b8cd4cdbee` (branch `feat/speckit-backfill-remaining-components`)
**Backup**: `components/interfaces/.specify/sync/backups/spec.md.20260722T232124Z.bak`
**Direction**: AUTO-BACKFILL (code → spec), except one code DEFECT routed to align-tasks per hard rules.

This cycle supersedes the prior apply pass (`applied: 2026-07-21`, FR-027/FR-028
only); this pass covers all further drift accumulated since, through the
`zyre` / RDMA-initiator / RDMA-responder split and the 002 `remote-lookup`
rewrite.

## Changes Applied to `spec.md`

| FR | Action | Result |
|----|--------|--------|
| FR-004 | Amended | Added `read_write_stats(&self) -> ReadWriteStats` to `IBlockDevice`'s method list. |
| FR-007 | Amended | Added `promote_block_to_memory_tier` and `try_evict_to_block` to `IDispatchMap`'s method list. |
| FR-008 | Amended | Added `pin`, `unpin`, and `read_write_stats` to `IDispatcher`'s method list. |
| FR-009 | Amended | Added `telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot` to `IMemoryTier`'s method list. |
| FR-012 | Amended | `batch_lookup` signature corrected from `&[(CacheKey, IpcHandle)]` to `&[(CacheKey, u32)]` (size hint, not GPU handle); added `initialize(&self, config: LookupConfig)` method; noted the 002 CPU/DRAM-only rewrite and pointer to FR-030/FR-031 for the GPU-facing path. |
| FR-013 | Superseded (banner added) | `IRemoteRequestHandler` no longer exists in code (removed by commit `29902a2`). Replaced the method list with a superseded banner pointing to the new **FR-030** (`IRemoteLookupRdmaInitiator`) / **FR-031** (`IRemoteLookupRdmaResponder`/`Admin`) split. FR number retained for traceability only. |
| FR-014, FR-025 | **Not modified** (deliberate) | Code DEFECT, not a spec/code evolution — see `align-tasks.md`. Spec continues to describe the intended (currently unreachable) `IExtendedMetadataStore` trait unchanged. |
| FR-017 | Amended | `Command` corrected 11→12 variants (listed); `Completion` corrected 10→11 variants (listed); added `ReadWriteStats` type bullet. |
| FR-018 | Amended | `DispatcherConfig` field count corrected 16→18; all 18 fields enumerated, including the 4 previously-undocumented memory-tier-eviction fields and `extended_metadata_partition_size`. |
| FR-019 | Amended | Added `MemoryTierTelemetrySnapshot` type bullet. |
| FR-023 | Amended | Removed `LookupRef` / `RemoteRequestHandlerError` (no longer exist in code; struck through with a superseded note pointing to FR-013/FR-033/FR-034); added `LookupConfig` (10 fields) type bullet. |
| FR-029 *(new)* | Added | `IZyre` / `IZyreNode` interfaces — LAN/gossip peer discovery and messaging. |
| FR-030 *(new)* | Added | `IRemoteLookupRdmaInitiator` interface — outbound RDMA push of local cache values. |
| FR-031 *(new)* | Added | `IRemoteLookupRdmaResponder` / `IRemoteLookupRdmaResponderAdmin` interfaces — inbound RDMA accept side (control-plane only). |
| FR-032 *(new)* | Added | Supporting Types — Zyre (`PeerId`, `ZyreEvent`, `NodeConfig`, `GossipConfig`, `ZyreError`). |
| FR-033 *(new)* | Added | Supporting Types — Remote Lookup RDMA Initiator (`RemoteRegion`, `PushStatus`, `RemoteLookupRdmaInitiatorError`). |
| FR-034 *(new)* | Added | Supporting Types — Remote Lookup RDMA Responder (`Endpoint`, `LocalRegion`, `ControlChannel`, `ResponderCommand`, `ResponderEvent`, `RemoteLookupRdmaResponderError`). |
| NFR-003 | Amended | Removed `LookupRef` from the `Send + Sync` list (type no longer exists); added a bullet noting `IZyreNode` is `Send` but deliberately not `Sync`. |
| User Story 5 | Amended | Rewritten to describe the outbound/inbound RDMA split and `IRemoteLookup::initialize`, replacing the removed `IRemoteRequestHandler` narrative. |
| User Story 7 *(new)* | Added | "Peer Discovery and Messaging" — covers `IZyre`/`IZyreNode`. |
| Key Entities | Amended | Removed `LookupRef` (superseded); added `PeerId` and `RemoteRegion`/`LocalRegion`. |

## Routed to `align-tasks.md` (NOT backfilled)

| Severity | Spec/ID | Summary |
|----------|---------|---------|
| MAJOR | 001-interfaces/FR-014 | `src/lib.rs` never declares `mod iextended_metadata_store;` (nor re-exports it) — the implemented trait is not compiled into the crate. |
| MAJOR | 001-interfaces/FR-014 | Consumer crate `components/extended-metadata-store` is absent from the root `Cargo.toml` workspace `members`, so it is never built/tested. |

Both are genuine code defects (a committed, complete module never wired in,
plus its sole consumer excluded from the workspace) rather than spec/code
divergence from an intentional change, so per the AUTO-BACKFILL hard rules
they are NOT reflected as spec amendments — FR-014/FR-025 are left exactly as
previously backfilled, describing the intended (currently unreachable) API.

## Notes

- Only `.specify/sync/**` and `specs/001-interfaces/spec.md` were modified.
  No source code (`src/**`, `Cargo.toml`) was changed by this pass.
- FR numbering is now contiguous FR-001..FR-034.
- `drift-report.md`/`drift-report.json` and `proposals.md`/`proposals.json`
  from the prior (2026-07-21) cycle are left as historical record; they are
  superseded by this report for the current drift set.
