---
spec_sync_component: interfaces
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-09T21:53:37Z
spec_sync_git_commit: 83bcd4ff
spec_sync_inputs_sha256: 3ee6dd139459882a27132a6d93279cbb1079f416aeba02a68efe11ce23a691ed
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
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

# Drift Report: interfaces

**Generated**: pending
**Project**: interfaces

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 47 |
| Aligned | 45 |
| Drifted | 2 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

Spec: `001-interfaces` — Shared Interface Trait Definitions (34 FR + 6 NFR + 7 SC). "Implementation" here means the trait/type definitions themselves. The spec was self-synced 2026-08-07 and explicitly defers the one substantive drift below.

## Detailed Findings

### Spec 001-interfaces — Shared Interface Trait Definitions

**Aligned ✓** (spot-verified against source)
- FR-002 `ILogger` (error/warn/info/debug) — `src/ilogger.rs:3-10`; re-exported `src/lib.rs:45`
- FR-004 `IBlockDevice` incl. `read_write_stats` — `src/iblock_device.rs:589`
- FR-006 `IEvictionPolicy::track(..., semantics: BlockSemantics)` + FR-020 `BlockSemantics`/`SessionId` — exported `src/lib.rs:33-38`
- FR-007 `IDispatchMap::set_checksum`/`get_checksum` behind `integrity-check` — `src/idispatch_map.rs:286-295`
- FR-017 `ReadWriteStats`, FR-018 `LookupResult` (4-variant) — present/exported
- FR-030/FR-031/FR-033/FR-034 RDMA initiator/responder split, `push_async`/`PushCompletion` — exported `src/lib.rs:52-60`
- Cargo features `spdk`, `gpu`, `integrity-check` all declared — `Cargo.toml:17-19` (matches Overview / NFR-002)
- Remaining FR-001, FR-003, FR-005, FR-008..FR-013, FR-015, FR-016, FR-019, FR-021..FR-024, FR-026..FR-029, FR-032, and NFR-001..006 — modules present and re-exported per `src/lib.rs`

**Drifted ⚠️**
- FR-014 `IExtendedMetadataStore` interface — **major**
  - Spec: FR-014 defines the `IExtendedMetadataStore` trait (put/get/delete/iterate_all/force_flush) as part of the crate.
  - Actual: the trait is defined in `src/iextended_metadata_store.rs:30-47`, but that module is **never declared** (`mod`) nor re-exported (`pub use`) in `src/lib.rs`. Grep confirms no reference to the file anywhere else in `src/`. The interface is therefore not part of the compiled crate. The consumer `extended-metadata-store` does `use interfaces::IExtendedMetadataStore` (`../extended-metadata-store/src/lib.rs:21`) and would fail to build; the break is masked only because that crate is excluded from the workspace. Documented as "Deferred (not applied)" in the spec's 2026-08-07 sync note.
  - Location: `src/lib.rs` (missing `mod`/`pub use`), definition at `src/iextended_metadata_store.rs:30`
- FR-025 `ExtendedMetadataStoreError` supporting type — **major** (same root cause)
  - Spec: 4-variant enum `ExtendedMetadataStoreError` (NotFound, StorageError, CapacityExhausted, ValueTooLarge) exported from `interfaces`.
  - Actual: the enum exists with exactly those 4 variants (`src/iextended_metadata_store.rs:5-15`) but, living in the undeclared module, it is **not exported** from the crate. Same orphaned-module cause as FR-014.
  - Location: `src/iextended_metadata_store.rs:5`

**Not Implemented ✗**
- None.

## Unspecced Features

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| (none) | | | |

## Recommendations
1. Add `mod iextended_metadata_store;` and `pub use iextended_metadata_store::{IExtendedMetadataStore, ExtendedMetadataStoreError};` to `src/lib.rs` to make FR-014/FR-025 real. This is a one-line-each fix that removes the latent compile break for the `extended-metadata-store` consumer and is a prerequisite for bringing that crate into the workspace.
2. After wiring the module, confirm `cargo build`/`cargo build --features spdk` still pass and the consumer crate compiles.
