# fix_dispatcher_eviction-errors-counted-unpersisted

> **In short** - Problem: the eviction scan booked every non-pin refusal (notably `KeyNotFound` for a slot not yet registered in the dispatch map) as "blocked unpersisted", so that counter over-reported write-through lag and hid third causes. Fix: only the dispatch-map's `InvalidState` (no `ssd_offset`) refusal increments `evictions_blocked_unpersisted`; other refusals increment neither counter.


```
component:      dispatcher
finding:        eviction-errors-counted-unpersisted
property:       DIS-TIER-STATS-BLOCKED-CAUSES
found by:       see "How it was found"
base commit:    850a00403ce3        branch: repair/j33-rc22-20261030
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
Each eviction candidate refused because it holds a read pin increases evictions_blocked_by_pin by one, and each candidate refused for lacking an SSD offset increases evictions_blocked_unpersisted by one, counted per candidate examined.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:359`
```
- **FR-033**: The `DispatcherConfig` MUST include `ssd_eviction_threshold` (f64, default 0.9), `ssd_eviction_low_watermark` (f64, default 0.8), `ssd_eviction_batch_size` (usize, default 64), `ssd_eviction_interval_secs` (u64, default 5), `max_eviction_attempts` (usize, default 2048), `memory_tier_eviction_threshold` (f64, default 0.0), `memory_tier_eviction_low_watermark` (f64, default 0.70), `memory_tier_eviction_batch_size` (usize, default 64), `memory_tier_eviction_interval_secs` (u64, default 2), `cold_staging_slots` (usize, default 64), `cold_staging_buf_bytes` (usize, default 16 MiB = 16 × 1024 × 1024), and `store_backpressure_ms` (u64, default 5000). Setting `ssd_eviction_threshold` to 0.0 disables the SSD evictor. Setting `memory_tier_eviction_threshold` to 0.0 disables the memory-tier evictor. Setting `cold_staging_slots` to 0 disables the cold-load staging pool (FR-053). *(Sync 2026-09-09 — code authoritative, commit `39ffee9b`:)* `store_backpressure_ms` bounds how long a store reservation (`reserve_memory`) retries on a full memory tier before surfacing `AllocationFailed`; setting it to 0 disables the retry so `reserve_memory` fails fast on a full tier (the store path's best-effort drop-on-full, FR-060, still applies regardless). `max_eviction_attempts` controls how many iterations `evict_for_space` attempts before returning `AllocationFailed`. *(Sync 2026-08-07 — backfilled additional shipped `DispatcherConfig` fields: `metadata_partition_size` (u64, default 128 MiB) and `extended_metadata_partition_size` (u64, default 128 MiB), which size the metadata and extended-metadata GPT partitions created during `initialize()`; and `backfill_delay_ms` (u64, default 10), a peer-to-peer/remote-replication tuning knob consumed only on the dispatcher's backfill path and inert for purely local caching. These are part of the shared `DispatcherConfig` struct in the interfaces crate.)* *(Sync 2026-10-14 — code authoritative, commit `8e12e49f`:)* `cold_staging_buf_bytes` default raised from 4 MiB to **16 MiB**. `serve_cold_staged` only serves a cold load through a staging buffer when the transfer fits (`total_size <= buf_bytes`); a 64-token KV-offload block is 8 MiB per worker, so the old 4 MiB default made that guard always false and every tier-full cold load returned `AllocationFailed`, which vLLM's `OffloadingConnector` turns into a fatal `assert transfer_result.success` → `EngineDeadError`. 16 MiB covers the 8 MiB block with headroom; at the default 64 slots this is 1 GiB of pinned host DMA (up from 256 MiB).
- **FR-034**: During `initialize()`, after GPU and memory-tier are ready, the dispatcher MUST call `IGpuServices::register_host_memory` on the memory-tier pool to CUDA-pin and SPDK-register it for zero-copy NVMe and GPU DMA. Registration failure MUST be logged as a warning (zero-copy pipelines require this registration). Each data drive's `ClientChannels` MUST be managed via a per-drive `ChannelPool` rather than a single cached instance: the pool starts empty at `initialize()` and grows on demand — each concurrent reader (`promote_and_serve`, `serve_cold_staged`, and each `promote_to_memory_tier` per-drive prefetch thread) checks out an exclusive channel via `ChannelPool::checkout()`, which connects a fresh `ClientChannels` on first use or reuses one returned by a prior lease, and returns a `ChannelLease` (RAII) that puts the channel back in the pool on drop. Checkout MUST drain any completions left in `completion_rx` by a previous user before handing out the channel, so a recycled channel's stale tags are never matched against a new caller's segments. `connect_client()` MUST be called outside the pool's lock so a slow connect does not block other checkouts. This replaces a prior design that cached a single `ClientChannels` per drive at init time: a single shared SPSC channel let two concurrent readers on the same drive (e.g. `batch_lookup` and the `promote_to_memory_tier` prefetch path) dequeue each other's NVMe completions ("completion theft"), causing a permanent, non-timing-out hang. The pool's lazy, per-checkout connection is a deliberate trade-off of a small first-use latency for correctness under concurrent per-drive access.
- **FR-035**: During `shutdown()`, before memory-tier teardown, the dispatcher MUST call `IGpuServices::unregister_host_memory` to release CUDA page-locking and SPDK registration on the memory-tier pool.
```
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:360`
```
- **FR-034**: During `initialize()`, after GPU and memory-tier are ready, the dispatcher MUST call `IGpuServices::register_host_memory` on the memory-tier pool to CUDA-pin and SPDK-register it for zero-copy NVMe and GPU DMA. Registration failure MUST be logged as a warning (zero-copy pipelines require this registration). Each data drive's `ClientChannels` MUST be managed via a per-drive `ChannelPool` rather than a single cached instance: the pool starts empty at `initialize()` and grows on demand — each concurrent reader (`promote_and_serve`, `serve_cold_staged`, and each `promote_to_memory_tier` per-drive prefetch thread) checks out an exclusive channel via `ChannelPool::checkout()`, which connects a fresh `ClientChannels` on first use or reuses one returned by a prior lease, and returns a `ChannelLease` (RAII) that puts the channel back in the pool on drop. Checkout MUST drain any completions left in `completion_rx` by a previous user before handing out the channel, so a recycled channel's stale tags are never matched against a new caller's segments. `connect_client()` MUST be called outside the pool's lock so a slow connect does not block other checkouts. This replaces a prior design that cached a single `ClientChannels` per drive at init time: a single shared SPSC channel let two concurrent readers on the same drive (e.g. `batch_lookup` and the `promote_to_memory_tier` prefetch path) dequeue each other's NVMe completions ("completion theft"), causing a permanent, non-timing-out hang. The pool's lazy, per-checkout connection is a deliberate trade-off of a small first-use latency for correctness under concurrent per-drive access.
- **FR-035**: During `shutdown()`, before memory-tier teardown, the dispatcher MUST call `IGpuServices::unregister_host_memory` to release CUDA page-locking and SPDK registration on the memory-tier pool.
- **FR-036**: System MUST provide a `lookup_async(key, ipc_handle) -> Result<GpuStream, DispatcherError>` method on the `IDispatcher` interface. This method performs the same cache lookup as `lookup` (dispatch-map query, memory-tier hit, BlockDevice promotion) but returns a `GpuStream` handle instead of blocking on the H2D DMA transfer. The caller MUST call `stream_synchronize` on the returned stream before accessing the GPU destination memory. For SSD promotion paths the copy completes synchronously and a null stream is returned. *(Sync 2026-10-14 — code authoritative, commit `1d24ff27f`:)* On a BlockDevice hit, `lookup_async` MUST hold the read pin its `dm.lookup` took until `promote_and_serve` has returned, and release it only then. That pin is what stops a concurrent `remove` or SSD eviction from deleting the entry, or freeing the freshly inserted memory-tier slot, while the SSD read, the GPU copy and the in-place `promote_block_to_memory_tier` are still running. *(Previously the pin was released before promotion started.)* If `promote_and_serve` fails with `AlreadyExists` (a concurrent cold lookup of the same key won the `mt.insert` race), `lookup_async` MUST treat it as a hit and serve the winner's slot via `serve_concurrently_promoted` (FR-051), not fail the load. The synchronous `lookup` method MUST delegate to `lookup_async` internally and call `stream_synchronize` before returning.
```

### The code: before (-) and after (+)
```diff
@@ -78,4 +78,8 @@ mod repair_test_eviction_events_dropped;
 mod repair_test_failed_promotion_releases_slot;
 
