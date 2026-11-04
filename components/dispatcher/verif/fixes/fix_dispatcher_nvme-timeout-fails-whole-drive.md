# fix_dispatcher_nvme-timeout-fails-whole-drive

> **In short** - Problem: one timed-out NVMe read in a batch's cold path failed every entry on that drive (pipeline.rs `Timeout` arm), violating per-entry error isolation (US-11 AS-4). Fix: keep submitting and draining; the timed-out entry is exactly the object left incomplete, which is failed alone after the loop while every other entry completes.


```
component:      dispatcher
finding:        nvme-timeout-fails-whole-drive
property:       DIS-BATCH-LOOKUP-ERROR-ISOLATION
found by:       see "How it was found"
base commit:    b7d4e9b1f4f6        branch: repair/j35-rc8-20261103
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
A read or DMA failure on one cold entry of a batch is reported only for that entry; the other entries are processed and reported independently.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:217`
```
4. **Given** a cold entry's NVMe read or GPU DMA fails, **When** the scatter-gather loop encounters the error, **Then** the error is reported for that entry only; other entries in the batch continue to be processed independently. Exception: if the scatter-gather's final pipe-stream sync fails, every entry whose H2D was issued in that call is reported as an error, since none of their copies is known to have landed (FR-061; *Sync 2026-10-31*, test `batch_lookup_multi_region_cold_fails_when_final_sync_fails`).
5. **Given** entries that do not exist in the dispatch map, **When** `batch_lookup` is called, **Then** those entries receive `KeyNotFound` errors while valid entries are served normally.
```

### The code: before (-) and after (+)
```diff
@@ -82,4 +82,8 @@ mod repair_test_failed_promotion_releases_slot;
 mod repair_test_eviction_errors_counted_unpersisted;
 
+#[cfg(test)]
+#[path = "repair_test_nvme_timeout_fails_whole_drive.rs"]
+mod repair_test_nvme_timeout_fails_whole_drive;
+
 use parking_lot::RwLock;
 use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
@@ -1221,4 +1221,8 @@ pub unsafe fn scatter_gather_multi_drive_zero_copy(
         .collect();
 
+    // Last NVMe read timeout seen on each drive, for the error message of the
+    // incomplete-object pass below (None: no timeout on that drive).
+    let mut timeout_msg: Vec<Option<String>> = vec![None; num_drives];
+
     let mut stream_idx: usize = 0;
 
@@ -1400,13 +1404,18 @@ pub unsafe fn scatter_gather_multi_drive_zero_copy(
                 completed[d] += 1;
                 total_completed += 1;
-                for r in all_results[d].iter_mut() {
-                    if r.is_ok() {
-                        *r = Err(DispatcherError::IoError(format!(
-                            "NVMe read timeout drive={d} (handle {handle:?})"
-                        )));
-                    }
-                }
-                obj_failed[d].fill(true);
-                stop_submitting[d] = true;
+                // One read on this drive timed out. The completion carries only
+                // the actor-assigned handle — not the caller tag — so the
+                // affected object cannot be named here, but this is NOT a
+                // whole-drive failure (US-11 AS-4): the actor forgets a
+                // timed-out op, so that segment's ReadDone never arrives and
+                // its object is exactly the one left with segments outstanding
+                // after the loop. Keep submitting and draining; the
+                // incomplete-object pass below fails that entry alone, and
+                // every other entry keeps its completions and is processed
+                // independently. A late drive write for the forgotten op can
+                // only land in the timed-out object's own slot: each segment
+                // reads into its own chunk of its own object's slot, and the
+                // chunk_bufs Arc keeps that memory alive for the whole call.
+                timeout_msg[d] = Some(format!("NVMe read timeout drive={d} (handle {handle:?})"));
             }
             Ok(_) => {
@@ -1470,4 +1479,19 @@ pub unsafe fn scatter_gather_multi_drive_zero_copy(
             }
         }
+        // An object whose reads did not all complete — its NVMe read timed out
+        // (the actor forgets a timed-out op, so its ReadDone never arrives and
+        // obj_remaining never reaches zero) — is failed for that entry only;
+        // the objects that did complete were already delivered or failed
+        // above. This is the per-entry attribution the timeout completion
+        // itself cannot provide (US-11 AS-4).
+        for obj_idx in 0..ds.objs.len() {
+            if obj_remaining[d][obj_idx] > 0 && all_results[d][obj_idx].is_ok() {
+                all_results[d][obj_idx] = Err(DispatcherError::IoError(
+                    timeout_msg[d]
+                        .clone()
... (6 more lines in the commit)
```

