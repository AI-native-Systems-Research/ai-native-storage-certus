---
spec_sync_component: dispatcher
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-01T00:00:00Z
spec_sync_git_commit: 72c7182b
spec_sync_inputs_sha256: f3e895dfb7a1641be202aadbf175934db7ea2bc5f65a4226d0d681ecb4f677e1
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
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

> **Re-stamp 2026-09-29 (served_by Phases 2-3).** `ServedBy` + `LookupOutcome` in
> `interfaces`, `batch_lookup` widened, the `LOOKUP` byte carrying the tier, and
> `TierEventStats` from eight fields to ten (the route partition). Digest 127cc2e8f652….

> **Re-stamp 2026-09-29 (feature 002 Phase 1: counters, FR-058 sync, taxonomy
> collapse).** Branch `certus-lookup-observability`, layered on the accounting fixes
> re-stamped above. Adds the two remote-lookup counters to `TierEventStats` (so FR-058
> goes from six fields to eight), the responder-side serve counters, and the collapse
> of the two remote taxonomy values into one. The appended sweep below carries the
> findings and, importantly, the reason its method differs from the workload
> generator's: this component cites 4 requirement ids across 120 defined, so
> citation-counting proves nothing here.

> **Re-stamp 2026-09-29 (lookup-accounting fixes; spec backfilled with the behaviour
> change).** Branch `fix/lookup-accounting`. Two defects in the server's lookup
> accounting: `batch_lookup` collapsed every `RemoteLookupError` into `IoError`, so a
> key no peer holds was reported as an I/O failure and `certus_lookup_misses_total`
> read 0; and the transport host dropped both entries held back before dispatch and
> every non-`KeyNotFound` error, so hits + misses did not account for what the client
> asked for. CODE (authoritative): `NotFound` now surfaces as `KeyNotFound` while
> `TransportError` stays `IoError`; every dispatched entry lands in exactly one of
> hits / misses / errors. SPEC BACKFILL: spec 001 FR-011 gains the unsuccessful-fetch
> case it never stated, and `contracts/errors.md` is corrected for both variants.
> Digest recomputed over this branch's tree (58c590e9c829…); the previous stamp
> (`9c0c88f5…`) predated these `src/**` and `specs/**` edits and was stale.

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

---

# Appended sweep — feature 002 Phase 1 (2026-09-28)

Generated: 2026-09-28
Component: `components/dispatcher` (+ `lib/shmq-dispatcher`, `apps/certus-server-yaml`)
Trigger: feature 002 Phase 1 — lookup classification fix, remote-lookup counters, FR-024 accounting
Branch: `certus-lookup-observability`

## THE METHOD USED HERE IS NOT THE ONE THAT WORKED ON THE WORKLOAD GENERATOR

The generator's sweep counted each requirement id across `crates/**/*.rs` and treated an
uncited requirement as a signal, because that code cites requirements densely. **That
method is invalid in this component and would have produced a garbage report.** Measured:

| | dispatcher | workload-generator |
|---|---|---|
| requirements defined | 120 (78 + 42) | 99 |
| distinct ids cited in code | **4** | 75+ |

Four citations against 120 requirements. Reporting "116 not implemented" would have been
the output of the method, and it would have been false. This sweep therefore checks
**meaning**, scoped to what Phase 1 could have falsified, and says so rather than
implying whole-spec coverage.

## Summary

| Category | Count |
|----------|-------|
| Specs analysed | 2 |
| Requirements checked (scoped) | 11 |
| ✓ Aligned after fixes | 8 |
| ⚠️ Drifted — **FIXED in this pass** | 3 |
| ✗ Not implemented | 0 in scope |
| 🆕 Unspecced surface | 1 (pre-existing, structural — see below) |

## Findings

### 1. ⚠️ MAJOR — FR-058 enumerated six counters; the struct has eight

`spec.md` FR-058 (001): "`TierEventStats` is a `Copy` struct of **six** `u64` fields",
then names them. `components/interfaces/src/idispatcher.rs` now has **eight** —
`remote_lookup_hits` and `remote_lookup_misses` were added by `9416c997`.