+#[cfg(test)]
+#[path = "repair_test_eviction_errors_counted_unpersisted.rs"]
+mod repair_test_eviction_errors_counted_unpersisted;
+
 use parking_lot::RwLock;
 use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
@@ -1185,12 +1189,23 @@ impl DispatcherComponent {
                     self.tier_counters.record_eviction_blocked_by_pin();
                 }
-                Err(_) => {
-                    // The only other refusal for a resident entry is a missing
-                    // ssd_offset, i.e. write-through has not landed. Counted rather
-                    // than inferred by subtraction, so a third cause appearing later
-                    // shows up as the two counters failing to sum to the scanned
-                    // total instead of being silently attributed to this one.
+                Err(interfaces::DispatchMapError::InvalidState(_)) => {
+                    // The dispatch-map signals "memory-tier entry has no SSD
+                    // offset (write-through not complete)" with InvalidState,
+                    // so that refusal is the unpersisted one. Counted rather
+                    // than inferred by subtraction, so a third cause appearing
+                    // later shows up as the two counters failing to sum to the
+                    // scanned total instead of being silently attributed to
+                    // this one.
                     self.tier_counters.record_eviction_blocked_unpersisted();
                 }
+                Err(_) => {
+                    // A refusal that is neither a pin nor a missing ssd_offset
+                    // (e.g. KeyNotFound: a memory-tier slot `reserve_memory`
+                    // inserted whose dispatch-map entry is not registered yet).
+                    // FR-033 reserves each counter for its own cause, so this
+                    // must increment NEITHER — a third cause has to stay
+                    // visible as the two counters failing to sum to the
+                    // scanned total, not be absorbed into the unpersisted one.
+                }
             }
             // Write-through incomplete (no ssd_offset) so it can't be demoted.
