# fix_dispatcher_rw-stats-decrease-after-shutdown

> **In short** - Problem: `shutdown()` (and re-`initialize()`) dropped the data drives, so `read_write_stats()` returned zeros afterwards — the cumulative statistics decreased between two successive calls. Fix: fold the outgoing drives' counters into a persistent `rw_stats_baseline` before teardown and aggregate baseline + live drives; complete the Creusot model update (the eviction-path frame predicates also had to preserve the new baseline field).


```
component:      dispatcher
finding:        rw-stats-decrease-after-shutdown
property:       DIS-READ-WRITE-STATS-MONOTONIC
found by:       see "How it was found"
base commit:    850a00403ce3        branch: repair/j33-rc21-20261030
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
The read/write statistics never decrease between two successive calls, and they are all zero when the dispatcher is built without the telemetry features.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:383`
```
- **FR-056** *(Sync 2026-08-07 — backfilled; these methods ship on `IDispatcher` but had no requirement)*: The dispatcher MUST additionally expose the following `IDispatcher` methods:
  - **GPU-staged memory-lifecycle primitives** — an explicit reserve→copy→complete flow that lets a caller populate a cache entry from GPU memory across one or more regions without the single-shot `populate` call:
    - `reserve_memory(key, size, session_id, deadline) -> *mut u8` reserves a memory-tier slot of `size` bytes for `key`, tags it with `session_id`, and returns the host pointer for staging. The reservation is made via `evict_and_insert` under **bounded store backpressure**: on a momentarily-full tier (`AllocationFailed`) it retries the allocation on a 20 ms poll until a deadline elapses — absorbing transient saturation while the background evictor demotes and in-flight write-throughs land — then surfaces `AllocationFailed` only if the tier is still full. The `deadline` parameter sets that bound: `None` uses the dispatcher's own `store_backpressure_ms` budget (FR-033, default 5 s) measured from the start of *this* call — the legacy per-call behavior used by the dispatcher's own `populate`/`batch_populate` paths; `Some(instant)` retries only until `instant`, a deadline the caller shares across a whole reserve batch so the batch's *total* backpressure is bounded by one budget rather than paying it per key (`K ×` the budget). `store_backpressure_ms = 0`, or a `Some` instant already in the past, restores fail-fast: one best-effort attempt, no retry. The threads it waits on (evictor, background writer, other workers' loads) are independent of the store worker, so the wait is deadlock-free. See FR-060. *(Sync 2026-09-09 — code authoritative, commit `39ffee9b`; Sync 2026-09-15 — commit `bec6c6ec` added the `deadline` parameter so the shm-queue OP_RESERVE handler shares one budget across the batch instead of paying it per key and overrunning the client's ring request deadline.)* *Diagnostics:* `CERTUS_LOG_RESERVE` (truthy) logs each `reserve_memory` call with its key, size and `session_id` at `info`. It is off by default because it is noisy under load. *(Sync 2026-10-14 — backfilled note, code authoritative, `src/lib.rs:3589-3604` as of 2026-10-23.)*
```

