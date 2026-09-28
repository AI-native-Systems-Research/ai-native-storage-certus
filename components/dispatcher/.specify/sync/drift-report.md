---
spec_sync_component: dispatcher
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-25T17:13:57Z
spec_sync_git_commit: 13caf87e
spec_sync_inputs_sha256: 9c0c88f545acf08a38390f88ea45d359ca6fb8300f8b1e39d00d4348019f1627
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Re-stamp 2026-09-25 (SSD-evictor drive-selection bug fix; drift resolved by backfill).** Branch `fix/ssd-evictor-drive-index-hash`. The background SSD-utilization evictor (`background.rs`, `run_ssd_eviction`) selected the extent manager to free from with a raw `key % num_drives`, while placement/write-through and the inline `remove(key)` path use the splitmix64 `drive_index(key, num_drives)` (FR-039(4), FR-009). The two disagree for almost all keys when `num_drives > 1`, so the evictor freed the wrong extent manager — leaking the intended extent and potentially freeing a live extent for another key on the wrong drive. CODE (authoritative): both call sites now route through `drive_index` (made `pub(crate)`). SPEC BACKFILL: the Session Q&A "How does the SSD evictor determine drive ownership for extent removal?" previously answered `key % num_drives` "matching the write-through path" — a claim that was false pre-fix; it was rewritten to describe `drive_index` and record the prior drift. `src/lib.rs` + `src/background.rs` changed (moved the digest); `spec.md` Q&A changed (moved it again). Digest recomputed over a clean tree matching CI; drift status `clean`.
> **Stamp provenance.** `spec_sync_git_commit` is the HEAD (`e4b97a0b`) the digest
> was computed against; this report is committed together with the `spec.md`
> backfill, the `src/lib.rs` edits, and the `src/cold_pool.rs` deletion it
> certifies, so the tree the next commit lands is exactly the tree the digest
> covers. The CI Spec-Sync Gate re-runs `scripts/spec-sync-hash.sh
> components/dispatcher` over `src/**` + `specs/**` (with `components/interfaces/`
> folded in) and matches `spec_sync_inputs_sha256`; the digest here was recomputed
> over a clean tree (no untracked files under `src/`/`specs/`) after the last edit.

# Spec Drift Report — dispatcher

Generated: 2026-09-21
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)
Mode: Read-only drift analysis, then apply — **BACKFILL** to `spec.md` (Finding 1,
code authoritative) plus a **CODE deletion** of dead `ColdReadPool` scaffolding
(Finding 2, user-directed).
Branch: `evolve-throughput`

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Drift findings this sweep | 2 |
| ⚠️ Drifted → resolved by backfill (spec→code) | 1 (Finding 1: scatter-gather batch cold path) |
| 🧹 Dead code → resolved by deletion (code) | 1 (Finding 2: unused `ColdReadPool`) |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 0 (after this sweep; `scatter_gather_multi_drive_zero_copy` now specced as FR-061) |

Scope of this sweep: since the last clean sync the only input-changing commit on
this component's data path is **`6f35b13a`** ("optimizer iter-005:
iter005-prop001-scatter-gather"), which rewrote `batch_lookup`'s cold (BlockDevice)
promotion path. All prior sweeps' findings (store backpressure + drop-on-full
FR-060; Check→Pin graceful degrade FR-024/FR-039; per-device streams FR-037/FR-052;
coalesced per-object H2D FR-019) were re-verified against the current source and
remain **aligned** — see "Regression re-verification" below.

## Detailed Findings

### 1. `batch_lookup` cold path: per-drive threads + `ColdReadPool` → single-thread multi-drive scatter-gather — severity: major — RESOLVED BY BACKFILL (code authoritative)

- Commit: `6f35b13a`.
- New shipped code (`src/pipeline.rs`):
  - `pub struct DriveWork<'a> { channels: &ClientChannels, drive: &dyn IBlockDevice, jobs: &[ColdReadJob] }` (`src/pipeline.rs:746`).
  - `pub unsafe fn scatter_gather_multi_drive_zero_copy(gpu, streams: &[GpuStream; 2], drive_works: &[DriveWork], chunk_size, max_queue_depth, metrics) -> Vec<Vec<Result<(), DispatcherError>>>` (`src/pipeline.rs:766`). Fans NVMe reads out to all drives and fans completions in via **one multiplexed poll loop on the caller thread**; distributes `max_queue_depth` proportionally per drive by segment count (`src/pipeline.rs:848`); preserves coalesced per-object H2D and per-object completion routing.
