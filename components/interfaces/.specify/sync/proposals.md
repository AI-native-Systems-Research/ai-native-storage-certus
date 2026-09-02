# Sync Proposals: interfaces

**Project**: interfaces
**Spec**: `components/interfaces/specs/001-interfaces/spec.md`
**Source of truth for backfill**: the trait/type definitions in `components/interfaces/src/`.

Each proposal is classified **BACKFILL** (code is correct → update spec), **ALIGN**
(spec is correct → code must change; recorded as a task, no `.rs` edited here), or
**HUMAN_DECISION** (intent unclear → leave for a human).

## BACKFILL (spec → matches code) — `approved: true`

All of these are cases where the trait/type definitions are the authoritative,
shipping behaviour and the spec text has simply fallen behind. Confident to apply.

- **P1 — FR-008 `batch_lookup` signature.** Update spec.md:175 to `batch_lookup(&self, entries: &[(CacheKey, Vec<IpcHandle>)]) -> Vec<Result<(), DispatcherError>>` and note the one-or-more-regions scatter semantics. Evidence: `src/idispatcher.rs:338-341`.
- **P2 — FR-008 `copy_gpu_to_memory_async` signature.** Update spec.md:180 to `copy_gpu_to_memory_async(&self, key: CacheKey, regions: &[IpcHandle], stream: GpuStream)` with the contiguous-gather note. Evidence: `src/idispatcher.rs:460`.
- **P3 — FR-008 add `batch_populate`.** Add method entry to FR-008. Evidence: `src/idispatcher.rs:427`.
- **P4 — FR-008 add `tier_event_stats`.** Add method entry to FR-008. Evidence: `src/idispatcher.rs:575`.
- **P5 — FR-018 add `TierEventStats`.** Add supporting-type entry (Copy+Default+PartialEq+Eq, 4 u64 fields). Evidence: `src/idispatcher.rs:190-202`, `src/lib.rs:33`.
- **P6 — FR-017 `Command` 12 → 13 variants (add `FlushSync`).** Evidence: `src/iblock_device.rs:322,411`.
- **P7 — FR-017 `Completion` 11 → 12 variants (add `FlushDone`).** Evidence: `src/iblock_device.rs:439,501`.
- **P8 — FR-017 `ReadWriteStats` histograms + `IO_SIZE_BUCKETS`.** Document `read_size_buckets`/`write_size_buckets`, the `IO_SIZE_BUCKETS = 25` const (re-exported `src/lib.rs:94`), and the `size_bucket`/`bucket_lower_bound`/`merge_from` helpers. Evidence: `src/iblock_device.rs:139,159-161,177,193,218`.
- **P9 — FR-021 `FormatParams` "10-field" → "9-field".** Correct the count and enumerate the 9 fields. Evidence: `src/iextent_manager.rs:42-67`.
- **P10 — FR-023 `LookupConfig` "10-field" → "12-field".** Correct the count; add `caller_wait` and `connection_teardown_timeout`. Evidence: `src/iremote_lookup.rs:29,50,57`.
- **P11 — FR-014/FR-025 refresh stale note.** The top-of-file "Last Synced 2026-08-07 … Deferred (not applied)" note claims `IExtendedMetadataStore` is still an orphaned module. It is now wired in (`src/lib.rs:78,100`). Rewrite the note to record the resolution. Evidence: `src/lib.rs:78,100`, `src/iextended_metadata_store.rs:5,30`.
- **P12 — New FR-035 for `IIpcServer` (unspecced backfill).** Add an FR documenting the interface (initialize/serve/shutdown/metrics_snapshot) and its supporting types (`IpcServerConfig` 4-field, `IpcError` 4-variant, `IpcMetricsSnapshot` 5-field), **with an explicit orphaned-module caveat** noting it is not yet declared/re-exported in `src/lib.rs` and that the `ipc-component` consumer is a latent build break until it is (see ALIGN-IFACE-001). Evidence: `src/iipc.rs:40-192`. This mirrors how FR-014 was historically documented while orphaned.

## ALIGN (code → matches spec) — recorded as task, `approved: true` for the task, no `.rs` edited

- **P13 — ALIGN-IFACE-001: wire `iipc` into `src/lib.rs`.** The intended contract (evidenced by the fully-documented interface, its tests, and a real consumer) is that `IIpcServer` et al. are part of the exported `interfaces` API. The code defect is the missing `mod iipc;` + `pub use`. Fixing it is a `.rs` change, which this sync must not make, so it is appended to `align-tasks.md`. Evidence: `src/lib.rs` (missing decl), `src/iipc.rs`, `components/ipc-component/src/lib.rs:33`.

## HUMAN_DECISION

- None. All findings are unambiguous documentation-vs-shipping-code drift or a clear
  orphaned-module code defect with a single obvious fix.