### The code: before (-) and after (+)
```diff
@@ -429,4 +429,10 @@ define_component! {
             bg_mt_evictor: Mutex<Option<MemoryTierEvictor>>,
             data_drives: RwLock<Vec<DataDrive>>,
+            // Cumulative read/write counters of data drives already torn down
+            // (shutdown) or replaced (re-initialize). `read_write_stats`
+            // starts from this baseline and merges the live drives on top, so
+            // the dispatcher-wide total never decreases over the lifetime of
+            // the component (FR-056 monotonicity). See absorb_drive_rw_stats.
+            rw_stats_baseline: RwLock<interfaces::ReadWriteStats>,
             pipeline_ring: RwLock<Option<pipeline::PipelineRing>>,
             warm_stream: AtomicU64,
@@ -1788,4 +1794,19 @@ impl DispatcherComponent {
     }
 
+    /// Fold the live data drives' read/write counters into
+    /// `rw_stats_baseline`.
+    ///
+    /// Called before the drive list is cleared (`shutdown`) or replaced
+    /// (`initialize`): the baseline is what `read_write_stats` aggregates
+    /// from, so the counters of drives that are about to be dropped stay
+    /// part of the cumulative total and the aggregate never decreases.
+    fn absorb_drive_rw_stats(&self) {
+        let drives = self.data_drives.read();
+        let mut baseline = self.rw_stats_baseline.write();
+        for drive in drives.iter() {
+            baseline.merge_from(&drive.block_dev_iface.read_write_stats());
+        }
+    }
+
     fn create_data_drives(
         &self,
@@ -2115,4 +2136,7 @@ impl IDispatcher for DispatcherComponent {
         if self.spdk_env.is_connected() || has_bd_factory {
             let drives = self.create_data_drives(&config)?;
+            // Replacing the drive list must not lower the cumulative counters:
+            // fold the outgoing drives' totals into the baseline first.
+            self.absorb_drive_rw_stats();
             *self.data_drives.write() = drives;
 
@@ -2370,4 +2394,7 @@ impl IDispatcher for DispatcherComponent {
         // then join threads. This prevents crashes from SPDK transport teardown
         // invalidating memory that other actors are still actively polling.
+        // The drives are about to be dropped; keep their read/write counters
+        // in the baseline so read_write_stats stays monotonic after shutdown.
+        self.absorb_drive_rw_stats();
         let drives = {
             let mut g = self.data_drives.write();
@@ -4446,5 +4473,8 @@ impl IDispatcher for DispatcherComponent {
         // block device tracks its own SSD read/write bytes+ops (zeroed unless
         // built with the telemetry feature); sum for the dispatcher-wide total.
-        let mut agg = interfaces::ReadWriteStats::default();
+        // The baseline carries the counters of drives already torn down, so the
+        // cumulative total keeps holding (and never drops below) what the
+        // removed drives had accumulated.
+        let mut agg = self.rw_stats_baseline.read().clone();
         for drive in self.data_drives.read().iter() {
             // merge_from sums every counter and both size histograms.
@@ -5552,4 +5582,5 @@ mod tests {
             Mutex::new(None),
             RwLock::new(Vec::new()),
... (154 more lines in the commit)
```

### The test that shows the bug: `components/dispatcher/src/repair_test_eviction_window_tinylfu.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `eviction-window-poisoned-by-admission-front`.
//!
//! `eviction-policy-optimized` admits a store whose sketch count is no higher
//! than the current victim's at the LRU **front**. Under a store burst that is
//! every fresh key, so the front of the eviction order fills with entries whose
//! write-through has not landed and which `try_evict_to_block` must refuse.
//! Persisted, demotable entries sit just behind them.
//!
//! `evict_for_space` scans `MAX_SCAN × attempts` candidates, but every scan that
//! frees nothing returns `AllocationFailed` at once, so the widening never
//! happens: the scan stays 4 deep, sees only the unpersisted front, and the
//! store is refused although demotable entries exist. The fix takes one
//! last-chance full-depth scan before giving up, as `dispatcher-p2p` does.
//!
//! Uses the real `MemoryTierComponent` and `EvictionPolicyOptimizedComponent`,
//! so the eviction order is the one the `full-optimized` profile produces.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};

use parking_lot::RwLock;

use component_core::query_interface;
use eviction_policy_optimized::EvictionPolicyOptimizedComponent;
use interfaces::{
    CacheKey, DispatchMapError, IDispatchMap, IEvictionPolicy, IMemoryTier, LookupResult,
};
use memory_tier::MemoryTierComponent;

use crate::{DispatcherComponent, DispatcherError, TierEventCounters};

const SLOT: u32 = 4096;
/// Persisted entries, stored first. They age to the back of the eviction order.
const PERSISTED: u64 = 8;
/// Unpersisted entries, stored next. TinyLFU admits each at the LRU front.
const UNPERSISTED: u64 = 8;