### The test that shows the bug: `components/dispatcher/src/repair_test_nvme_timeout_fails_whole_drive.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `nvme-timeout-fails-whole-drive`
//! (DIS-BATCH-LOOKUP-ERROR-ISOLATION, US-11 AS-4 "Edge case one fails").
//!
//! A read or DMA failure on one cold entry of a batch must be reported only
//! for that entry; the other entries are processed and reported independently.
//! A plain read error already fails only its own object; the defect is the
//! NVMe read **timeout**: `Completion::Timeout` carries only the actor-assigned
//! handle (not the caller tag), and the scatter-gather completion loop used
//! that as a reason to fail every entry on the drive.
//!
//! The dispatcher gets one mock data drive whose NVMe reads time out for
//! chosen keys (the mock actor delivers `Completion::Timeout` for their LBAs
//! and never completes them, exactly as the real actor forgets a timed-out
//! op). The other keys' reads succeed. The drive is placed in `data_drives`
//! before `initialize`, which keeps it in memory-tier-only mode (no SPDK
//! environment, no factory).
//!
//! Self-contained: it drives the public `IDispatcher` API through its own
//! mocks and touches only items that exist in the unfixed code as well.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatchMapError, DispatcherConfig, DmaBuffer,
    Extent, ExtentManagerError, FormatParams, GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, GpuStream,
    IBlockDevice, IDispatchMap, IDispatcher, IExtentManager, IGpuServices, IMemoryTier, IpcHandle,
    LookupOutcome, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, NvmeBlockError,
    OpHandle, TelemetrySnapshot, WriteHandle,
};

use crate::{ChannelPool, DataDrive, DispatcherComponent};

const BLOCK: u32 = 4096;
/// Bytes of disk per key: key `k` lives at byte offset `k * EXTENT`.
const EXTENT: u64 = 16 * BLOCK as u64;

