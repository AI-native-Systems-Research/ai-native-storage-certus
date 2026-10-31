---
spec_sync_component: interfaces
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-31T06:35:50Z
spec_sync_git_commit: 50a0f2bf
spec_sync_inputs_sha256: 961dca7002f71f03b281f87f75dd1daec95211257a4f792d3c60e3d72cd46029
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Sync 2026-10-30 (branch `specsync-a`: optimizer session `s20261029-094947` merged onto unstable, plus `55a4ddd4` clippy) — delta, drift resolved by backfill. Not stamped here** (the final pass stamps every component on the merged tree; the frontmatter above is the previous stamp and is stale). Base `a0292be6` (`origin/unstable`, fresh stamp). `git diff a0292be6 HEAD -- components/interfaces` touches only `src/` (10 files, +189/-51), from three commits: `4941320a` (new method), `6a917346` (merge, no own change here) and `55a4ddd4` (clippy). Every changed hunk was read and each spec statement it could affect was checked against the code.
>
> - **FR-011 `IGpuServices` method inventory — DRIFTED (moderate), code authoritative.** `4941320a` adds `memcpy_batch_async(dsts, srcs, sizes, stream)` behind `spdk` (`src/igpu_services.rs:745-768`); FR-011 listed 24 methods and not this one. The doc contract (no order among entries, entries must not overlap, pointers valid until sync, host pointers pinned, stream must not be the legacy NULL stream; errors: no GPU support, not initialized, slice lengths differ, CUDA failure) matches the implementation `components/gpu-services/src/lib.rs:973-1038` (length check `:991-999`, empty batch `Ok` `:1000-1002`, `cudaMemcpyBatchAsync` with stream access order `:1004-1026`). The callers honour the NULL-stream rule: `copy_gpu_to_memory_async` falls back to per-region `memcpy_d2h_async` on a null stream (`components/dispatcher/src/lib.rs:3876-3878`); the warm serve batches only when `warm_raw != 0` (`lib.rs:1422-1427`); a one-entry list uses the single memcpy (`components/dispatcher/src/pipeline.rs:120-127`). **Backfilled** (FR-011 entry, P-1).
> - **plan.md method counts — DRIFTED (minor).** `IGpuServices` is 25 methods (11 spdk-gated), not 24 (10). `IDispatcher` is 25, not 24: `clear_all` (unstable `266ed975`) was recorded in spec.md US2/FR-008 by the 2026-10-25 sync, but the plan table was not updated. Total 180, not 178. Recounted mechanically over every `define_interface!` block at base and HEAD. **Backfilled** (P-2).
> - **Implementation Notes citation — DRIFTED (cosmetic).** The `IGpuServices` verified-properties comment moved from `:231-246` to `:250-265` (SAFETY comments inserted above it). **Backfilled** (P-3).
> - **`55a4ddd4` clippy changes — ALIGNED, checked not assumed.** (a) `GpuDmaBuffer`'s `Debug` now ends with `..` (`src/igpu_services.rs:222`); no spec, plan, README or test quotes its `Debug` output (grep over `components/**/*.md` and `*.rs` for `"GpuDmaBuffer`). (b) SAFETY comments on `Sync` for `LookupResult`, `GpuIpcHandle`, `GpuDmaBuffer`, `GpuStream`: NFR-003 lists exactly these as `Send + Sync`; nothing changed in the impls. `DmaBuffer`'s `Sync` (`src/spdk_types.rs:227`) shares the comment above `Send` and was not touched. (c) `# Errors` on `IZyreNode::{start, join, leave, shout, whisper, recv, try_recv}` (`src/izyre.rs:366-447`) match `components/zyre/src/node.rs:78-83,144-269`, the only implementor; FR-029 gives one-liners and enumerates no errors, and FR-032's 7-variant `ZyreError` covers every variant named. (d) `WriteHandle::publish` `# Errors`/`# Panics` (`src/iextent_manager.rs:137-150`) match `:150-161`; FR-021 / NFR-006 / Implementation Notes ("publish/abort … consumed on first call, with drop guard for abort") still hold. (e) `#[must_use]` on getters/constructors and doc backticks: no signature or behaviour change; the reworked `izyre.rs` tests assert the same `validate()` results.
> - **Wire ops (brief question).** No interfaces spec describes `LookupBlocks`/`CopyToStoreBlocks`/`LookupChunks`/`CopyToStoreChunks` or the shmq wire layout; the only wire mention (FR-018a, `op_reserve` writes `0` on any `Err`) still holds (`lib/shmq-dispatcher/src/translate.rs:904`).
> - **SC re-run this session.** SC-2/SC-7: `cargo test -p interfaces` passes (35 unit + 54 doctests, 1 ignored). SC-3: `cargo check -p interfaces --features spdk` not possible here (no `deps/spdk`). SC-4: `cargo doc -p interfaces --no-deps` gives **3 warnings** (unresolved links `IDispatcher::tier_event_stats` at `src/idispatcher.rs:339`, `create_stream` at `src/igpu_services.rs:587`, `stream_synchronize` at `:607`); all three exist verbatim at base `a0292be6`, so not introduced by this branch. Recorded as a pre-existing doc item (P-4, not approved); NFR-004's runnable-examples task interfaces-14 stays open as before (the new method has no example, like its four memcpy siblings).
>
> **Would stamp: `clean`** for this branch's changes: all drift it introduced is backfilled. P-4 (SC-4 rustdoc links, pre-existing) was then fixed in `26e7a9cc`; `cargo doc -p interfaces --no-deps` is warning-free. Open and not new: align-tasks interfaces-7/interfaces-14 (tracked since 2026-10-21, unchanged).