- Call site (`src/lib.rs`, `batch_lookup` cold path):
  - Group by drive via `Self::drive_index(entry.key, num_drives)` (`src/lib.rs:2341`), which is a **splitmix64 finalizer mod `num_drives`** (`src/lib.rs:494-503`), *not* the raw `key % num_drives` the pre-sync FR-039(4) claimed.
  - Per-drive cold-prep builds `pipeline::ColdReadJob`s via `evict_and_insert`; `AllocationFailed` entries defer to the staging post-pass (`src/lib.rs:2385-2411`).
  - Checks out **one** `ChannelLease` per active drive from that drive's `ChannelPool` (`src/lib.rs:2468-2481`), builds one `DriveWork` per drive, and calls `scatter_gather_multi_drive_zero_copy` **once** on the caller thread (`src/lib.rs:2492-2501`). No `std::thread::scope`, no `MAX_QUEUES_PER_DRIVE`, no `pipelined_multi_object_zero_copy` per thread.
  - Uses the per-device pipeline stream **pair** `dev_streams.pipe[0]/[1]` (`src/lib.rs:2431-2461`), falling back to temporary created streams.
  - Staging post-pass unchanged in intent — `serve_cold_staged` one lease at a time (`src/lib.rs:2553-2566`).
- Spec (pre-sync) described the superseded model: FR-039(4)/(5), FR-019 ("`max_queue_depth=128` per thread"), User Story 11 narrative + scenarios 1 & 3 ("per-drive thread groups", "`MAX_QUEUES_PER_DRIVE` … threads per drive", "single thread per drive"), Key Entities "Pipelined Reader", and the Session Q&A all described spawning per-drive threads each running `pipelined_multi_object_zero_copy`.
- Why code is authoritative: `6f35b13a` is the shipped throughput optimization on `evolve-throughput`; the scatter-gather loop is what runs today and the per-drive-thread model no longer exists in source. Direction confirmed with the user: **backfill spec → scatter-gather.**
- Backfill applied to `spec.md`:
  - **New FR-061** — specifies `scatter_gather_multi_drive_zero_copy` + `DriveWork`: single caller-thread multiplexed loop, proportional `max_queue_depth` split by segment count, coalesced per-object H2D on the per-device pipe stream pair, per-drive drain-all-on-error (FR-054), results indexed by drive then job.
  - **FR-039** steps (4)/(5) rewritten to the checkout-one-channel-per-drive + single scatter-gather model; `key % num_drives` corrected to the splitmix64-hash-mod-`num_drives` that `drive_index` computes.
  - **FR-019** last sentences: `promote_and_serve` (single-entry) still uses `pipelined_multi_object_zero_copy`; `batch_lookup` now uses `scatter_gather_multi_drive_zero_copy`.
  - **FR-052** — `cold_pool::ColdReadRequest { gpu_device }` device-selection sentence replaced with the caller-thread batch-device → pipe-stream-pair path.
  - **User Story 11** — narrative + acceptance scenarios 1 & 3 rewritten to the scatter-gather model (no threads, proportional queue-depth share).
  - **Key Entities "Pipelined Reader"** + **Session Q&A** — scatter-gather listed as the primary batch cold variant.
  - New **Last Synced: 2026-09-21** metadata line.

### 2. Unused `ColdReadPool` scaffolding — severity: moderate (dead code) — RESOLVED BY DELETION (user-directed)

