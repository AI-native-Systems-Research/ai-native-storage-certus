# Spec Sync Apply Report
Applied: 2026-07-21
Project: dispatcher-p2p
Spec: 001-gpudirect-cold-path
Base commit: 833e9f36e01f1df8a0e0fc57d5cd223d823d3199

## Actions
1. Backed up spec.md → `.specify/sync/backups/spec.md.bak`.
2. Added **FR-015** to `specs/001-gpudirect-cold-path/spec.md` (after FR-014), documenting that the `IGpuServices` receptacle now exposes `set_device`/`device_of_ptr` for multi-GPU device selection. Wording explicitly scopes this as an interface keep-up: the capability is present in the receptacle/mock (only the test mock implements it), and the production cold path does NOT yet route transfers by device (per-device routing not wired into `pipelined_ssd_to_gpu_p2p`).
3. Marked Proposal 1 (`FR-015`) `"approved": true` in `proposals.json`.

## Result
| Proposal | Requirement | Direction | Status |
|----------|-------------|-----------|--------|
| 1 | FR-015 | BACKFILL (code authoritative) | Applied |

- spec.md functional requirements: FR-001..FR-014 → FR-001..FR-015.
- No existing FR modified; no conflicts.
- Backup available at `.specify/sync/backups/spec.md.bak` for rollback.

---

# Spec Sync Apply Report — Cycle 2
Applied: 2026-07-22
Project: dispatcher-p2p
Spec: 001-gpudirect-cold-path
Mode: AUTO-BACKFILL
Source: `.specify/sync/drift-report.{json,md}` generated 2026-07-22T22:35:42Z

## Summary

| Category | Count |
|----------|-------|
| Specs analyzed | 1 |
| FR/SC checked | 21 (all aligned pre-apply) |
| Drifted items fixed (data-model.md only) | 2 |
| Unspecced features backfilled (new FRs + entities in spec 001) | 2 |
| New specs created | 0 |
| Spec superseded | 0 |
| Align-tasks deferred | 1 |

## Decision: BACKFILL into existing spec, not a new spec

Both unspecced features (`P2pColdReadPool` cold-path worker pool, eviction-event notification
channel) were backfilled as new FRs + Key Entities into `001-gpudirect-cold-path` rather than
split into a new spec directory. Rationale:
- The cold-path worker pool is an execution-model change to the *same* cold-read path already
  specified by FR-004/FR-005/FR-011 (it changes how cold-path jobs are dispatched, not what the
  cold path does); it has no independent user story or acceptance scenario distinct from the
  existing "Cold Lookup Completes via P2P Path" story.
- The eviction-event channel is small (~110 LOC), has no confirmed external consumer yet (see
  align-tasks.md), and does not yet have its own success criteria — insufficient to justify a
  standalone spec today. Flagged in align-tasks.md as a candidate for a future
  `002-eviction-telemetry` spec once a consumer contract is confirmed.

## Actions

1. Backed up `spec.md` → `.specify/sync/backups/spec.md.20260722T232132Z.bak`.
2. Backed up `data-model.md` → `.specify/sync/backups/data-model.md.20260722T232132Z.bak`.
3. **BACKFILL** — Added **FR-016** to `spec.md` (after FR-015): persistent per-drive
   `P2pColdReadPool` cold-path worker pool as the primary cold-read execution model, with the
   pre-existing inline per-batch connect as an explicit degraded fallback when pool creation
   fails at init; shutdown ordering (pool stopped before P2P ring destroyed).
4. **BACKFILL** — Added **FR-017** to `spec.md` (after FR-016): eviction-event notification
   channel (`create_eviction_channel`, `EvictionEvent`/`EvictionReason`, non-blocking `try_send`,
   drop-and-count backpressure via `eviction_dropped_count`).
5. **BACKFILL** — Added **Key Entities** `P2pColdReadPool` and `EvictionEvent / EvictionReason`
   to `spec.md`.
6. **DRIFT FIX (code-authoritative)** — `data-model.md`: `P2pRing.streams` corrected from
   `[GpuStream; 2]` to `[GpuStream; 4]` (matches `NUM_STREAMS = 4` in `src/p2p_ring.rs:30` and
   `spec.md` FR-003/FR-005, which were already correct).
7. **DRIFT FIX (code-authoritative)** — `data-model.md`: `PathSelection` section rewritten. The
   original single-enum `OnceLock<PathSelection>` description did not match the code; replaced
   with the actual two-field model (`p2p_ring: RwLock<Option<P2pRing>>`,
   `pipeline_ring: RwLock<Option<PipelineRing>>`, both settable independently at init, path chosen
   per call). Updated the `Relationships` section accordingly.
8. **BACKFILL (data-model)** — Added `P2pColdReadPool` and `EvictionEvent / EvictionReason`
   entities to `data-model.md`, and two new `Relationships` bullets describing how the pool and
   the eviction channel attach to `DispatcherP2pComponent`.
9. **DEFER** — Appended one item to `align-tasks.md`: the eviction-event channel's downstream
   consumer is unconfirmed (drift report speculates a gRPC `TakeEvents` RPC in sibling commit
   `4d5bd13`, not verified in this component's tree). FR-017 was written to document only the
   producer-side contract to avoid asserting an unverified consumer.

## Result

| Item | Direction | Status |
|------|-----------|--------|
| Cold-path worker pool (`cold_pool.rs`) | BACKFILL → FR-016 + Key Entity + data-model entity | Applied |
| Eviction-event channel | BACKFILL → FR-017 + Key Entity + data-model entity | Applied |
| `data-model.md` `P2pRing.streams` (2 vs 4 streams) | Drift fix, code-authoritative | Applied |
| `data-model.md` `PathSelection` representation | Drift fix, code-authoritative | Applied |
| Eviction-event consumer contract | AMBIGUOUS | Deferred → `align-tasks.md` |

- `spec.md` functional requirements: FR-001..FR-015 → FR-001..FR-017.
- `data-model.md`: 5 entities → 7 entities (`P2pRing`, `ThreadPartition`, `PipelineRing`,
  `LookupResult`, `PathSelection` [corrected], `P2pColdReadPool` [new], `EvictionEvent /
  EvictionReason` [new]).
- No source code touched; only `spec.md`, `data-model.md`, and `.specify/sync/` Markdown were
  edited.
- Backups available at `.specify/sync/backups/spec.md.20260722T232132Z.bak` and
  `.specify/sync/backups/data-model.md.20260722T232132Z.bak` for rollback.