// --- dispatch map: just enough state for `try_evict_to_block` ---

... (242 more lines)
```


Traces: FR-056, idispatcher.rs read_write_stats

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem — rw-stats-decrease-after-shutdown (DIS-READ-WRITE-STATS-MONOTONIC)

Plain sentence: after the dispatcher is shut down, a caller who asks again for the
SSD read/write statistics gets all zeros back, even though the drives had done I/O
before — the numbers a monitoring dashboard plots suddenly drop to zero instead of
staying flat.

Spec location: `components/interfaces/src/idispatcher.rs:935-940` —
`IDispatcher::read_write_stats` must return *cumulative* counters that are
"Monotonic; take deltas across two calls to measure a window" (FR-056,
DIS-READ-WRITE-STATS-MONOTONIC). The same wording is in the spec backup
`.specify/sync/backups/20260807T160256Z/idispatcher.rs:662-667`.

Code location that disagrees: `components/dispatcher/src/lib.rs:2372-2375` —
`shutdown()` does `std::mem::take(&mut *g)` on `data_drives`, dropping every data
drive (their per-drive counters go with them), while
`components/dispatcher/src/lib.rs:4444-4454` — `read_write_stats()` aggregates
only over the drives *currently in* `data_drives`. After shutdown that list is
empty, so the aggregate is `ReadWriteStats::default()` (all zeros), which is
strictly smaller than any pre-shutdown value. The same drop also happens on
re-`initialize()` with live drives (`lib.rs:2117` replaces the drive list without
folding the old counters anywhere).

Verdict: **code wrong**. The spec is the intended behaviour (cumulative,
monotonic counters for the life of the component); the implementation loses the
counters when it tears the drives down. Fix: keep a dispatcher-wide baseline of
the aggregated counters that survives drive teardown; `read_write_stats` reports
baseline + live drives, `shutdown` (and `initialize`'s drive replacement) folds
the outgoing drives' counters into the baseline first.

## Addendum (attempt 2): why attempt 1 was rejected

Attempt 1 fixed the code and the model of `read_write_stats`/`shutdown`/`initialize`,
but the acceptance check's HARMLESS leg (whole-crate Creusot re-proof) found four
modules that proved at the base commit and no longer proved:
`promote_and_serve` (19 unproved goals), `promote_one` (5), `bl_cold_nodrive` (4),
`bl_cold_prep` (4).

Cause: the model gained a `rw_base` field on the ghost world `W`, and `same_env`
(core.rs) was extended to preserve it — but the two *eviction-path* frame
predicates that enumerate preserved `W` fields, `evict_frame` (core.rs:495) and
`eai_frame` (evict.rs:177), were not. Every function that goes through an eviction
(`promote_one`, `promote_and_serve`, `bl_cold_prep`, `bl_cold_nodrive`) must
re-establish `same_env` from those frames, and the missing `rw_base` link left
that conjunct unproved. Verdict is unchanged: **code wrong** (attempt 1's src fix
stands); this attempt completes the model update.

# Repair report: rw-stats-decrease-after-shutdown (DIS-READ-WRITE-STATS-MONOTONIC)

## In short

Problem: `shutdown()` (and re-`initialize()`) dropped the data drives, so `read_write_stats()` returned
zeros afterwards — the cumulative statistics decreased between two successive calls.
Fix: fold the outgoing drives' counters into a persistent `rw_stats_baseline` before teardown and
aggregate baseline + live drives; complete the Creusot model update (the eviction-path frame
predicates also had to preserve the new baseline field).

## Bug

`IDispatcher::read_write_stats` (spec: `components/interfaces/src/idispatcher.rs:935-940`, FR-056)
promises cumulative counters that are "Monotonic; take deltas across two calls to measure a window",
but the implementation aggregated only over the drives currently in `data_drives`: `shutdown()`
(`components/dispatcher/src/lib.rs`, `std::mem::take` of the drive list) and `initialize()` (drive-list
replacement) dropped the drives together with their counters, so the aggregate fell back to all zeros.

## Impact

- Monitoring consumers that sample `read_write_stats()` periodically and take deltas (as the spec
  text itself instructs) saw negative deltas after a shutdown, corrupting throughput/latency
  accounting and any alerting built on it.
- The final sample taken after shutdown read as "no I/O ever happened", so the lifetime totals of a
  dispatcher instance were unrecoverable from the API at the exact moment an operator is most likely
  to scrape final stats.
- Re-initialize (lifecycle restarts, tests) erased history mid-flight the same way.
- No data-plane corruption: only reported statistics were affected, never stored data.

## How to reproduce

1. `git checkout 850a00403` (the unfixed base).
2. Copy `components/dispatcher/src/repair_test_rw_stats_after_shutdown.rs` into that tree (it is
   wired into `lib.rs` by `#[cfg(test)] #[path = "repair_test_rw_stats_after_shutdown.rs"] mod ...;`).
