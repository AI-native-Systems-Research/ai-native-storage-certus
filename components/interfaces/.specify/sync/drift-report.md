# Drift Report: interfaces

Generated: 2026-08-07T15:29:55Z
Spec: components/interfaces/specs/001-interfaces/spec.md
Implementation: components/interfaces/src/*.rs, Cargo.toml

## Summary

| Metric | Count |
|--------|-------|
| Aligned | 37 |
| Drifted | 3 |
| Not Implemented | 1 |
| Unspecced | 4 |

The `interfaces` crate is largely aligned with its backfilled spec: the bulk of FR-001..FR-034 interface/type requirements match trait definitions and `lib.rs` re-exports. Most divergences reported in the prior (2026-07-22) pass have since been absorbed into the spec (read_write_stats, promote_block_to_memory_tier/try_evict_to_block, pin/unpin, RDMA split, zyre). Remaining divergences: (1) an orphaned `IExtendedMetadataStore` module defined on disk but never wired into `lib.rs`, so FR-014/FR-025 are effectively not part of the compiled crate; (2) the `IEvictionPolicy::track` signature carries an extra `BlockSemantics` argument absent from FR-006; and (3) several unspecced items (`BlockSemantics`, `SessionId`, the `integrity-check` feature with `set/get_checksum`, and `push_async`/`PushCompletion`).

## Detailed Findings

### Aligned (representative)

- FR-001 IGreeter — `src/igreeter.rs:5`.
- FR-002 ILogger — `src/ilogger.rs:5-8`.
- FR-003 ISPDKEnv (spdk) — exported `src/lib.rs:87`.
- FR-004 IBlockDevice incl. `read_write_stats` — exported `src/lib.rs:90-93`.
- FR-005 IBlockDeviceAdmin — exported `src/lib.rs:96`.
- FR-007 IDispatchMap incl. `promote_block_to_memory_tier`, `try_evict_to_block`, `recover_extent`, `convert_to_storage` — `src/idispatch_map.rs:125,234,262,271`.
- FR-008 IDispatcher (incl. pin/unpin, read_write_stats) — `src/idispatcher.rs`.
- FR-009 IMemoryTier, FR-010 IExtentManager, FR-011/FR-028 IGpuServices (set_device/device_of_ptr) — present and exported.
- FR-012 IRemoteLookup (initialize + `batch_lookup(&[(CacheKey,u32)])`) — `src/iremote_lookup.rs`, exported `src/lib.rs:49-51`.
- FR-013 IRemoteRequestHandler correctly SUPERSEDED — no such trait exists in code (matches spec note).
- FR-030 IRemoteLookupRdmaInitiator (push/connect/disconnect/disconnect_all/set_local_peer_id) — `src/iremote_lookup_rdma_initiator.rs:221,247,254,257,266`.
- FR-031 IRemoteLookupRdmaResponder/Admin — exported `src/lib.rs:57-60`.
- FR-029/FR-032 IZyre/IZyreNode/types — exported `src/lib.rs:61-67`.
- FR-016..FR-024, FR-033/FR-034 supporting types — present/exported (spot-checked via lib.rs).
- NFR-001..006, SC-1..7 — structurally satisfied (feature gating in Cargo.toml/lib.rs; Send/Sync and error-type conventions present).

### Drifted

1. **FR-006 IEvictionPolicy — `track` signature drift** — MEDIUM
   - Spec: `track(&self, pool: PoolId, key: CacheKey) -> Result<EvictionHandle, EvictionPolicyError>`.
   - Code: `track(&self, pool: PoolId, key: CacheKey, semantics: BlockSemantics) -> Result<EvictionHandle, EvictionPolicyError>` (`src/ieviction_policy.rs:87`).
   - The extra `semantics: BlockSemantics` parameter is not described in FR-006.

2. **FR-018 LookupResult — variant count mismatch** — LOW
   - Spec FR-018: "3-variant enum (NotExist, MismatchSize, BlockDevice, MemoryTier)" — label says 3 but lists 4.
   - Code: 4 variants `NotExist, MismatchSize, BlockDevice{..}, MemoryTier{..}` (`src/idispatch_map.rs:11-22`). Spec text is internally inconsistent (should read 4-variant).

3. **FR-014 IExtendedMetadataStore / FR-025 ExtendedMetadataStoreError — defined but not exported** — HIGH
   - `src/iextended_metadata_store.rs` defines `IExtendedMetadataStore` (`:30-45`) and `ExtendedMetadataStoreError` (`:6`), but `lib.rs` does NOT declare `mod iextended_metadata_store` nor re-export the interface/error (confirmed: no reference in `src/lib.rs`).
   - Consequence: the file is orphaned/dead code and the types FR-014/FR-025 claim to be part of the `interfaces` crate are not compiled or reachable through the crate's public API.

### Not Implemented

- FR-014 IExtendedMetadataStore (and its FR-025 error type) — not wired into the crate (see Drifted #3). No reachable public API despite the spec listing it as a crate interface.

## Unspecced Code

| Item | Location | Notes |
|------|----------|-------|
| `BlockSemantics` struct + `session_id` field | `src/ieviction_policy.rs:43` | Not listed in FR-020 (Eviction Policy supporting types); introduced as the extra `track` arg. |
| `SessionId` type alias (`u64`) | `src/ieviction_policy.rs:33` | Not in spec; exported at `src/lib.rs:38`. |
| `integrity-check` Cargo feature + `set_checksum`/`get_checksum` | `Cargo.toml:507`; `src/idispatch_map.rs:287,295` | Overview lists only `spdk`/`gpu` features; FR-007 does not include these CRC-32 methods. |
| `push_async` method + `PushCompletion` type | `src/iremote_lookup_rdma_initiator.rs:158,198`; exported `src/lib.rs:53` | FR-030 lists only push/connect/disconnect/disconnect_all/set_local_peer_id; FR-033 supporting types omit `PushCompletion`. |

## Conflicts / Spec references to nonexistent artifacts

- FR-014/FR-025 reference an `IExtendedMetadataStore` interface that the `interfaces` crate does not actually expose (orphaned module) — spec asserts export, code does not deliver.
- Implementation Notes reference "formally verified properties documented in comments (IDispatchMap: 10 props, IDispatcher: 10 props, IExtentManager: 10 props, IGpuServices: 10 props, IMemoryTier: 10 props)" — not independently verified in this pass.

## Recommendations

1. Decide the intent of `IExtendedMetadataStore`: either add `mod iextended_metadata_store;` + re-exports to `lib.rs` (making FR-014/FR-025 real), or remove the orphaned file and delete FR-014/FR-025 from the spec.
2. Update FR-006 to include the `semantics: BlockSemantics` parameter and document `BlockSemantics`/`SessionId` under FR-020 (or backfill a dedicated FR).
3. Add the `integrity-check` feature and `set_checksum`/`get_checksum` to the Overview features list and FR-007.
4. Add `push_async`/`PushCompletion` to FR-030/FR-033.
5. Fix the FR-018 "3-variant" label to "4-variant" for `LookupResult`.