> **Re-stamp 2026-10-29 (branch `fix/dispatcher-remove-ssd-extent`, merged with unstable `e35618b2`).** Transitive only: this component's own `src/` and `specs/` are as unstable has them; its digest moves because `components/interfaces` changed (the `IDispatcher::check` doc gains an `# Errors` section, B-17 of the dispatcher sync). No analysis below changes.
> **Re-stamp 2026-10-25 (branch `001-rdma-cache-transfer`, rebased onto unstable `a905f89b`).** New since the last stamp: `IDispatcher::clear_all` and its `ClearAll { removed, still_pinned }` result, recorded in `specs/001-interfaces/spec.md` (method inventory, method and struct entries). Upstream's changes arrive with the rebase; their own spec updates are upstream's.

> **Re-stamp 2026-10-23 (branch `unstable`, commit `d7a24d58` "Fix workspace build warnings" plus the spec-sync follow-up).** That commit only fixed warnings under the workspace lint posture: it added `Debug` impls, narrowed unreachable `pub` items to `pub(crate)`, wrapped `unsafe fn` bodies in explicit `unsafe {}` blocks, and the follow-up ran `rustfmt`. It also touched `components/interfaces` (a `Debug` impl for `Command`), which changes every component's digest. This component's own `src/` was in that commit. Every changed item was checked: none is part of a provided interface, none is named as public by the spec, and no behaviour changed. Own change: a manual `Debug` impl for `Command` (`src/iblock_device.rs:417-423`, prints the variant discriminant only, since variants hold `Arc<DmaBuffer>`). No variant, field, trait method or documented behaviour changed; every interface FR still holds. All citations were re-verified by diff-mapping each base line to its new position. Status stays `clean`.

> **Apply 2026-10-21 (branch fix/dispatcher-remove-clear-mt, approved by Cornel).** Batch apply of the 12 approved proposals listed in `proposals.json`. Backup: `.specify/sync/backups/20261021T225804Z/`. Details: `apply-report.{md,json}`.
>
> - **Applied: 10 BACKFILL proposals** to `specs/001-interfaces/spec.md`, each using the exact old→new text from the proposal. **1** FR-017: `Command` 13 / `Completion` 12 variants, plus the `ReadWriteStats` histograms. **2** FR-018: `DispatcherConfig` 19 fields, plus a new `TierEventStats` bullet. **3** FR-008 / US2: `copy_gpu_to_memory_async(regions: &[IpcHandle])`, plus `batch_populate`, `tier_event_stats` and `schedule_write_through`; the US2 inventory goes from 16 to 22 methods. **4** FR-008: result contracts for `clear_memory_tier` (pinned entries kept, `Timeout`) and `flush_to_ssd` (`IoError`). **8** FR-023: `LookupConfig` 12 fields. **9** FR-021: `FormatParams` 9 fields. **10** Overview / FR-009: three features, `IMemoryTier` ungated. **11** NFR-002: `gpu` gates nothing. **12** Implementation Notes: verified-properties claim corrected. **13** Banner: FR-014 "deferred" marked resolved. A `Last Synced 2026-10-21` banner line was also added.
> - **Applied: 2 ALIGN proposals, applied by hand before this run.** **5** (`clear_memory_tier` doc) and **6** (`batch_lookup` doc) are in `src/idispatcher.rs`. They were checked against the proposed text, but this apply did not edit `src/`. `cargo doc -p interfaces --no-deps --features spdk` has not been run yet.
> - **Became tasks / stay open (not approved, TRACK), recorded in `align-tasks.md`.** **7** (major): port the `remove` / `clear_memory_tier` / `flush_to_ssd` repairs to component **`dispatcher-p2p`**. It is recorded here because the contract is this crate's `IDispatcher` doc; the work belongs to `dispatcher-p2p`. **14** (moderate): NFR-004 runnable doc examples. Its BACKFILL alternative was not chosen.
>
> After this apply, the 14 drift items of the 2026-10-21 analysis below stand as follows: 12 resolved (1-6, 8-13) and 2 open (7, 14). SC-2/3/4/7 have still not been re-executed. The frontmatter is left for the re-stamp script.