3. `cargo test -p dispatcher repair_test_rw_stats_after_shutdown` — the test installs a mock data
   drive whose `IBlockDevice::read_write_stats` returns fixed non-zero counters, samples
   `read_write_stats()` twice, calls `shutdown()`, samples twice more, and asserts every counter and
   histogram bucket never decreases; on the unfixed code the post-shutdown aggregate is all zeros and
   the assertion panics. After the fix it passes (and the full suite passes).

## Fix

**Real code (`components/dispatcher/src/lib.rs`)** — root cause: the aggregate only ever looked at
*live* drives while the drive set is destructively replaced at two points in the lifecycle.

- `DispatcherComponent` gained `rw_stats_baseline: RwLock<interfaces::ReadWriteStats>` (persistent,
  starts at zero).
- New private helper `absorb_drive_rw_stats()` sums every live drive's `read_write_stats()` into the
  baseline; `shutdown()` calls it immediately before taking/dropping the drive list and
  `initialize()` calls it before installing a newly created drive list, so a replaced drive
  generation's totals are preserved.
- `read_write_stats()` now starts the aggregate at the baseline (cloned under the read lock) and adds
  the live drives on top: the reported value is baseline + live — monotonic across shutdown and
  re-initialize, and still exactly the sum of every data drive that ever existed (which also keeps
  the DIS-READ-WRITE-STATS-SUM reading true).
- The 26 `DispatcherComponent::new(...)` construction sites pass
  `RwLock::new(interfaces::ReadWriteStats::default())` for the new field.

**Proof model (`components/dispatcher/verif-creusot/`)** — the model of every function whose body
changed was updated, plus the new driver:

- Ghost world `W` gained `rw_base` with `rw_total(w, n) = w.rw_base + rwsum(w.rw, n)` (opaque, with
  trusted `rw_total_def`/`rw_total_zero`/`rw_total_congr` lemmas) and a trusted boundary reader
  `bd_rw_base` (`src/core.rs`, `src/boundary.rs`); `same_env` preserves `rw_base`.
- `read_write_stats` modelled over `rw_total`; new `absorb_drive_rw_stats` modelled with a loop;
  `shutdown` and `initialize` postconditions extended with the baseline fold (`src/misc.rs`,
  `src/methods.rs`, `src/init.rs`).
- Drivers: `verify_dis_read_write_stats_monotonic` (two calls, `shutdown`, two more calls;
  `result.1 >= result.0`, and all-zero for a telemetry-free fresh state) and its
  `verify_dis_read_write_stats_monotonic__mutant` twin (strictly greater), which fails as required;
  `verify_dis_read_write_stats_sum` re-stated over `rw_total` (`src/drivers.rs`,
  `tools/gen_drivers.py` kept in sync).

