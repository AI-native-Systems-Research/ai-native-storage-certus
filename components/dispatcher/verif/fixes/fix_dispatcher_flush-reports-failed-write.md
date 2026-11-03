# fix_dispatcher_flush-reports-failed-write

> **In short** - Problem: when an SSD write failed, `flush_to_ssd` still said "OK", so a later `clear_memory_tier` silently threw away the only copy of that cache entry; its count was also wrong. Fix: write failures are now recorded; flush returns an error if one is still resident, else the persisted-entry count.


```
component:      dispatcher
finding:        flush-reports-failed-write
property:       DIS-FLUSH-REPORTS-FAILED-WRITE
found by:       see "How it was found"
base commit:    12197f982a41        branch: repair/dispatcher-flush
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
When flush_to_ssd returns Ok, every entry populated before the call has its data on the block device with an SSD offset recorded, and the returned count is the number of entries that now have a valid SSD offset; if a write-through to the block device failed, flush_to_ssd returns Err instead. Thus a clear_memory_tier after an Ok flush never removes the only copy of an entry.

## At a glance: spec, code, test

### What the spec says
(no spec line cited - a property the code itself documents)

### The code: before (-) and after (+)
```diff
@@ -2,5 +2,5 @@
 
 use crossbeam_channel::{Receiver, Sender};
-use std::collections::HashMap;
+use std::collections::{HashMap, HashSet};
 use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
 use std::sync::{Arc, Mutex};