**This is drift I introduced and then missed.** Phase 1 updated FR-011 and
`contracts/errors.md` for the classification change, but not the one requirement that
*enumerates these very counters*. The precedent was already in the file and I did not
follow it: the 2026-09-09 sync updated FR-058 from four fields to six when
store-backpressure added two.

**FIXED**: FR-058 now says eight and documents both, including the accounting rule that a
`TransportError` is neither hit nor miss, and that zeroes mean "no remote traffic" rather
than "remote lookup unwired".

**SC-017 also fixed**, because it covered FR-058 and would otherwise silently
under-verify it: a populate/lookup/evict cycle does not exercise the remote counters at
all, so they needed their own criterion (must not move on a purely local hit; hits +
misses equals keys forwarded less transport failures).

### 2. ⚠️ MODERATE — `contracts/idispatcher.md` claimed the interface change is what makes FR-024 enforceable

`contracts/idispatcher.md:51` said `served_by` being meaningful on failure paths "is what
makes 'every request lands in exactly one bucket' enforceable."

**Phase 1 disproved this.** FR-024's aggregate identity is now enforced server-side by a
pure tally in `shmq-dispatcher`, with **no interface change at all**. The claim tied a
requirement to a design that turned out not to be necessary for it.

**FIXED**: the contract now claims what `LookupOutcome` actually adds — per-key
attribution, which a tally cannot recover — and explicitly warns against re-arguing the
identity as a justification for the change. The argument for Phase 2 has to be
attribution, or it is not an argument.

### 3. ⚠️ MODERATE — the same contract's "Migration: compiler-enforced" was unqualified

True of the return-type widening it was written about; **false as a blanket property**.
Adding two fields to `TierEventStats` compiled with zero errors because the other three
implementors use `..Default::default()`, so a future implementor can silently
under-report.

**FIXED**: scoped to the return-type change, with the struct-field counter-example
recorded as measured rather than predicted.

### ✓ Aligned (verified, not assumed)

- **FR-011 + `contracts/errors.md`** — `RemoteLookupError::NotFound` → `KeyNotFound`,
  `TransportError` → `IoError`. Matches `dispatcher/src/lib.rs` exactly; both tables
  updated in the same commit as the behaviour change.
- **FR-024** — hits + misses + errors == entries requested, enforced by
  `Translator::tally_lookup` and tested as the identity, mutation-verified.
- **FR-026** — attribution does not depend on draining the eviction stream; the counters
  are polled via `tier_event_stats()`, which does not touch that stream.
- **FR-030/031** — per-component documentation obligations met: 001 for the behaviour
  change, definition-site docs in `shmq-dispatcher` (which owns no `specs/`).
- **FR-025, FR-027, FR-028, FR-029** — correctly **not** claimed by Phase 1; recorded as
  awaiting later phases in 002's new coverage table rather than left to inference.

### 🆕 Unspecced surface — the Prometheus endpoint has no owning spec (PRE-EXISTING)

`serve_metrics` renders 16 `certus_*` metrics. **No requirement in any spec names any of
them**, including the three Phase 1 added (`certus_lookup_errors_total`,
`certus_remote_lookup_hits_total`, `certus_remote_lookup_misses_total`).

**Not attributed to this change.** `apps/certus-server-yaml` has no `specs/` and no
`.specify/` at all, so it cannot own a feature, and the same gap already covers
`certus_populates_total`, the memory-tier and the NVMe metrics. Phase 1 made an existing
structural gap two metrics wider; it did not create it.

Worth a decision, not a silent backfill: **metric names are an external contract** — a
scraper or dashboard breaks if they change — and right now nothing in the repo pins them.
Options are to give the server app a spec, or to name the Prometheus surface in the
dispatcher spec that produces the counters. Both are choices for the user.

## Inter-spec conflicts

None found between 001 and 002 in the checked scope. The known **seven-value taxonomy
versus five-value wire projection** tension is internal to 002, deliberately recorded
there, and unresolved pending sign-off — a declared open decision, not drift.

## Recommendations

1. **Commit findings 1–3** (done in this pass) — all three are documentation catching up
   to shipped code, no behaviour change.
2. **Decide who owns the `/metrics` name surface** before Phase 3 widens it further.
3. **Do not run the citation-count method on this component again** without stating that
   it does not apply; record the 4-of-120 measurement so the next sweep starts from it.