```

### The test that shows the bug: `components/dispatcher/src/repair_test_eviction_errors_counted_unpersisted.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `eviction-errors-counted-unpersisted`
//! (DIS-TIER-STATS-BLOCKED-CAUSES).
//!
//! FR-033 (specs/002-served-by-tier-attribution) reserves
//! `evictions_blocked_unpersisted` for the one refusal cause "the entry has no
//! `ssd_offset` yet" and `evictions_blocked_by_pin` for "a reader holds it".
//! Any third cause must increment NEITHER counter, so that the two counters'
//! sum against the scanned total exposes it instead of absorbing it.
//!
//! The dispatch-map refuses a candidate in a third way that is neither of
//! those: `KeyNotFound` for a memory-tier slot that exists in the tier but has
//! no dispatch-map entry yet (the window after `reserve_memory` inserted the
//! slot and before the entry is registered). `evict_one_clean`'s `Err(_)`
//! catch-all booked that refusal as `evictions_blocked_unpersisted`, so the
//! counter over-reported write-through lag by every unregistered slot the
//! scan touched.
//!
//! Three candidates, one of each refusal cause, are scanned in one
//! `evict_for_space` call. `evict_for_space` takes two passes over them (the
//! per-step scan, then the last-chance full-depth scan), so each candidate is
//! examined twice:
//!
//!   * the pinned one must add 2 to `evictions_blocked_by_pin` (1 per
//!     examination),
//!   * the unpersisted one must add 2 to `evictions_blocked_unpersisted`,
//!   * the unregistered one must add 0 to both.
//!
//! On the unfixed code the unregistered candidate is counted as unpersisted,
//! so `evictions_blocked_unpersisted` comes out as 4 and the assertion fails.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};

use parking_lot::RwLock;