// --- dispatch map: entries with reference counts, as the real one enforces them ---
... (832 more lines)
```


Traces: US-11 AS-4, 002 Edge case one fails

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem: nvme-timeout-fails-whole-drive (DIS-BATCH-LOOKUP-ERROR-ISOLATION)

## One sentence

When one cold entry's NVMe read in a batch times out, the dispatcher marks **every** entry on
that drive as failed, so a single slow read turns a whole batch's worth of good entries into
errors.

## Spec vs code

**Spec** — `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:217`
(User Story 11, Acceptance Scenario 4):

> Given a cold entry's NVMe read or GPU DMA fails, When the scatter-gather loop encounters
> the error, Then the error is reported for that entry only; other entries in the batch
> continue to be processed independently.

**Code** — `components/dispatcher/src/pipeline.rs:1398-1410`, the
`Ok(Completion::Timeout { handle })` arm of the multiplexed completion loop in
`scatter_gather_multi_drive_zero_copy`:

```rust
Ok(Completion::Timeout { handle }) => {
    ...
    for r in all_results[d].iter_mut() {
        if r.is_ok() {
            *r = Err(DispatcherError::IoError(format!(
                "NVMe read timeout drive={d} (handle {handle:?})"
            )));
        }
    }
    obj_failed[d].fill(true);
    stop_submitting[d] = true;
}
```

A `ReadDone` with `Err` (a plain read failure) fails only its own object
(`all_results[d][obj_idx]`, pipeline.rs:1341-1347), and a DMA failure fails only its own
object (pipeline.rs:1379-1383). The timeout arm is the exception: it fails **all** jobs on
drive `d`, marks all of the drive's objects failed, and stops submitting the drive's
remaining segments — so entries whose reads were fine (or never got submitted) are reported
as errors and their slots are released by `batch_lookup`'s result-processing loop
(lib.rs:3035-3041). The reported symptom is a batch where one timed-out read erases the
whole drive's worth of results.

Root cause: `Completion::Timeout` carries only the actor-assigned `OpHandle`, not the
caller tag, so the loop cannot name the affected object and takes the conservative way out
of failing the entire drive.

## Verdict

**Code wrong.** The spec's per-entry error isolation (US-11 AS-4) is explicit; the code's
whole-drive escalation contradicts it. The fix must attribute the timeout to exactly the
entry whose read did not complete — which the loop can do without the tag: the block-device
actor forgets a timed-out op (its `ReadDone` never arrives), so the affected object is
precisely the one left with `obj_remaining > 0` when the loop drains.

## Attempt 2 note (2026-11-04)

The first attempt's fix was not rejected on its merits: the acceptance check **timed out
after 27000 s** before reaching a verdict. Reconstruction from the check's own artifacts
(`~/.cache/repair-gate*`): (a) this was the first repair on base b7d4e9b1f, so the check's
"before" re-verification pass had no cache and ran a full ~4.6 h Creusot gate scoring; (b) a
second repair job (worktree rc23, DIS-TIER-STATS-ROUTE-PARTITION, same base) ran its
acceptance check concurrently, halving effective CPU. Both costs are now gone — the
before-side gate result for this base is cached (114 proved, complete with SUMMARY), and
every leg of the check has been re-verified directly (see report.md). The verdict above is
unchanged: code wrong, and the fix on disk is the correct one.

# Repair report: nvme-timeout-fails-whole-drive (DIS-BATCH-LOOKUP-ERROR-ISOLATION)

## In short

Problem: one timed-out NVMe read in a batch's cold path failed every entry on that
drive (pipeline.rs `Timeout` arm), violating per-entry error isolation (US-11 AS-4).
Fix: keep submitting and draining; the timed-out entry is exactly the object left
incomplete, which is failed alone after the loop while every other entry completes.

## Bug

`batch_lookup`'s cold path groups cold entries per drive and multiplexes all their
segments over one `ChannelLease` in `scatter_gather_multi_drive_zero_copy`
(`components/dispatcher/src/pipeline.rs:1086`). Per-object accounting
(`obj_remaining`/`obj_failed`) already isolates ordinary failures: a `ReadDone`
carrying `Err` fails only its own object, and a GPU DMA failure fails only its own
object.

The `Ok(Completion::Timeout { handle })` arm was the exception
(pipeline.rs:1402 before the fix): it wrote an `IoError` into **every** still-ok
result on drive `d`, set `obj_failed[d]` for all of the drive's objects, and set
`stop_submitting[d] = true`. One slow read therefore erased the whole drive's worth
of batch results — entries whose reads had already succeeded, and entries whose
segments were never even submitted, were all reported as errors.

Root cause: `Completion::Timeout` carries only the actor-assigned `OpHandle`, not
the caller tag, so the loop cannot name the affected object at the moment the
completion arrives and took the conservative way out of failing the entire drive.
But the actor *forgets* a timed-out op (`check_timeouts` removes it from
`pending_ops`, so its `ReadDone` never arrives, and real completions for removed
handles are discarded) — which means the affected object is deterministically the
one left with `obj_remaining > 0` when the loop drains. The information was always
there; the arm just did not use it.

## Impact

Any NVMe read timeout (default 30 s, pipeline.rs:60) during a batch lookup turned
every cold entry sharing that drive into a `DispatcherError::IoError` — wrong
results reported to the caller for entries whose data was read correctly, plus
unnecessary slot releases and (with the escalation arms) lost submissions for
entries never attempted. Under a flaky drive this multiplies a single-entry fault
into a whole-batch outage, which is exactly the scenario US-11 AS-4 ("002 Edge
case one fails") forbids. Not a data-corruption bug: the failure direction is
spurious errors, not wrong data.

## How to reproduce

`components/dispatcher/src/repair_test_nvme_timeout_fails_whole_drive.rs` (wired
at `components/dispatcher/src/lib.rs:84-86`) provides a self-contained mock stack
(MockNvme returns `Completion::Timeout` for chosen LBAs, mirroring the actor's
behavior) and four tests, including a no-timeout control:

```bash
cargo test -p dispatcher repair_test_nvme_timeout
```

On the unfixed code, `batch_lookup_times_out_one_entry_only` and
`batch_lookup_times_out_two_entries_independently` fail: the healthy entries come
back `served_by == Error` instead of `Ssd` with the correct bytes. On the fixed
code all four pass.

## Fix

Three hunks in `scatter_gather_multi_drive_zero_copy`
(`components/dispatcher/src/pipeline.rs`), all in the direction of *less*
escalation:

1. pipeline.rs:1223-1225 — a per-drive `timeout_msg: Vec<Option<String>>`
   remembering the last timeout (and its handle) for the error message.
2. pipeline.rs:1402-1420 — the `Timeout` arm now only records that message and
   counts the completion. It no longer touches other entries' results, no longer
   marks the drive's objects failed, and no longer stops submitting: the actor
   has forgotten the timed-out op, so that object is exactly the one that will
   not complete, and the loop keeps draining the drive's other segments.
3. pipeline.rs:1480-1496 — after the drain loop (next to the existing
   "unsubmitted work" pass), an incomplete-object pass: any object with
   `obj_remaining > 0` and an ok result is failed with the recorded timeout
   message (fallback string if the drain ended without a Timeout completion,
   e.g. channel close). This is the per-entry attribution the completion itself
   cannot provide.

Why this is safe for the other entries: each segment reads into its own chunk of
its own object's slot, and `chunk_bufs` keeps those Arcs alive for the whole call,
so a late drive completion for the forgotten op can only land in the already-failed
object's own slot. Promotion (`promote_block_to_memory_tier`) happens per job from
`all_results`, so healthy entries are still promoted and served independently.
The unit's queue-depth and channel accounting are unchanged: the Timeout
completion is still counted (`completed[d]`, `total_completed`) as that op's
completion, matching the actor's bookkeeping (a removed op's real completion is
discarded, so counting the Timeout keeps the lease balanced).

**Rejected alternatives:**

- *Thread the caller tag through `Completion::Timeout`* (change
  `interfaces/src/iblock_device.rs`): correct in principle but touches a shared
  interface and the block-device actor outside this component's write scope, and
  it is unnecessary — the incomplete-object pass derives the same attribution
  from state the loop already keeps.
- *Fail the whole drive but keep submitting* (drop only `stop_submitting`):
  still reports healthy entries as failed; violates AS-4.
- *Retry the timed-out read in the loop*: changes latency and drive-load
  semantics beyond the finding; the spec asks for error isolation, not retry.

Formal verification (`components/dispatcher/verif-creusot/`):

- `src/batch.rs`: `ColdG` gained `ok: Seq<bool>` (batch index → the entry's cold
  read completed *and* its promotion landed); new ghost struct `PgG { prom, ok }`
  is returned by `bl_cold_pooled`, threaded through `bl_cold` into `ColdG` and
  `batch_lookup`'s `BlG`, with loop invariants on the promote pass and
  pass-through ensures; the property is stated at `batch_lookup` level: if entry
  `i` reports `Err(Io(Ssd))` while cold entry `j` (single region) has
  `cold.ok[j]`, then `result[j] == Ok(())`.
- `src/drivers.rs` `verify_dis_batch_lookup_error_isolation` (two cold entries,
  one can fail) and its `__mutant` twin (asserting the old whole-drive behavior:
  the healthy entry may also be `Err`).
- `src/boundary.rs`: `sg_job`'s doc comment states the per-job timeout
  attribution; its contract (`result == o_ssd(*s.g)`) already modeled the fixed
  behavior.

**Why the first acceptance run timed out, and why that is resolved.** The first
attempt's fix was never evaluated on its merits: the acceptance check ran out of
its 27000 s budget. Reconstruction from the check's own artifacts shows where the
time went: (a) this was the first repair on base b7d4e9b1f, so the check's
"before" re-verification pass had no cache and ran a full ~4.6 h Creusot gate
scoring before the "after" pass could even start; (b) a second repair job
(worktree rc23, same base commit) ran its acceptance check concurrently on the
same machine, roughly halving effective CPU for both. Neither cost recurs for this
attempt: the before-side gate result for this base is now cached (114 proved, 0
lost — the in-progress after-side pass from the first attempt, still running at
attempt-2 start, had already re-scored all 114 base-proved properties as proved
with zero losses), and the check's remaining legs were re-verified directly this
time (see Risk). No code, test or proof change was needed for the timeout; the fix
on disk is attempt 1's fix, unchanged.

## Scope beyond the finding

None in behavior. The other three `Completion::Timeout` arms in pipeline.rs
belong to paths that are per-entry by construction: `pipelined_ssd_to_gpu_zero_copy`
(single object per call), `pipelined_ssd_to_dram_only` (called per entry inside a
loop, lib.rs:4360), and `pipelined_multi_object_zero_copy` (currently only ever
called with a single job, lib.rs:1003). The src/ diff is the three hunks above
plus test wiring; the verif diff is the `ColdG.ok`/`PgG` threading, the two new
drivers, and a doc comment. (Re-running `cargo creusot` during verification
regenerated `.coma`/`proof.json` artifacts under `verif-creusot/verif/`; those
carry no semantic change.)

## Risk

Low. The change only *narrows* which results an error is written into: the
timeout arm no longer mutates other entries' results at all, and the new
post-loop pass only fires for objects that would otherwise silently lack a
completion. The failure mode of a mistake would be an entry reported failed that
should have succeeded — covered by the four new tests (including the two-entry
and two-timeout independence cases and the no-timeout control).

Verification re-run from scratch in attempt 2, all in the foreground:

- Crate tests: 13 consecutive `cargo test -p dispatcher` runs, **183 passed,
  0 failed every run**, no flakes.
- Red-first re-verified exactly as the check does it: fresh worktree at base
  b7d4e9b1f, the test file injected alone, `cargo test repair_test_` →
  `FAILED. 60 passed; 2 failed` (it fails on the unfixed code by assertion, not
  by a compile error).
- Leg-1 modules re-proved with forced replay: `batch_lookup` PROVED,
  `verify_dis_batch_lookup_error_isolation` PROVED (0.6 s warm — comfortably
  inside the gate's 60/300 s caps), and
  `verify_dis_batch_lookup_error_isolation_mutant` FAILS with proof-goal
  evidence (`✘ (1/2)`, "1 unproved file") — the anti-vacuity twin has content.
- Whole-crate HARMLESS leg: `shutdown` (flagged FAILED by attempt 1's contended
  sweep — an artifact of running under the parallel job's CPU load) re-proved
  cleanly in seconds; the sweep's remaining 24 non-mutant failures are exactly
  the base's cached unproved set, so no proof regresses.
- Every one of the 114 base-proved properties (the leg-E "lost" comparison) is
  confirmed proved on the fixed tree: 106 re-scored proved by the gate's own
  after-side pass with zero losses, the last 8 (`DIS-TIER-STATS-*`,
  `DIS-INV-*`, `DIS-FLUSH-REPORTS-FAILED-WRITE`) re-proved directly.
- Dependent crates (`certus-server`, `certus-server-yaml`, `shmq-dispatcher`,
  `remote-lookup-bench`, `workload-node-agent`) still `cargo check` clean.
- `cargo fmt -p dispatcher --check`: the only diff is the pre-existing lib.rs
  module-order blank line at lib.rs:8789, present at HEAD and untouched here.

## Follow-ups

- `pipelined_multi_object_zero_copy` shares one `outcome` across all jobs in its
  loop; it is per-entry today only because every caller passes a single job.
  If it ever serves multiple jobs, it needs the same incomplete-object
  attribution (or per-object results like `scatter_gather_multi_drive_zero_copy`).
- The conservative arms in `scatter_gather_multi_drive_zero_copy` — "unexpected
  completion", "invalid tag", "completion channel closed" — still fail the whole
  drive. These indicate channel/protocol corruption rather than a per-read
  fault, so whole-drive failure is defensible, but they have the same shape as
  this finding and deserve their own spec decision.
- The channel-checkout escalation in `batch_lookup` (lib.rs, `chan_checkout`
  oracle) can leave `None` results, which the attribution loop would unwrap;
  distinct root cause (pool exhaustion), not fixed here.
- The 24 base-unproved non-mutant modules should be re-proven or re-triaged by
  whoever owns the rw_stats_baseline / memcpy_batch_async merges.
- Queue scheduling: two repair jobs on the same base running their acceptance
  checks concurrently doubled the gate cost and caused the first attempt's
  timeout (both ran uncached ~4.6 h before-side passes against the same base
  commit). Running the queue's jobs strictly one at a time — or keying the
  before-side gate cache so concurrent jobs on the same base share an in-flight
  pass — would remove this failure mode.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-BATCH-LOOKUP-ERROR-ISOLATION`, callee modules `batch_lookup`.

```
ACCEPT-REPO: base b7d4e9b1f4f60637662bf6b97997ce096f8348e1; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (183 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-BATCH-LOOKUP-ERROR-ISOLATION yes
  D. MODEL   : dispatcher yes
  E. (before side for dispatcher reused from cache: same base, proof crate and gate)
  E. REVERIFY: dispatcher yes (114 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 183 passed in each run
  dispatcher                   | DIS-BATCH-LOOKUP-ERROR-ISOLATION         | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 114 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 194 min | 450 min | rejected |
| 2 | 36 min | 112 min | accepted |

Total wall-clock: 792 min.