- After `6f35b13a` routed the batch cold path through scatter-gather, the `ColdReadPool` (module `src/cold_pool.rs`: `ColdReadPool`, `ColdReadRequest`, `Drop`) was still **constructed** in `initialize()` and **torn down** in `shutdown()` but was **never submitted to** — the batch cold path builds `DriveWork` and calls `scatter_gather_multi_drive_zero_copy` directly. Verified: no `submit`/enqueue call to the pool remained in `batch_lookup` or elsewhere in `src/lib.rs`.
- FR-044 (pre-sync) still *required* a `ColdReadPool` ("The dispatcher MUST provide a `ColdReadPool` … `batch_lookup` dispatches cold entries to the pool"). This was drift: a load-bearing requirement whose subject was dead in the implementation.
- User decision (interactive step): **"Remove ColdRealPool redundant code."** — i.e. delete the dead scaffolding rather than merely document it.
- Code change applied (`src/lib.rs`, `src/cold_pool.rs`):
  - Removed `pub mod cold_pool;`.
  - Removed the `cold_pool: Mutex<Option<cold_pool::ColdReadPool>>` field from the `define_component!` `fields: {}` block.
  - Removed the `initialize()` construction block (`ColdReadPool::new(...)`, `COLD_POOL_QUEUES_PER_DRIVE`).
  - Removed the `shutdown()` teardown block (`pool.shutdown()`).
  - `git rm src/cold_pool.rs`.
  - Fixed the resulting positional-constructor arity: the `define_component!`-generated `DispatcherComponent::new(...)` dropped one field, so all 23 test call sites had one `Mutex::new(None)` (the `cold_pool` slot) removed via a uniform `replace_all` edit.
- Spec change: **FR-044 marked `~~REMOVED~~`** (superseded 2026-09-21, `6f35b13a`) with a pointer to FR-034 (per-drive `ChannelPool`) + FR-061 (scatter-gather) for cold-read resource management. FR-052's `cold_pool::ColdReadRequest` reference removed (see Finding 1).
- Verification: `grep -rn 'cold_pool|ColdReadPool|ColdReadRequest|COLD_POOL' components/dispatcher/src` → none. `cargo check -p dispatcher --tests` → clean (arity break resolved, no other references). **Note:** `components/dispatcher-p2p` has a *separate* `P2pColdReadPool` that is still actively used — it was deliberately **not** touched (out of scope).
- Stale source comments corrected in the same pass (no behavior change): module threading-model doc (`src/lib.rs:46`), the cold-promotion comment (`src/lib.rs:2291`), and the staging post-pass comment's `pool_guard` reference (`src/lib.rs:2548`).

## Regression re-verification (prior sweeps still aligned)

- **FR-060** (store backpressure + drop-on-full): `reserve_memory` retry loop and `populate`/`batch_populate` unconditional drop-on-full intact.
- **FR-024 / FR-039(1)/(3)** (Check→Pin graceful degrade, `1d55b9c2`): skip-not-drop eviction and the removal of the single-entry inline `promote_and_serve` fast path intact; single-key cold still takes the pooled (now scatter-gather) path.
- **FR-034** (per-drive `ChannelPool` + RAII `ChannelLease`, completion-drain on checkout): now the *sole* cold-read channel manager after `ColdReadPool` deletion; scatter-gather checks out exactly one lease per active drive.
- **FR-037 / FR-052** (per-device warm/store streams + pipeline pair): scatter-gather consumes the per-device pipe pair; warm/store unchanged.
- **FR-053** (cold-load staging fallback) and **FR-054** (drain-all-on-error, no early break): staging post-pass and per-drive drain semantics preserved in the scatter-gather loop.

## Not Implemented
None.

## Unspecced Code
None after this sweep. `scatter_gather_multi_drive_zero_copy` and `DriveWork` are now specified by FR-061; the retired `ColdReadPool` is deleted.

## Recommendations
1. Commit this `drift-report.md` (with the freshness stamp above) together with the
   `spec.md` backfill, the `src/lib.rs` edits, and the `src/cold_pool.rs` deletion it
   certifies, so the CI Spec-Sync Gate sees a fresh, matching report.
2. Follow-up (carried over from earlier sweeps) — ✅ **RESOLVED**: the two stale
   transport-example source comments in `copy_gpu_to_memory_async` were removed in
   commit `6d7ba234` ("revert dispatcher GPU experiments to specified stream
   model"). Verified this sweep: `grep -rin grpc components/dispatcher/src` returns
   nothing, and the canonical spec's FR-040 / FR-042 already describe the shm-queue
   control transport (`2026-08-31` sweep). `align-tasks.md` T1 is closed. No
   dispatcher source or spec references to the removed control transport remain;
   residual mentions live only in dated `.specify/sync/` changelog/backup records
   and unrelated `certus-server` gRPC-server specs, which are left intact as
   historical / legitimate.