@@ -254,7 +254,12 @@ impl EvictionSender {
 /// `remove` can take it over, and makes the job and `remove` agree on who
 /// releases the pin and who frees the DRAM slot the job may still be reading.
+///
+/// It also records the keys whose latest write-through failed, so that
+/// `flush_to_ssd` can report an entry left with no SSD copy.
 #[derive(Default)]
 pub struct WriteThroughPins {
     keys: Mutex<HashMap<CacheKey, PinState>>,
+    /// Keys whose latest write-through did not land on the block device.
+    failed: Mutex<HashSet<CacheKey>>,
 }
 
@@ -356,4 +361,23 @@ impl WriteThroughPins {
     }
 
+    /// A write-through for `key` was settled: `landed` if its data reached the
+    /// block device and the SSD offset was recorded.
+    pub(crate) fn record_write(&self, key: CacheKey, landed: bool) {
+        let mut failed = self.failed.lock().unwrap();
+        if landed {
+            failed.remove(&key);
+        } else {
+            failed.insert(key);
+        }
+    }
+
+    /// Number of keys whose latest write-through failed and that `resident`
+    /// still holds; keys no longer resident (removed since) are forgotten.
+    pub(crate) fn failed_writes(&self, resident: impl Fn(CacheKey) -> bool) -> usize {
+        let mut failed = self.failed.lock().unwrap();
+        failed.retain(|&key| resident(key));
+        failed.len()
+    }
+
     fn forget_if_idle(keys: &mut HashMap<CacheKey, PinState>, key: CacheKey) {
         if keys
@@ -1394,4 +1394,5 @@ impl DispatcherComponent {
     }
 
+    /// Returns whether the data landed (an SSD offset was recorded).
     fn process_write_job(
         dm: &Arc<dyn IDispatchMap + Send + Sync>,
@@ -1400,11 +1401,13 @@ impl DispatcherComponent {
         extent_mgrs: &[Arc<dyn IExtentManager + Send + Sync>],
         job: WriteJob,
-    ) {
+    ) -> bool {
         let written = Self::write_through(mt, drives, extent_mgrs, &job);
         Self::settle_write_through(dm, job.key, written);
+        written.is_some()
     }
 
... (74 more lines in the commit)
```

### The test that shows the bug: `components/dispatcher/src/repair_test_flush_failed_write.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `flush-reports-failed-write`
//! (DIS-FLUSH-REPORTS-FAILED-WRITE).
//!
//! `flush_to_ssd` promises that every entry populated before the call has its
//! data on the block device once it returns `Ok`, and that the returned count is
//! the number of entries that now have a valid SSD offset. If a write-through to
//! the block device failed, it must return `Err` instead: otherwise a
//! `clear_memory_tier` after the `Ok` flush drops the only copy of the entry.
//!
//! The dispatcher gets one data drive whose NVMe mock fails the write of a chosen
//! key. The drive is placed in `data_drives` before `initialize`, which in
//! memory-tier-only mode (no SPDK environment, no factory) keeps it and hands it
//! to the background writer, so the real write-through path runs against it.
//! Entries are stored the way the shm-queue store path does it:
//! `reserve_memory` then `copy_gpu_to_memory_completed`.
//!
//! Self-contained: it drives the public `IDispatcher` API through its own mocks
//! and touches only items that exist in the unfixed code as well.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    BlockDeviceError, CacheKey, ClientChannels, Command, Completion, DispatchMapError,
    DispatcherConfig, Extent, ExtentManagerError, FormatParams, IBlockDevice, IDispatchMap,
    IDispatcher, IExtentManager, IMemoryTier, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot, NvmeBlockError, OpHandle, TelemetrySnapshot, WriteHandle,
};

use crate::{ChannelPool, DataDrive, DispatcherComponent};

const BLOCK: u32 = 4096;
/// Bytes of disk per key: key `k` is written at byte offset `k * EXTENT`.
const EXTENT: u64 = 16 * BLOCK as u64;

// --- dispatch map: entries with reference counts, as the real one enforces them ---

... (619 more lines)
```


Traces: Spin J23 flush_clear PA, PB, PC (node7, reproduced on green 2026-10-20), components/interfaces/src/idispatcher.rs flush_to_ssd doc (persistence guarantee, return value), Decision by Cornel 2026-10-20: flush_to_ssd returns Err when a write-through failed (FV_NOTEBOOK/decisions/decision_dispatcher_flush-to-ssd-and-a07.md)

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem: flush-reports-failed-write (DIS-FLUSH-REPORTS-FAILED-WRITE)

**In one sentence:** when copying a cached entry to the SSD fails, `flush_to_ssd` still says "OK", so a
caller who then empties the DRAM tier with `clear_memory_tier` loses the only copy of that entry.

## Where spec and code disagree

- **Spec** — `components/interfaces/src/idispatcher.rs:824-832` (`flush_to_ssd` doc): "Guarantees that
  every entry populated before this call has its data persisted to the block device … Returns the number
  of entries that now have a valid SSD offset." It is refined by the decision of 2026-10-20
  (`FV_NOTEBOOK/decisions/decision_dispatcher_flush-to-ssd-and-a07.md` §1): `flush_to_ssd` returns `Err`
  if a write-through to the block device failed, and the count is the number of entries that now have
  a valid SSD offset.
- **Code** — `components/dispatcher/src/lib.rs:3989-4002` (`flush_to_ssd`) only waits for the write queue
  to be empty and always returns `Ok(in_flight-at-call-time)`. A failed write-through is dropped silently
  at `components/dispatcher/src/lib.rs:1442-1456` (`settle_write_through`, the `None` arm just releases
  the pin) after `write_through` (`lib.rs:1460-1530`) returns `None` on any reservation or SSD-write error.
  Nothing records the failure, so `flush_to_ssd` cannot report it. The returned count is the number of
  jobs queued when the call started, not the number of entries with an SSD offset.

## Verdict

**Code wrong.** The interface promise and the approved decision agree; the implementation breaks both
(no error on a failed write, and the wrong count).

# Repair: flush-reports-failed-write (DIS-FLUSH-REPORTS-FAILED-WRITE)

## In short
Problem: when an SSD write failed, `flush_to_ssd` still said "OK", so a later `clear_memory_tier` silently threw away the only copy of that cache entry; its count was also wrong.
Fix: write failures are now recorded; flush returns an error if one is still resident, else the persisted-entry count.

## Bug
`flush_to_ssd` only waited for the write queue to drain and always returned `Ok(jobs queued at call time)`, even when a write-through to the block device had failed and left an entry with no SSD copy.

## Impact
A caller that does the documented "flush, then clear the memory tier" sequence (the Python test suites and the shm-queue `OP_FLUSH_TO_SSD` client do this) was told its data was durable when it was not. The following `clear_memory_tier` then removed the entry from both tiers, so the next lookup misses: the cached value is lost without any error being reported. Separately, the returned number was the count of jobs still queued at the start of the call (often 0), not the documented "number of entries that now have a valid SSD offset".

## How to reproduce
1. `cargo test -p dispatcher --lib repair_test_flush_failed_write` on the code before this fix. The test (`components/dispatcher/src/repair_test_flush_failed_write.rs`) puts one mock NVMe drive into the dispatcher that fails the write of key 2, stores keys 1 and 2, and calls `flush_to_ssd`.
2. Observe `flush_to_ssd returned Ok(2) although the write-through of key 2 failed`. The second case observes `Ok(0)` where 2 entries have an SSD offset.
3. Loom: `cd components/dispatcher/verif-loom && RUSTFLAGS="--cfg loom" cargo test --release verify_dis_flush_reports_failed_write -- --exact` fails before the fix and passes after it.

## Fix
- `WriteThroughPins` (`src/background.rs`), the ledger the background writer and `remove` already share, now also keeps a set of keys whose **latest** write-through failed. `record_write(key, landed)` inserts or removes a key. `failed_writes(resident)` drops keys that are no longer in the memory tier and counts the rest.
- `run_write_job` (`src/lib.rs`) records the outcome of every write-through it settles, on both the pinned (populate) path and the untracked (`schedule_write_through`) path. `process_write_job` now returns whether the data landed. The job that `remove` took over records nothing, because its entry is gone.
- `flush_to_ssd` waits for the queue as before. If any key that is still resident has a failed latest write-through, it returns `Err(IoError("… N memory-tier entries have no SSD copy …"))`. Otherwise it returns the number of memory-tier entries that have an SSD offset (`is_evictable`, i.e. persisted and unpinned) — the documented count.
- A failure is cleared when a later write-through of the same key lands (for example a retry via `schedule_write_through`), or when the key leaves the memory tier (removed, cleared or evicted). So `Err` always means "a resident entry has no SSD copy because its write failed". It never refers to an entry that no longer exists.

Alternatives rejected:
- *Scan the tier for entries without an SSD offset and fail on any of them.* That would also fail for entries that were never meant to be written: `schedule_write_through` is best-effort, and remote-published entries may never get a job. Their absence of a copy is not a failed write, and the decision says to report a *failed* write-through.
- *A per-call failure counter (failures since flush started).* It misses a failure that happened just before the call, for a job that had already been processed when `flush_to_ssd` began. That entry is still unpersisted, and the following clear would drop it. This was Spin's PA/PB interleaving.
- *Keep the counter `in_flight()` but fix only the error path.* Rejected because the decision explicitly asks for the count to match the doc.
- *Change `process_write_job`/`settle_write_through` to take the ledger.* This was tried first and works. But the Loom test file calls `process_write_job(dm, mt, drives, job)` directly, and changing its signature would have meant editing the judged test file. Recording the result in the caller (`run_write_job`) keeps that signature and is equivalent.

Verification:
- Unit: the new repair test passes. `cargo test -p dispatcher` passed 5/5 runs on the final code (99 lib + 5 + 14 integration tests).
- Loom: `verify_dis_flush_reports_failed_write` **passes** (406 s, no preemption bound, as in the test). `verify_dis_flush_reports_failed_write__mutant` (FlushNoWait) still **fails**. The mirror follows the fixed code line for line: `WriteThroughPins.failed`, `record_write`, `failed_writes`, `process_write_job -> bool`, `run_write_job`, `flush_to_ssd`, plus a mirror of `IDispatchMap::is_evictable`. All `// mirrors` line references are refreshed, and `mirrored_sources.sha256` matches the fixed `src/lib.rs` and `src/background.rs` (`sha256sum -c` OK). As regression checks, `verify_dis_flush_then_clear_demotes` passes and its mutant fails. `verify_dis_remove_no_wait` and `verify_dis_spin_clear_mt_keeps_pinned` pass.
- Creusot: the model gets a ghost field `wf` (the failed set) with trusted boundary specs `pins_record_write` / `pins_failed_writes`. `writer_step` records the job's outcome. `flush_to_ssd` mirrors the new error check and count loop, and its contract now states the property. A new driver `verify_dis_flush_reports_failed_write` is **proved** and its `__mutant` twin fails. It is also added to `tools/gen_drivers.py`, so regeneration keeps it. These are proved: `flush_to_ssd`, `drain`, `writer_step`, `process_write_job`, `verify_dis_flush_persists_all`, `verify_dis_schedule_wt_queues` (both mutants fail).

## Scope beyond the finding
None in the shipped code. In the verification artifacts:
- Loom gains a mirror of `is_evictable`, which the new `flush_to_ssd` calls.
- Creusot: `refute_dis_flush_returns_count` **no longer proves**. This is intended: it refuted the old count ("returns 0 while an entry is persisted"), and that negation is no longer true. DIS-FLUSH-RETURNS-COUNT should be re-scored from *refuted* to proved/open by the scorer. I did not edit the property bundle.

## Risk
- Behaviour change: `flush_to_ssd` can now return `Err(IoError)`. Existing callers in this repo `.unwrap()` it only in unit tests with no failing drive. The Python callers through the shm-queue get an error status where they used to get a count.
- The return value changes meaning, from "jobs queued at call time" to "entries with an SSD offset". Python test output that prints "Flushed: N jobs" will show different numbers. No caller checks the value.
- Cost: `flush_to_ssd` now walks the memory tier once (`oldest_keys(usize::MAX)` plus one `is_evictable` per key). It is a blocking, admin-style call, so this is acceptable. The writer's per-job overhead is one extra short mutex hold on a `HashSet`.
- The failed set is bounded by the number of resident keys, and it is pruned on every flush.
- `flush_to_ssd` now also requires the `dispatch_map`/`memory_tier` receptacles. Both are already required by `initialize`, so an initialized dispatcher always has them.

## Follow-ups
- Interface doc and spec text, per the decision: add "or returns `Err` if a write-through to the block device failed" to `components/interfaces/src/idispatcher.rs` (`flush_to_ssd`) and to the dispatcher spec (`specs/001-dispatcher-cache-interface/spec.md`, the `flush_to_ssd() -> usize` bullet, which still says "number of entries flushed"). This is outside my write scope. The decision also asks for a transitive drift re-stamp.
- Re-score DIS-FLUSH-RETURNS-COUNT (see Scope above).
- These failures already exist at HEAD and are not caused by this change. I confirmed each one on an unmodified copy of HEAD:
  - Loom `verify_dis_remove_order` fails (`[DmRemove, MtFree]` vs expected `[MtFree, DmRemove, ExtentFree]`).
  - Creusot `shutdown` times out at 600 s.
  - Creusot `refute_dis_schedule_wt_pin_neutral` and `verify_dis_inv_not_initialized` (21/23 goals) do not prove.
- `cargo clippy -D warnings` already fails on lines this change does not touch (lib.rs:1024, 2439, 5838 and extent-manager). It reports nothing on the changed lines.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-FLUSH-REPORTS-FAILED-WRITE`, callee modules ``.

```
ACCEPT-REPO: base 12197f982a41e77ad6557d0cf0ed3af17ce3d5a9; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (118 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-FLUSH-REPORTS-FAILED-WRITE yes (Loom)
  E. REVERIFY: dispatcher yes (111 proved after the repair; 1 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 118 passed in each run
  dispatcher                   | DIS-FLUSH-REPORTS-FAILED-WRITE (Loom)    | violated before -> PROVED in Loom's search (all interleavings)); twin fails; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 111 proved, 1 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 158 min | 444 min | accepted |

Total wall-clock: 602 min.