**Attempt 2 (this revision): completing the model update.** The first repair was rejected by the
acceptance check's HARMLESS leg: four modules that prove at the base commit — `promote_and_serve`
(19 unproved goals), `promote_one` (5), `bl_cold_nodrive` (4), `bl_cold_prep` (4) — no longer proved.
Adding `rw_base` to `W` and to `same_env` was not enough: the two *eviction-path* frame predicates
that enumerate the preserved `W` fields, `evict_frame` (`src/core.rs`) and `eai_frame`
(`src/evict.rs`), still did not preserve `rw_base`. Every function that reaches an eviction
(`promote_one`, `promote_and_serve`, `bl_cold_prep`, `bl_cold_nodrive`) re-establishes `same_env`
from those frames, and the missing `rw_base` link left that conjunct unproved. The fix adds
`b.rw_base == a.rw_base` to both `evict_frame` and `eai_frame` (one line each) — the eviction paths
are built from trusted boundary operations with record-update postconditions (`W { ..*s.g }`), which
preserve the new field automatically. After the change all four modules prove again (fresh forced
re-proof), and a full sweep of the crate's 393 proof modules (2 provers, per-module, the repo's
`tools/run_batch.sh` pattern) shows every module that proves at base still proves: 235/235 non-mutant
modules PROVED, the only failures being the six `refute_*` harnesses that already fail at base and
the mutant twins, which must fail.

### Rejected alternatives

- *Keep the drive stubs alive after shutdown:* changes the shutdown lifecycle and resource
  reclamation semantics for a statistics-only concern; the block-device actors are genuinely gone.
- *Fix only `shutdown()`:* re-`initialize()` reproduces the same decrease; the property speaks of any
  two successive calls.
- *Drop `rw_base` from `same_env` instead of extending the eviction frames:* `same_env` is what
  carries the baseline through `drain`/`checkpoint_all` in the `shutdown` proof; weakening it would
  break the monotonicity proof itself.
- *Weakening `initialize()` to not replace drives:* `initialize()` is specified to build a fresh
  drive set from config.

## Scope beyond the finding

- Re-initialize path: the same decrease existed across `initialize()` on an already-initialized
  component; fixed by the same fold.
- `DIS-READ-WRITE-STATS-SUM` was re-verified against the new aggregate (`rw_total`) so the two
  readings of the same method stay consistent.
- Post-shutdown `read_write_stats()` is now also stable (two calls return equal values); the repair
  test additionally asserts this.
- The `evict_frame`/`eai_frame` completion is required by THIS property's model change (the baseline
  field must be preserved by every path the drivers reason about), so it is in scope, but it also
  future-proofs the frames for any later `W` field.

## Risk

- Low. The real-code change adds one `RwLock<ReadWriteStats>` field and a read-only aggregation pass;
  it touches no data path and introduces no nested locking (the baseline lock is taken without
  holding other locks; `read_write_stats` clones under the lock and releases before iterating
  drives).
- Counter semantics are unchanged (field-wise adds via `merge_from`, as before); the model keeps
  ghost `Int` counters like every other counter in the model.
- The model change (two frame predicates preserve one more field) strengthens hypotheses available
  to every consumer of the eviction frames; the whole crate was re-verified module-by-module after
  it (all base-proved modules still prove, no test or proof regressions; dispatcher tests 162 passed
  in each of 5 runs).

## Follow-ups

- Consider exposing the live/retired split of the counters in a future interface revision if
  consumers want per-generation stats; not needed for this property.
- The verif model keeps `rw_base` as a scalar total (one representative counter field); per-field
  modelling could be added if a future property needs per-field monotonicity.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-READ-WRITE-STATS-MONOTONIC`, callee modules `read_write_stats,shutdown`.

```
ACCEPT-REPO: base 850a00403ce3a8ae4815c3abcb59f5d7f264e99f; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (162 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-READ-WRITE-STATS-MONOTONIC yes
  D. MODEL   : dispatcher yes
  E. (before side for dispatcher reused from cache: same base, proof crate and gate)
  E. REVERIFY: dispatcher yes (112 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 162 passed in each run
  dispatcher                   | DIS-READ-WRITE-STATS-MONOTONIC           | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 112 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 300 min | 162 min | rejected |
| 2 | 128 min | 121 min | accepted |

Total wall-clock: 712 min.