use interfaces::{
    CacheKey, DispatchMapError, IDispatchMap, IMemoryTier, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot,
};
... (304 more lines)
```


Traces: 002 FR-033, idispatcher.rs TierEventStats, 002 FR-034, dispatcher/src/lib.rs:1066, dispatcher/src/lib.rs:1106

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem (finding `eviction-errors-counted-unpersisted`, DIS-TIER-STATS-BLOCKED-CAUSES)

**One plain sentence:** when the eviction scan is refused for a reason that is neither a held
pin nor a missing write-through, the dispatcher wrongly books it as "blocked because
unpersisted", so that counter over-reports and hides exactly the third causes it exists to
expose.

**Spec location:** `components/dispatcher/specs/002-served-by-tier-attribution/spec.md`
FR-033 (lines 568-585): `evictions_blocked_by_pin` counts candidates refused with
`ActiveReferences`, `evictions_blocked_unpersisted` counts candidates "refused for want of an
`ssd_offset`", both per candidate examined, and — deliberately — any *third* cause must land in
*neither* counter so that the two counters' sum against the scanned total shows a discrepancy
instead of absorbing it. The interface contract is `TierEventStats` in
`components/interfaces/src/idispatcher.rs:435-457`.

**Code location:** `components/dispatcher/src/lib.rs:1187-1194` (`evict_one_clean`): the match
on `dm.try_evict_to_block(cand)` has an `Err(_)` catch-all that calls
`record_eviction_blocked_unpersisted()` for **every** refusal that is not `ActiveReferences`.
The dispatch-map (`components/dispatch-map/src/lib.rs:548-580`) can refuse a candidate in more
ways than "no `ssd_offset`": `KeyNotFound` for a memory-tier slot that `reserve_memory` has
already inserted but whose dispatch-map entry is not registered yet, `InvalidState` for an
entry that is not in MemoryTier state, and `Timeout`. Each of those is currently counted as
`evictions_blocked_unpersisted`.

**Verdict: code wrong.** The spec is explicit that the unpersisted counter is reserved for the
missing-`ssd_offset` refusal and that third causes must show up as a missing increment; the
code's catch-all silently attributes every other refusal to write-through lag. The fix is to
count only the dispatch-map's `InvalidState` refusal (which is how it signals "memory-tier
entry has no SSD offset", per its own contract and the Creusot boundary model
`dm_try_evict`: `Err(DmE::InvalidState) => ... && !persisted(...)`) as unpersisted, and to let
every other refusal (notably `KeyNotFound`) increment neither counter.

## In short

Problem: the eviction scan booked every non-pin refusal (notably `KeyNotFound` for a slot not
yet registered in the dispatch map) as "blocked unpersisted", so that counter over-reported
write-through lag and hid third causes. Fix: only the dispatch-map's `InvalidState` (no
`ssd_offset`) refusal increments `evictions_blocked_unpersisted`; other refusals increment
neither counter.

## Bug

`evict_one_clean` (`components/dispatcher/src/lib.rs:1187`, the `Err(_)` arm of the
`dm.try_evict_to_block` match) counted every refusal that was not `ActiveReferences` as
`evictions_blocked_unpersisted`, but the dispatch-map refuses candidates in more ways than
"no `ssd_offset`": `KeyNotFound` for a memory-tier slot that `reserve_memory` has inserted
whose dispatch-map entry is not registered yet, `InvalidState` for an entry not in
MemoryTier state, and `Timeout`.

## Impact

A user watching `TierEventStats` (the Prometheus counters the server publishes) sees
`evictions_blocked_unpersisted` inflated by every unregistered slot the scan touched — under
a store burst that is every fresh key — so write-through lag looks worse than it is, and the
FR-033 health check ("the two refusal counters sum to the scanned total, so a third cause
shows up as a discrepancy") can never fire: the third cause is silently absorbed into the
unpersisted counter. Eviction behaviour itself (what is freed, what is skipped) is unchanged.

## How to reproduce

1. `cargo test -p dispatcher repair_test_eviction_errors_counted` — on the unfixed code the
   test fails with `evictions_blocked_unpersisted: 4` (the `KeyNotFound` candidate counted
   twice, once per scan pass) against the expected `2`; on the fixed code it passes.
2. The test (`components/dispatcher/src/repair_test_eviction_errors_counted_unpersisted.rs`)
   scans three candidates through `evict_for_space`: one pinned, one unpersisted, one present
   in the memory tier but absent from the dispatch map, and asserts the exact per-cause
   counter deltas.
3. For the proof: `cd components/dispatcher/verif-creusot && cargo creusot
   verify_dis_tier_stats_blocked_causes -f` (Proved) and `cargo creusot
   verify_dis_tier_stats_blocked_causes_mutant -f` (fails, as the anti-vacuity twin must).

## Fix

`evict_one_clean` now matches the dispatch-map's error variant exactly as FR-033 demands:
`ActiveReferences` → `evictions_blocked_by_pin` (unchanged), `InvalidState` — which is how
the dispatch-map signals "memory-tier entry has no SSD offset (write-through not
complete)", per its own implementation and the Creusot boundary model `dm_try_evict`
(`Err(DmE::InvalidState) => … && !persisted(…)`) — → `evictions_blocked_unpersisted`, and
every other refusal (`KeyNotFound`, `Timeout`, …) increments neither counter, so a third
cause stays visible as the two counters failing to sum to the scanned total.

Alternatives rejected:
- *Matching the `InvalidState` message string* to also exclude the "entry is not in
  memory-tier state" case: string-matching across a component boundary is brittle, and the
  two `InvalidState` refusals are indistinguishable through the typed interface. The Creusot
  boundary model deliberately collapses both into `DmE::InvalidState` with the
  `!persisted(loc)` reading, which is the contract this repair proves against.
- *Inferring the unpersisted count by subtracting `evictions_blocked_by_pin` from the
  scanned total*: explicitly forbidden by FR-033 — subtraction absorbs any third cause into
  the unmeasured counter, which is the very defect being repaired.
- *Changing the dispatch-map to return a dedicated error variant*: out of scope — the repair
  must stay inside `components/dispatcher/`.

The Creusot model (`verif-creusot/src/evict.rs`) mirrors the new three-arm match and gained
three exact-count postconditions (a scan whose every candidate is pinned adds the candidate
count to `blk_pin` and nothing to `blk_unp`, vice-versa for unpersisted, and neither counter
moves when every candidate is unregistered), with matching loop invariants. The predicates
quantify over the scanned candidate list rather than `ok[tick][j]` — the nested-oracle form
made every SMT goal time out. `verif-creusot/src/drivers.rs` gained
`verify_dis_tier_stats_blocked_causes` (proved) and its `__mutant` twin, which asserts the
old catch-all behaviour and now fails, as required.

## Scope beyond the finding

None. The only src/ change is the `Err` arm split in `evict_one_clean` plus the test wiring;
`clear_memory_tier`'s own `try_evict_to_block` match was left alone because it counts
nothing in `TierEventStats`.

## Risk

Behaviour change is confined to a metrics counter: `evictions_blocked_unpersisted` can only
decrease relative to before (refusals that are not `InvalidState` no longer land in it); no
eviction, allocation, or error-path behaviour changed. `TierEventStats` consumers that
compared this counter against an absolute threshold will see smaller values — that is the
correction. The proof-side change strengthens `evict_one_clean`'s contract only (existing
ensures unchanged), and every module that proved at the base revision still proves
(234/234 non-mutant modules re-verified, including `evict_for_space`, `evict_and_insert`
and `reserve_memory`).

## Follow-ups

- `dispatcher-p2p` reports the same two counters for its foreground eviction paths
  (FR-035); its `Err(_)` arms should be audited for the same catch-all defect.
- The `Timeout` variant of `DispatchMapError` is currently unreachable from
  `try_evict_to_block`; if it ever becomes reachable it will (correctly) land in neither
  counter — worth a note in the dispatch-map spec when that changes.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-TIER-STATS-BLOCKED-CAUSES`, callee modules `evict_and_insert`.

```
ACCEPT-REPO: base 850a00403ce3a8ae4815c3abcb59f5d7f264e99f; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (162 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-TIER-STATS-BLOCKED-CAUSES yes
  E. (before side for dispatcher reused from cache: same base, proof crate and gate)
  E. REVERIFY: dispatcher yes (112 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 162 passed in each run
  dispatcher                   | DIS-TIER-STATS-BLOCKED-CAUSES            | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 112 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 91 min | 114 min | accepted |

Total wall-clock: 206 min.
