# Spec Sync Proposals
Generated: 2026-08-31
Project: dispatcher
Spec: 001-dispatcher-cache-interface
Source: `.specify/sync/drift-report.json`
Branch: `sync-tmp`

Summary: 6 BACKFILL (5 drifted requirements + 1 unspecced), all APPROVED interactively. No ALIGN, no HUMAN_DECISION. Editing scope: `specs/**` only (source untouched).

---

## Proposal 1 — FR-037 (single warm stream → dual per-device load/store) [APPROVED]

- **Direction**: BACKFILL (code authoritative — spec wording stale)
- **Requirement**: FR-037 (+ FR-052 stream inventory)
- **Commit**: `995596e4`
- **Rationale**: `DeviceStreams` now holds two warm streams per device — `warm_load` (H2D)
  and `warm_store` (D2H) — split so H2D and D2H DMA overlap on the PCIe bus
  (`src/lib.rs:335-345`); `device_streams_for` creates both plus the pipeline pair
  (`:371-378`). The spec described "a single warm CUDA stream".
- **Before**: "pre-allocate a single warm CUDA stream … used … for `memcpy_h2d_async` … the per-device warm stream from `DEVICE_STREAMS`".
- **After**: per-device `warm_load` (H2D) + `warm_store` (D2H) streams, split for PCIe bidirectional overlap; H2D via `memcpy_batch_async` on `warm_load`, D2H on `warm_store`; `warm_stream` AtomicU64 fallback retained.

---

## Proposal 2 — FR-052 (per-device stream inventory) [APPROVED]

- **Direction**: BACKFILL (code authoritative)
- **Requirement**: FR-052
- **Commit**: `995596e4`
- **Rationale**: `DeviceStreams { warm_load, warm_store, pipe: [u64; 2] }` (`src/lib.rs:339-345`)
  is two warm streams + a pipeline pair per device, not "one warm stream plus one pipeline
  stream pair".
- **Before**: "one warm stream plus one pipeline stream pair per device".
- **After**: "two warm streams (`warm_load` for H2D and `warm_store` for D2H, FR-037) plus one pipeline stream pair per device".

---

## Proposal 3 — FR-006 / FR-039(2) / FR-056 / Implementation Notes (warm-path copy API) [APPROVED]

- **Direction**: BACKFILL (code authoritative)
- **Requirement**: FR-006, FR-039 step (2), FR-056 (`copy_gpu_to_memory_async`), Implementation Notes
- **Commit**: `d8c26d58`
- **Rationale**: Both warm directions now use the batched `IGpuServices::memcpy_batch_async`
  (`cuMemcpyBatchAsync`) multi-region API: memory-tier→GPU scatter (1 slot → N regions) on
  `warm_load` (`src/lib.rs:1116-1127`); GPU→memory-tier gather (N regions → 1 slot) in
  `copy_gpu_to_memory_async` on `warm_store` (`src/lib.rs:2998-3009`). `memcpy_h2d_async`
  survives only in the test mock (`src/lib.rs:4193,4260`) and one assertion (`:5244`).
- **Before**: warm-path H2D via `memcpy_h2d_async`; `copy_gpu_to_memory_async` "issues asynchronous GPU→host DMA on the supplied stream".
- **After**: batched `memcpy_batch_async` scatter (H2D on `warm_load`) / gather (D2H on `warm_store`).

---

## Proposal 4 — FR-040 (stale gRPC reference) [APPROVED]

- **Direction**: BACKFILL (code authoritative)
- **Requirement**: FR-040
- **Commit**: `97e26738`
- **Rationale**: gRPC removed entirely; shm-queue is the sole control transport. The
  `promote_to_memory_tier` API is unchanged — only the invoking transport changed.
- **Before**: "The gRPC handler spawns this as a detached background task when `BatchTouchRequest.promote = true`."
- **After**: "The shm-queue control transport (the sole control transport since gRPC was removed) spawns this as a detached background task when a batch-touch request sets `promote = true`."

---

## Proposal 5 — FR-042 (stale gRPC reference) [APPROVED]

- **Direction**: BACKFILL (code authoritative)
- **Requirement**: FR-042
- **Commit**: `97e26738`
- **Rationale**: The eviction-event channel is unchanged; the named example consumer
  (gRPC TakeEvents) was removed. The shm-queue transport is now the consumer.
- **Before**: "external consumers (e.g., gRPC TakeEvents stream) to observe cache evictions without polling."
- **After**: "external consumers (e.g., the shm-queue TakeEvents stream) to observe cache evictions without polling."

---

## Proposal 6 — FR-058 + SC-017 (tier_event_stats) [APPROVED]

- **Direction**: BACKFILL-UNSPECCED (new FR + SC)
- **Requirement**: new FR-058, new SC-017
- **Commit**: `4659626b`
- **Rationale**: `IDispatcher::tier_event_stats() -> TierEventStats` (`idispatcher.rs:564`;
  struct `:189-210`) is implemented with a lock-free `TierEventCounters` subsystem
  (`src/lib.rs:111-160`, `tier_counters` field `:317`, trait impl `:3390`) but no
  requirement covers it. Four monotonic `u64` counters: `promotions_to_memory`,
  `promotions_to_gpu`, `evictions_from_memory`, `evictions_from_ssd`. Always-on (unlike
  telemetry-gated `read_write_stats`); non-tiering variants return zeros.
- **Before**: (none — feature unspecced)
- **After**: New **FR-058** (tier-event counters) + **SC-017** (counters zero at startup, monotonic, delta = tier events in window; non-tiering variant returns zeros).

---

## Not proposed (already resolved / out of scope)

- **US11 queue-depth contradiction** — resolved in the 2026-08-20 sync (`max_queue_depth = 128` throughout). Aligned.
- **CLAUDE.md path / `-v2` names** — already corrected in the current tree; also outside editing scope. Aligned.
- **DI/test surface** — already specced as FR-057 / SC-016 (2026-08-20). No action.
- **Two `src/lib.rs` "gRPC handler" source comments** — source is outside this sync's editable scope; flagged for a follow-up source-comment cleanup with the `97e26738` rationale.