> **Sync 2026-10-21 (branch fix/dispatcher-remove-clear-mt) — full re-analysis, not a delta.** Every FR, NFR and SC in `specs/001-interfaces/spec.md` (37 FR including FR-018a/FR-023a/FR-031a, 6 NFR, 7 SC = 50) was re-checked against `src/**` with file:line evidence. Every enum's variants and every struct's fields were counted, not assumed. The branch's own inputs are the two Cornel-approved working-tree edits: the FR-018a `store_already_resident` note and the `IDispatcher::flush_to_ssd` `# Errors` doc. **Both are ALIGNED** with the dispatcher repairs `efd502331` / `9bd231d85` (`dispatcher/src/lib.rs:3507,3541` count only on the store path; `lib.rs:4026-4034` returns `IoError` on a failed write-through). **38 aligned, 8 requirements drifted (14 drift items), 0 not implemented, 4 not re-verified.**
>
> - **Introduced by this branch (3).** The `clear_memory_tier` doc comment (`src/idispatcher.rs:806-810`) still says entries being written "are removed entirely". Since `cccfb328e` the dispatcher keeps a pinned entry's slot, retries it, and returns `Timeout` after 5 s. Spec FR-008's one-liners for `clear_memory_tier` / `flush_to_ssd` say nothing about the new error results. `dispatcher-p2p`, the second `IDispatcher` implementor, does not meet the newly documented `flush_to_ssd` contract (and still has the two bugs fixed in `dispatcher`).
> - **Older count/inventory drift found by the full recount (11 items).** `Command`/`Completion` are 13/12 variants, not 12/11 (`FlushSync`/`FlushDone`, since `e56a8b4e6`). `DispatcherConfig` has 19 fields, not 18 (`store_backpressure_ms`). `LookupConfig` has 12, not 10. `FormatParams` has 9, not 10. FR-008 omits `batch_populate` and `tier_event_stats` and still gives `copy_gpu_to_memory_async` its old single-`IpcHandle` signature. `IMemoryTier` is not `spdk`-gated. No code is gated on `gpu`. The "verified properties in comments" note and the 2026-08-07 "FR-014 deferred" banner are stale. The `batch_lookup` doc still says "one `Result`". NFR-004's runnable-examples rule is unmet in five modules.
>
> SC-2/3/4/7 (build, doc and test runs) were **not re-executed**: this session was not permitted to run `cargo`. Proposals: 14 (10 BACKFILL, 4 ALIGN), all `approved: false`. Frontmatter left unchanged for the re-stamp script; its `clean` status does not reflect this analysis.

> **Sync 2026-10-14 (branch `fix/cold-staging-buf-16mib`).** The branch raised `DispatcherConfig::cold_staging_buf_bytes` from 4 MiB to 16 MiB (`src/idispatcher.rs:125`, commit `8e12e49f`). FR-027 in this component's own spec still named the old 4 MiB default — **DRIFTED (moderate), code authoritative**. Updated FR-027 to 16 MiB with the reason (an 8 MiB per-worker KV-offload block fails the `serve_cold_staged` fit guard under a 4 MiB buffer; see dispatcher FR-033). The `DispatcherConfig` field-list summary names the field without a default and is unaffected. No actionable drift remains.

> **Sync 2026-10-09 (branch `fix/evict-blocked-by-pin`).** Doc-comment change only: `IDispatchMap::try_evict_to_block` now documents `ActiveReferences` for a held reference and `InvalidState` for the remaining non-evictable cases, matching the dispatch-map implementation and FR-026. No signature or type change. `specs/001-interfaces` describes the method without enumerating its errors (line 166), so it stays aligned; verified by reading it.

> **Sync 2026-10-05 (branch `fix/store-declines-root-cause`).** Delta analysis on a certified baseline: `origin/unstable` (`08a5ae88`) changed no component `src/`or `specs/` after its `fa4adab0` re-stamp, so the only new inputs are this branch's changes. Every FR naming a counter, a `TierEventStats`/`RemoteServeStats` field, or an `IDispatcher` method was located by grep over `specs/**` and re-checked against the changed code.
>
> - **FR-023 `RemoteServeStats` field list — DRIFTED (major).** The spec declared a **6-field** type and named `peer_pins_taken` and `peer_pin_hold_us_total`; the shipped type has **4 fields** and neither of those exists. Both were removed deliberately in `4e185dd2` after measurement — `peer_pins_taken` read 1.000x the already-published `peer_served_keys` on all three runs, and `peer_pin_hold_us_total` accumulated per batch against per-key counts so it could not form the mean it existed for. The spec text had been added by earlier commits *on this same branch* and was not revised in place when the counters went. **Resolution**: code authoritative. Field list corrected to 4, both removed fields recorded as SUPERSEDED with the measured reason, and the two hold-time bullets reworded — with `_total` gone there is no mean, so `peer_pin_hold_us_max` is now stated as the only hold-time field and deliberately a high-water mark.
> - **FR-018a / FR-031a — ALIGNED, verified not assumed.** All three store-refusal counters are published by `apps/certus-server-yaml/src/metrics.rs`; `schedule_write_through` has four implementations, none blocking, and its best-effort contract holds at `remote-lookup/src/actor.rs:629`.
>
> No actionable drift remains for this component.
> **Sync 2026-10-05 (branch `fix/poller-cpu-placement-logging`).** Delta analysis on a certified baseline: this component's existing clean stamp was re-verified to equal the spec-sync hash of `git archive origin/unstable`, so the only new inputs are this branch's src/specs/interfaces changes. Every FR/SC touching CPU placement, NUMA pinning, threads or SPDK init was located by grep over specs/** and re-checked against the changed code.
>
> - **DispatcherConfig::poller_base_cpu doc comment — ALIGN (doc) (minor).** The dispatchers assign NUMA-local cores round-robin, excluding each node's first two; block-device fallback / unpinned cases as in dispatcher FR-011. `components/interfaces/src/idispatcher.rs:46-56`. *Pre-existing doc drift; user chose to fix in this PR and re-stamp all components.* Resolution: Doc comment corrected. FR-018 lists `poller_base_cpu` by name only, so no spec text change was needed.
>
> No actionable drift remains for this component after apply.

> **Re-stamp 2026-10-01 (transitive: `components/interfaces` changed).** The spec-sync digest
> folds `components/interfaces/{src,specs}` into **every** component's hash, so an interface
> change invalidates all of them at once -- which is the design, not a defect. The interface
> change is the `served_by` attribution work: new `ServedBy` and `LookupOutcome` types, and
> `IDispatcher::batch_lookup` widened to return `Vec<LookupOutcome>`.
>
> **No re-analysis was performed for this component, and the digest bump asserts only what was
> actually checked**: the interface change is additive except for `batch_lookup`'s return type,
> which the compiler enforces across all implementors, and this component's own `src/**` and
> `specs/**` are unchanged. Components that implement `IDispatcher` (`dispatcher`,
> `dispatcher-p2p`) had their specs updated substantively; this one did not need it. If a later
> sweep finds drift here, this stamp is not evidence against it.

> **Sync 2026-09-15 (IDispatcher::reserve_memory signature — code authoritative).** Branch `fix-reserve-batch-deadline` (`bec6c6ec`) added a `deadline: Option<std::time::Instant>` parameter to `reserve_memory` in `src/idispatcher.rs`. The method inventory in `specs/001-interfaces/spec.md` was updated to the new signature and documents the two modes (`None` = dispatcher's own `store_backpressure_ms` budget; `Some(instant)` = a deadline shared across a reserve batch, bounding the batch's total backpressure). First adoption of the spec-sync freshness stamp for this component. Digest recomputed over a clean tree matching CI.

# Spec Drift Report

Generated: 2026-10-21
Project: interfaces (`components/interfaces`)
Branch: `fix/dispatcher-remove-clear-mt` (HEAD `efd502331` + Cornel-approved working-tree edits to `specs/001-interfaces/spec.md:331` and `src/idispatcher.rs:832-837`)
Method: full analysis. Every requirement was read in the spec and located in `src/**`, and every "N-variant" / "N-field" claim was recounted. The cross-component contracts this branch touched (`IDispatcher::{remove, clear_memory_tier, flush_to_ssd}`, `TierEventStats::store_already_resident`) were also checked against **both** implementors, `components/dispatcher` and `components/dispatcher-p2p`.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 50 (37 FR + 6 NFR + 7 SC) |
| ✓ Aligned | 38 (76%) |
| ⚠️ Drifted | 8 (16%) — FR-008, FR-009, FR-017, FR-018, FR-021, FR-023, NFR-002, NFR-004; 14 drift items in total, including two in non-requirement sections (Implementation Notes, Backfill banner) |
| ✗ Not Implemented | 0 (0%) |
| ⏸ Not re-verified (cargo not permitted in this session) | 4 (8%) — SC-2, SC-3, SC-4, SC-7 |
| 🆕 Unspecced Code | 5 items (all folded into drift proposals below) |

The 14 drift items: FR-008(a)–(e) (5 items; (a) and (e) are drift in the interface doc comments of `src/idispatcher.rs`, and (c) is a contract violation by the second implementor), FR-017, FR-018, FR-021, FR-023, Overview/FR-009, NFR-002, NFR-004, Implementation Notes, and the Backfill banner.

## Detailed Findings

### Spec: 001-interfaces — Shared Interface Trait Definitions

#### Branch inputs (approved edits): verified ✓

- **FR-018a `store_already_resident` (approved edit, spec.md:331)**: "counted only in `reserve_memory`, the store path. A load that loses the insert race … is not counted."
  - Code: `dispatcher/src/lib.rs:1214-1221`. The `AlreadyExists` arm of `evict_and_insert` returns without counting, and its comment says why. Counting happens only in `reserve_memory`: `lib.rs:3507` (budget-zero path) and `lib.rs:3541` (backpressure loop). "Bypasses the backpressure retry" holds: the `AlreadyExists` arm at `lib.rs:3535-3543` returns before any `record_store_backpressure`. → **ALIGNED**.
  - Interface doc `src/idispatcher.rs:383-401` ("returns from `evict_and_insert` immediately, bypassing the backpressure retry entirely") is still accurate. → ALIGNED.
  - The "Known asymmetry" bullet (spec.md:333) still holds: `serve_concurrently_promoted` exists in `dispatcher/src/lib.rs` (1 definition). Exposure on the metrics endpoint: `apps/certus-server-yaml/src/metrics.rs:227-235`. → ALIGNED.
- **`IDispatcher::flush_to_ssd` `# Errors` (approved edit, src/idispatcher.rs:832-837)**: `IoError` if a write-through failed for an entry still in the memory tier.
  - `dispatcher/src/lib.rs:4026-4034`: `write_through_pins.failed_writes(|key| mt.contains(key)) > 0` → `IoError`. The ledger is in `dispatcher/src/background.rs:362-380` (`record_write` / `failed_writes`, which forgets keys that are no longer resident). → **ALIGNED** for `dispatcher`. **Not met by `dispatcher-p2p`**: see Drifted FR-008(c).

#### Aligned ✓

- FR-001 `IGreeter::greeting_prefix` → `src/igreeter.rs:5`
- FR-002 `ILogger` error/warn/info/debug → `src/ilogger.rs:5-8`; export `src/lib.rs:50`
- FR-003 `ISPDKEnv` (5 methods, spdk) → `src/ispdk_env.rs:9-25`; gated `src/lib.rs:78-79,93-94`
- FR-004 `IBlockDevice` (11 methods incl. `read_write_stats`) → `src/iblock_device.rs:557-589`
- FR-005 `IBlockDeviceAdmin` (6 methods) → `src/iblock_device.rs:601-633`
- FR-006 `IEvictionPolicy` (9 methods; `track(…, semantics: BlockSemantics)`) → `src/ieviction_policy.rs:75-114`
- FR-007 `IDispatchMap` (20 methods; `set_checksum`/`get_checksum` under `integrity-check`) → `src/idispatch_map.rs:88-216` (cfg at 207, 215); feature declared `Cargo.toml:17`
- FR-010 `IExtentManager` (14 methods) → `src/iextent_manager.rs:173-221`
- FR-011 `IGpuServices` (24 methods; the 10 spdk-gated ones match the spec's tags) → `src/igpu_services.rs:268-771`
- FR-012 `IRemoteLookup` (initialize, `batch_lookup(&[(CacheKey,u32)])`, join/leave, `serve_stats`) → `src/iremote_lookup.rs:194,217-220,234,248,270`
- FR-013 SUPERSEDED pointer. `IRemoteRequestHandler` / `LookupRef` are absent from `src/**` (grep: no matches). → consistent
- FR-014 `IExtendedMetadataStore` (5 methods) → `src/iextended_metadata_store.rs:30-45`; **now wired** `src/lib.rs:83` (`mod`) and `src/lib.rs:105` (`pub use`), ungated
- FR-015 `IPartitionTable` (4 methods, spdk) → `src/ipartition_table.rs:114-127`
- FR-016 SPDK types: `SpdkEnvError` has 7 variants (`src/spdk_types.rs:19-34`) ✓, `BlockDeviceError` 10 (`:64-85`) ✓, `DmaBuffer` metadata map/Deref (`:364-405`) ✓, `DmaAllocFn` (`:219`), `set/is_spdk_env_active` (`:208-215`)
- FR-019 `MemoryTierError` 7 variants (`src/imemory_tier.rs:20-35`) ✓; `MemoryTierTelemetrySnapshot` `Copy+Default`, 3 fields (`:11-16`) ✓
- FR-020 `EvictionHandle`, `EvictionPolicyError` 2 variants (`src/ieviction_policy.rs:54-59`), `PoolId=u32` (`:29`), `SessionId=u64` (`:33`), `BlockSemantics` `Copy+Default` (`:42-50`) ✓
- FR-022 GPU types (`src/igpu_services.rs:28-41,58-118,134-200,225`) ✓
- FR-023a `ServedBy` 6 variants + `is_hit` (`src/idispatcher.rs:248-285`); `LookupOutcome` (`:307-312`) ✓
- FR-024 partition types; `PartitionTableError` 6 variants (`src/ipartition_table.rs:60-73`); `type_guids` 3 consts (`:91-112`) ✓
- FR-025 `ExtendedMetadataStoreError` 4 variants (`src/iextended_metadata_store.rs:6-15`), exported `src/lib.rs:105` ✓
- FR-026 `DispatchMapError` 11 variants (`src/idispatch_map.rs:39-62`) ✓
- FR-027 `cold_staging_slots` default 64, `cold_staging_buf_bytes` default 16 MiB → `src/idispatcher.rs:124-125` ✓
- FR-028 `set_device` / `device_of_ptr` → `src/igpu_services.rs:544,566` ✓
- FR-029 `IZyre` (`src/izyre.rs:427-436`), `IZyreNode: Send` plain trait, 16 methods (`:362-424`) ✓
- FR-030 `IRemoteLookupRdmaInitiator` (push_async, push, connect, disconnect, disconnect_all, set_local_peer_id) → `src/iremote_lookup_rdma_initiator.rs:198-266` ✓
- FR-031 Responder + Admin → `src/iremote_lookup_rdma_responder.rs:230-282` ✓
- FR-031a `schedule_write_through` (non-blocking, best-effort) → `src/idispatcher.rs:840-868` ✓
- FR-032 `PeerId` (`src/izyre.rs:60-91`), `ZyreEvent` 9 variants (`:112-155`), `NodeConfig` `#[non_exhaustive]` 9 fields (`:268-288`), `GossipConfig` 2 fields (`:216-222`), `ZyreError` 7 variants (`:16-31`) ✓
- FR-033 `RemoteRegion` 3 fields, `PushStatus` 4, `PushCompletion` (`:158`), initiator error 2 ✓
- FR-034 `Endpoint` 2, `LocalRegion` 3 (`length: usize`), `ResponderCommand` 1, `ResponderEvent` 3, responder error 6 ✓
- NFR-001 Zero coupling: `Cargo.toml:8-11` depends only on component-core / component-macros / optional spdk-sys ✓
- NFR-003 Thread safety: `LookupResult` Send+Sync `src/idispatch_map.rs:33-35`; `DmaBuffer` `src/spdk_types.rs:226-227`; `GpuIpcHandle` `src/igpu_services.rs:70-71`; `GpuDmaBuffer` `:145-146`; `GpuStream` `:228-229`; `Command: Send` `src/iblock_device.rs:419`; `Completion: Send + Clone` `:438,517`; `IZyreNode: Send` only `src/izyre.rs:362` ✓
- NFR-005 Error handling: all 14 error enums derive `Debug, Clone` and implement `Display` + `std::error::Error` (14 `impl std::error::Error` sites); `From<BlockDeviceError>` / `From<SpdkEnvError>` for `NvmeBlockError` `src/iblock_device.rs:74-85` ✓
- NFR-006 Deterministic cleanup: `DmaBuffer::drop` with SPDK-active guard `src/spdk_types.rs:375-390`; `GpuDmaBuffer::drop` `src/igpu_services.rs:188`; `WriteHandle::drop` abort `src/iextent_manager.rs:149-155` ✓
- SC-1 (structural: the crate holds only definitions) ✓; SC-5 (see NFR-003) ✓; SC-6 (see NFR-005) ✓

#### Drifted ⚠️

- **FR-008(a) — `clear_memory_tier` (spec.md:185) and its interface doc comment** — *introduced by this branch's `cccfb328e`*
  - Spec says: "`clear_memory_tier(&self) -> Result<usize, DispatcherError>` - Evict all memory-tier entries."
  - Interface doc `src/idispatcher.rs:806-810` says: "Entries whose write-through has completed are converted to block-device state … **Entries still being written are removed entirely.** Returns the number of entries cleared." No `# Errors` section.
  - Code (`dispatcher/src/lib.rs:3940-4004`): an entry whose write-through is pending holds the write-through read pin. `try_evict_to_block` returns `ActiveReferences` (`:3976`), so the entry **keeps its slot and is retried**. It is not removed. After 5 s with no progress the call returns `DispatcherError::Timeout` (`:3994-3998`), leaving pinned entries resident; `Ok(n)` counts only entries really cleared. This is now dispatcher FR-038.
  - Severity: **moderate**. The trait doc describes the exact behaviour the repair removed (freeing the slot of a pinned entry), so a new implementor following it would reintroduce the bug.
- **FR-008(b) — `flush_to_ssd` (spec.md:186)** — *this branch's `9bd231d85`*
  - Spec says: "`flush_to_ssd(&self) -> Result<usize, DispatcherError>` - Flush pending writes to SSD."
  - Code: `dispatcher/src/lib.rs:4006-4035` returns `IoError` when a failed write-through left an entry resident; otherwise it returns the count of memory-tier keys that `dm.is_evictable` (SSD offset **and** no refs). The interface doc already says this (approved, `src/idispatcher.rs:831-837`), as does dispatcher FR-056 (approved). Only this spec's one-liner is silent.
  - Severity: minor (summary line, no contradiction). The finer point that a pinned entry with an SSD offset is not counted (because `is_evictable` requires zero refs) is also undocumented in the trait doc's "entries that now have a valid SSD offset".
- **FR-008(c) — `IDispatcher` contract vs. the second implementor, `dispatcher-p2p`** — *cross-component, surfaced by this branch*
  - Interface contract (`src/idispatcher.rs:625-646, 806-838`): `remove` "Does NOT block waiting for background write-through … Proceeds immediately"; `flush_to_ssd` returns `IoError` on a failed write-through and otherwise the count of entries with an SSD offset.
  - `dispatcher-p2p/src/lib.rs:2889-2902`: `flush_to_ssd` returns `writer.in_flight()` from **before** the flush and never returns `IoError`. This is the pre-repair behaviour that `9bd231d85` fixed in `dispatcher`.
  - `dispatcher-p2p/src/lib.rs:2833-2864`: `clear_memory_tier` calls `mt.remove(key)` on **every** candidate whose `try_evict_to_block` failed, including `ActiveReferences` (`:2855-2858`), and counts every candidate (`:2860`). This is the pre-`cccfb328e` bug.
  - `dispatcher-p2p/src/lib.rs:2380-2381`: `remove` maps every `dm.remove` error, including `ActiveReferences` from the write-through pin installed by `downgrade_reference` at `:2609`, to `KeyNotFound`. This is the pre-`0236a3c98` bug.
  - Severity: **major** (same data-loss / use-after-release class as the three repaired bugs). The fix belongs to `dispatcher-p2p`, not to `interfaces`.
- **FR-008(d) / User Story 2 — method inventory incomplete and one signature stale**
  - Spec FR-008 (spec.md:171-189) and US2 (spec.md:49) list 19 / 16 methods.
  - Code: `IDispatcher` has 22 methods (`src/idispatcher.rs:499-883`). Not in this spec: `batch_populate` (`:688`) and `tier_event_stats` (`:883`); `schedule_write_through` is covered by FR-031a only. Both are implemented by `dispatcher` (`lib.rs:3324, 4080`) and `dispatcher-p2p` (`lib.rs:2430, 2915`).
  - Signature drift: the spec gives `copy_gpu_to_memory_async(&self, key, ipc_handle: IpcHandle, stream: GpuStream)`. Code: `regions: &[IpcHandle]` (`src/idispatcher.rs:732`), a gather of N per-layer regions (vLLM 0.23+).
  - Severity: moderate.
- **FR-008(e) — `batch_lookup` doc comment** (interface src)
  - `src/idispatcher.rs:591`: "Returns one `Result` per input entry, in the same order." The signature at `:599-602` returns `Vec<LookupOutcome>`, and spec.md:175 states this correctly. Stale since the 2026-10-01 widening.
  - Severity: minor.
- **FR-017 — `Command` / `Completion` variant counts; `ReadWriteStats` histograms**
  - Spec (spec.md:312-313, 315): "`Command`: 12-variant enum …", "`Completion`: 11-variant enum …"; `ReadWriteStats` "byte, op, and latency-sum counters with `total_ops`/`total_bytes`/`mean_*` helpers".
  - Code: `Command` has **13** variants, adding `FlushSync { ns_id }` (`src/iblock_device.rs:408-414`). `Completion` has **12**, adding `FlushDone { handle, result }` (`:500-506`). Both were added in `e56a8b4e6` (2026-08-07) for extent-manager `volatile_write_cache`; the behaviour is specified in block-device-spdk-nvme FR-031. `ReadWriteStats` also has `read_size_buckets` / `write_size_buckets` (`:159-161`), and the crate exports `IO_SIZE_BUCKETS` (`:139`, `src/lib.rs:99`), `size_bucket` (`:177`), `bucket_lower_bound` (`:193`) and `merge_from` (`:218`).
  - Severity: minor.
- **FR-018 — `DispatcherConfig` field count; `TierEventStats` not listed**
  - Spec (spec.md:318): "`DispatcherConfig`: 18-field configuration — …" (the list omits `store_backpressure_ms`).
  - Code: **19** fields (`src/idispatcher.rs:26-101`). `store_backpressure_ms` (`:60-68`, default 5000 at `:116`) is the field FR-008 `reserve_memory` (spec.md:179) relies on by name. `TierEventStats` (16 fields, `src/idispatcher.rs:319-463`, exported `src/lib.rs:37`) is named in FR-018a but missing from FR-018's type list.
  - Severity: minor.
- **FR-021 — `FormatParams` field count**
  - Spec (spec.md:350): "`FormatParams`: 10-field format configuration with defaults."
  - Code: **9** fields (`src/iextent_manager.rs:42-67`: data_disk_size, slab_size, max_extent_size, sector_size, region_count, metadata_alignment, instance_id, metadata_disk_ns_id, metadata_region_size).
  - Severity: minor.
- **FR-023 — `LookupConfig` field count**
  - Spec (spec.md:382): "`LookupConfig`: 10-field configuration … `group`, `quorum_pct`, `phase1_timeout`, `op_deadline`, `max_retry_rounds`, `max_keys_per_query`, `bind_ip`, `actor_cpu`, `discovery`, `node_endpoint`."
  - Code: **12** fields (`src/iremote_lookup.rs:29-79`). Adds `caller_wait: Option<Duration>` (`:50`, default `None`) and `connection_teardown_timeout: Duration` (`:57`, default 1000 ms, `:89`). Both are specified in remote-lookup spec 002 (lines 376, 480, 490).
  - Severity: minor.
- **Overview + FR-009 — `IMemoryTier` gating; feature count**
  - Spec (spec.md:28-29): "The crate has **two** Cargo features" (three are then listed); `spdk` gates "… `IMemoryTier` …". FR-009 header (spec.md:191): "(feature: spdk)".
  - Code: `IMemoryTier` is **not** gated. `define_interface!` at `src/imemory_tier.rs:53` has no `cfg`, and the export at `src/lib.rs:51` is unconditional. `Cargo.toml:13-19` declares three features.
  - Severity: minor (the spec claims more gating than exists).
- **NFR-002 — "GPU-specific code is gated behind `--features gpu`"** (spec.md:483)
  - Code: no `#[cfg(feature = "gpu")]` anywhere in `src/**` (grep: 0 matches). `gpu = []` is declared at `Cargo.toml:19`. The Overview's "Reserved for GPU-specific conditional compilation" (spec.md:30) is accurate; NFR-002 overstates.
  - Severity: minor.
- **NFR-004 — "All public types and methods have doc comments with runnable examples"** (spec.md:492)
  - Code: there are no doc examples at all in `src/idispatch_map.rs`, `src/imemory_tier.rs`, `src/ieviction_policy.rs`, `src/iextent_manager.rs`, `src/ipartition_table.rs` (0 code fences each). Some public items have no doc comment at all: `MemoryTierTelemetrySnapshot` fields (`src/imemory_tier.rs:13-15`), `ExtentManagerError` variants (`src/iextent_manager.rs:20-24`), `Extent` fields (`:12-14`), `WriteHandle` and its methods (`:95-146`), `FormatParams::new` (`:70`). Most `IDispatcher` examples are `no_run`.
  - Severity: moderate (CLAUDE.md coding convention; `cargo doc` does not enforce it, since `missing_docs` is not denied in `src/lib.rs`).
- **Implementation Notes — "formally verified properties documented in comments (IDispatchMap: 10 props, IDispatcher: 10 props, IExtentManager: 10 props, IGpuServices: 10 props, IMemoryTier: 10 props)"** (spec.md:545)
  - Code: `a93b5620f` (2026-08-20, "strip verification overclaims") removed those blocks from `imemory_tier`, `idispatcher`, `idispatch_map` and `iextent_manager`. Only `src/igpu_services.rs:231-246` remains ("10 properties, 19 verification conditions", pointing at `components/gpu-services/verif/`).
  - Severity: minor (stale documentation claim).
- **Backfill banner — "Deferred (not applied): FR-014/FR-025 `IExtendedMetadataStore` remains an orphaned module … never declared/re-exported from `lib.rs`"** (spec.md:17-22)
  - Code: declared at `src/lib.rs:83` and re-exported at `src/lib.rs:105`; resolved per `.specify/sync/align-tasks.md` (2026-08-20). The banner is written in the present tense and now contradicts FR-014's own Aligned status.
  - Severity: minor.

#### Not Implemented ✗

- None.

#### Not re-verified ⏸

- SC-2 (`cargo build` without spdk), SC-3 (`cargo build --features spdk`), SC-4 (`cargo doc --no-deps` warning-free), SC-7 (unit tests). This session was not permitted to run `cargo`. The last recorded evidence is `.specify/sync/apply-report.md` (2026-10-05: interfaces 78 tests passed, clippy clean). This branch changed only a doc comment in `src/` (`git diff -- src`: +6 lines, all `///`), so a regression is unlikely but **not shown**. SC-4's doc half is in any case contradicted by NFR-004 above.

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `IDispatcher::batch_populate` | `src/idispatcher.rs:679-688` | 10 | 001-interfaces FR-008 (Proposal 3) |
| `IDispatcher::tier_event_stats` + `TierEventStats` type | `src/idispatcher.rs:314-463, 877-883` | 157 | 001-interfaces FR-008 / FR-018 (Proposals 2, 3) |
| `Command::FlushSync` / `Completion::FlushDone` | `src/iblock_device.rs:408-414, 500-506` | 14 | 001-interfaces FR-017 (Proposal 1) |
| `ReadWriteStats` IO-size histograms, `IO_SIZE_BUCKETS`, `size_bucket`, `bucket_lower_bound`, `merge_from` | `src/iblock_device.rs:125-260` | ~135 | 001-interfaces FR-017 (Proposal 1) |
| `LookupConfig::{caller_wait, connection_teardown_timeout}` | `src/iremote_lookup.rs:44-57` | 14 | 001-interfaces FR-023 (Proposal 8) |

None warrants a NEW_SPEC: each extends a type or trait this spec already owns.

## Inter-Spec Conflicts

1. **interfaces `IDispatcher` contract vs. dispatcher-p2p implementation.** `flush_to_ssd`, `clear_memory_tier` and `remove` in `dispatcher-p2p/src/lib.rs:2889-2902, 2833-2864, 2357-2386` do not meet the contract this branch documented and repaired in `dispatcher`. See FR-008(c).
2. **interfaces `clear_memory_tier` doc (src/idispatcher.rs:808-809) vs. dispatcher FR-038 (approved 2026-10-20).** The trait doc says in-flight entries are removed; dispatcher FR-038 says a pinned entry "MUST keep its slot and is retried … `Timeout`".
3. **FR-018a "`store_already_resident` MUST be counted" vs. dispatcher-p2p.** `dispatcher-p2p/src/lib.rs:2932` lists `store_already_resident` as "NOT MEASURED" (left at default 0). The comment documents this deliberately, but FR-018a is worded for every `TierEventStats` producer. Recorded, not proposed: whether FR-018a binds dispatcher-p2p is a scope decision for the dispatcher-p2p spec.

## Recommendations

1. Fix the `clear_memory_tier` and `batch_lookup` doc comments in `src/idispatcher.rs` on this branch (Proposals 5, 6). They are doc-only, and the first is a direct consequence of `cccfb328e`.
2. Open a dispatcher-p2p repair for the three contract violations (Proposal 7), reusing the dispatcher repairs `0236a3c98`, `cccfb328e` and `9bd231d85`. Do not paper over it in the interface doc.
3. Apply the ten BACKFILLs (Proposals 1-4, 8-13) together; they are text-only and code-authoritative.
4. Track NFR-004 (Proposal 14) as a doc-examples task, or soften NFR-004 if the team accepts `no_run` and missing examples on trait methods.
5. Re-run `cargo build -p interfaces`, `cargo build -p interfaces --features spdk`, `cargo test -p interfaces` and `cargo doc -p interfaces --no-deps` before re-stamping, to close SC-2/3/4/7.
