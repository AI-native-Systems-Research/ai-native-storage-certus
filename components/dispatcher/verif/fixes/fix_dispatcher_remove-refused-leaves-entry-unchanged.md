# fix_dispatcher_remove-refused-leaves-entry-unchanged

> **In short** - Problem: A `remove` refused because a reader or client pin held the entry had already flipped that entry from DRAM-resident (MemoryTier) to SSD-only (BlockDevice), so later lookups were served from the SSD instead of DRAM. Fix: The refused path now flips the entry back in place (`promote_block_to_memory_tier`, the convert's inverse) before returning `KeyNotFound`, leaving the entry exactly as it was.


```
component:      dispatcher
finding:        remove-refused-leaves-entry-unchanged
property:       DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED
found by:       see "How it was found"
base commit:    a766e01df005        branch: repair/dispatcher-remove
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
When remove returns KeyNotFound because a reader, a client pin or a concurrent lookup holds a MemoryTier entry, the entry is unchanged - it is still MemoryTier with its memory-tier slot and its ssd_offset, nothing is freed, a later lookup is served from DRAM, and after the hold is released a remove succeeds and frees both the slot and the SSD extent.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:239`
```
- When the SSD evictor runs but all candidate entries have active references, no entries are evicted in that sweep. The evictor re-checks on its next interval.
- When the SSD evictor removes an entry, it first removes the dispatch-map entry. If the entry is pinned, that removal is refused and the candidate is skipped. Then it calls `IMemoryTier::remove` for the key. This is normally a no-op, because a BlockDevice candidate is not resident, but it is needed for the race in which a cold lookup promoted the key after the evictor's check. Finally it frees the extent via the extent manager (FR-032).
```
`components/dispatcher/specs/001-dispatcher-cache-interface/contracts/idispatcher.md:171`
```
- `remove()` guarantees, when it returns `Ok`, that the dispatch map entry is removed, so no new lookup can obtain its memory-tier pointer. When it returns `KeyNotFound`, nothing was freed.
- `prepare_store()` guarantees the key is visible via `check()` after return.
```

### The code: before (-) and after (+)
```diff
@@ -3199,4 +3199,9 @@ impl IDispatcher for DispatcherComponent {
         // write-through pin a concurrent populate leaves on the entry it
         // registers in the window between this lookup and the release.
+        // The memory-tier state this call's pre-pass flips away, to restore if
+        // the removal below is refused: a refused remove (`KeyNotFound`)
+        // frees nothing and changes nothing, so the entry must still be
+        // MemoryTier, with its memory-tier slot and its ssd_offset.
+        let mut flipped_memory_tier: Option<(*mut u8, u32)> = None;
         let block_offset = match dm.lookup(key) {
             Ok(LookupResult::BlockDevice { offset }) => {
@@ -3204,5 +3209,5 @@ impl IDispatcher for DispatcherComponent {
                 Some(offset)
             }
-            Ok(LookupResult::MemoryTier { .. }) => {
+            Ok(LookupResult::MemoryTier { pointer, size }) => {
                 // A resident entry may already carry its write-through SSD
                 // copy (`ssd_offset`), which this lookup result does not
@@ -3222,4 +3227,7 @@ impl IDispatcher for DispatcherComponent {
                         Ok(LookupResult::BlockDevice { offset }) => {
                             let _ = dm.release_read(key);
+                            // This call flipped the entry away: remember how
+                            // to put it back if the removal is refused.
+                            flipped_memory_tier = Some((pointer, size));
                             Some(offset)
                         }
@@ -3252,8 +3260,23 @@ impl IDispatcher for DispatcherComponent {
             // FR-018: the pin of a pending write-through does not hold remove up;
             // remove takes it over instead of waiting for the job.
-            Err(interfaces::DispatchMapError::ActiveReferences(_)) => self
-                .write_through_pins
-                .remove_entry(&*dm, key)
-                .ok_or(DispatcherError::KeyNotFound(key))?,
+            Err(interfaces::DispatchMapError::ActiveReferences(_)) => {
+                match self.write_through_pins.remove_entry(&*dm, key) {
+                    Some(job_frees_slot) => job_frees_slot,
+                    // A reader, a client pin or a concurrent lookup holds the
+                    // entry: the remove is refused. `KeyNotFound` promises that
+                    // nothing was freed and nothing changed, so put the entry back
+                    // the way this call found it — still MemoryTier, with its
+                    // memory-tier slot and its ssd_offset (the in-place promote
+                    // is the convert's inverse and keeps every reference) — and a
+                    // later lookup is served from DRAM again. Once the hold is
+                    // released, a later remove succeeds and frees both.
+                    None => {
+                        if let Some((pointer, size)) = flipped_memory_tier {
+                            let _ = dm.promote_block_to_memory_tier(key, pointer, size);
+                        }
+                        return Err(DispatcherError::KeyNotFound(key));
+                    }
+                }
+            }
             Err(_) => return Err(DispatcherError::KeyNotFound(key)),
         };
@@ -7401,2 +7424,6 @@ mod repair_test_remove_no_wait;
 #[path = "repair_test_remove_frees_ssd_extent.rs"]
 mod repair_test_remove_frees_ssd_extent;
+
+#[cfg(test)]
+#[path = "repair_test_remove_refused_leaves_entry_unchanged.rs"]
+mod repair_test_remove_refused_leaves_entry_unchanged;
```

### The test that shows the bug: `components/dispatcher/src/repair_test_remove_refused_leaves_entry_unchanged.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `remove-refused-leaves-entry-unchanged`
//! (DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED).
//!
//! `remove`'s location pre-pass flips a resident, written-through entry to its
//! block-device state (`convert_memory_tier_to_block`) to learn which SSD
//! extent it would have to free. When the removal itself is then refused —
//! a reader, a client `pin` or a concurrent lookup holds the entry, so
//! `dm.remove` answers `ActiveReferences` and the hold is not a write-through
//! pin `remove` could take over — the remove returns `KeyNotFound` but leaves
//! the entry flipped: it is BlockDevice instead of MemoryTier, so a later
//! lookup is served from the SSD instead of DRAM.
//!
//! The spec (spec.md:239, idispatcher.md:171 and proposal A-9) promises the
//! opposite: a refused remove frees *nothing* and changes *nothing* — the
//! entry is still MemoryTier with its memory-tier slot and its `ssd_offset`,
//! and once the hold is released a later remove succeeds and frees both the
//! slot and the SSD extent.
//!
//! Self-contained (its own mocks, the public `IDispatcher` API only), so it
//! builds against the dispatcher as it was before the repair as well as
//! after it.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use component_core::channel::SpscChannel;
use component_core::query_interface;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatchMapError, DispatcherConfig, DmaBuffer,
    Extent, ExtentManagerError, FormatParams, GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, GpuStream,
    IBlockDevice, IDispatchMap, IDispatcher, IExtentManager, IGpuServices, IMemoryTier,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, NvmeBlockError, OpHandle,
    TelemetrySnapshot, WriteHandle,
};

use crate::{ChannelPool, DataDrive, DispatcherComponent};

const BLOCK: u32 = 4096;
/// The one extent offset the mock extent manager hands out.
... (952 more lines)
```


Traces: specs/001-dispatcher-cache-interface/spec.md:239 (a refused remove returns KeyNotFound and frees nothing), specs/001-dispatcher-cache-interface/contracts/idispatcher.md:171 (KeyNotFound means nothing was freed), Spec-sync proposal A-9 (2026-10-23): remove flips a pinned MemoryTier entry to BlockDevice (src/lib.rs:3299) before dm.remove is refused (src/lib.rs:3331); a regression of 18c4f1f9, Decision by Cornel 2026-10-23: fix A-9 before the remove PR; test first

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem — DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED

**One sentence:** When `remove` is refused with `KeyNotFound` because a reader,
a client `pin` or a concurrent lookup still holds the entry, the dispatcher has
already flipped that entry from its DRAM-resident (MemoryTier) state to
SSD-only (BlockDevice), so the "refused" remove has in fact changed the cache:
later lookups of that key are served from the SSD instead of DRAM.

**Spec:** `specs/001-dispatcher-cache-interface/spec.md:239` — "A `remove`
that is refused because a reader or a client `pin` holds the entry ... returns
`KeyNotFound` and frees nothing"; `specs/001-dispatcher-cache-interface/contracts/idispatcher.md:171`
(postcondition) — "When it returns `KeyNotFound`, nothing was freed."
Spec-sync proposal A-9 (2026-10-23, approved) spells out the intended reading:
the refused remove leaves the entry *unchanged* — still MemoryTier with its
memory-tier slot and its `ssd_offset`, nothing freed, a later lookup served
from DRAM, and after the hold is released a remove succeeds and frees both the
slot and the SSD extent.

**Code:** `components/dispatcher/src/lib.rs:3218` — the location pre-pass of
`remove` calls `dm.convert_memory_tier_to_block(key)`, flipping a resident,
written-through entry to its block-device state to learn the extent offset it
must free. The removal itself is only attempted afterwards at
`components/dispatcher/src/lib.rs:3250`; when it is refused with
`ActiveReferences` and `write_through_pins.remove_entry` returns `None`
(the hold is a reader/client pin/concurrent lookup, not a write-through pin),
`remove` returns `KeyNotFound` at `components/dispatcher/src/lib.rs:3254-3257`
without ever undoing the flip. The entry is left in `BlockDevice` state —
the DRAM slot is still allocated but no lookup will find it, so the entry is
served from SSD and the slot is unreachable until a later remove of the key.

This is a regression of the 18c4f1f9 repair (DIS-REMOVE-RESOURCES-REUSABLE),
which introduced the flip so a remove can free the SSD extent of a
written-through resident entry.

**Verdict: code wrong.** The spec and the approved proposal A-9 agree; the
pre-pass mutates the entry before the removal that would authorize the
mutation is attempted, and leaves it mutated when the removal is refused.

# Repair report — DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED

## In short

Problem: A `remove` refused because a reader or client pin held the entry had already flipped
that entry from DRAM-resident (MemoryTier) to SSD-only (BlockDevice), so later lookups were
served from the SSD instead of DRAM.
Fix: The refused path now flips the entry back in place (`promote_block_to_memory_tier`, the
convert's inverse) before returning `KeyNotFound`, leaving the entry exactly as it was.

## Bug

`remove`'s location pre-pass (`src/lib.rs:3223`) flips a resident, written-through entry to its
block-device state (`dm.convert_memory_tier_to_block`) to learn which SSD extent it must free,
but when the removal itself is then refused — `dm.remove` answers `ActiveReferences` and the
hold is a reader/client pin/concurrent lookup, not a write-through pin `remove` could take over
— `remove` returns `KeyNotFound` without undoing the flip, leaving the entry in `BlockDevice`
state.

## Impact

A user who calls `remove` on a key while a lookup or a client `pin` holds it gets
`KeyNotFound` — the spec's "nothing was freed" answer — but the entry is silently demoted:
every later lookup of that key is served from the SSD (the slow path) instead of DRAM, and the
DRAM slot stays allocated while being unreachable through the map (only a later, successful
`remove` of the key reclaims slot and extent). The cache behaves as if the entry had been
evicted, although no eviction happened. This is a regression of the 18c4f1f9 repair
(DIS-REMOVE-RESOURCES-REUSABLE), which introduced the flip.

## How to reproduce

1. `cargo test -p dispatcher repair_test_remove_refused_leaves_entry_unchanged` — the unit
   test (client pin holds a written-through entry; `remove` refuses) fails on the unfixed code
   at "a refused remove changed the entry" (`after Some(Block(65536))`).
2. `cd components/dispatcher/verif-loom && RUSTFLAGS="--cfg loom" cargo test --release --test
   loom_props verify_dis_remove_refused_leaves_entry_unchanged -- --exact` — the exhaustive
   interleaving model (concurrent lookup parked in its read section) fails on the unfixed
   mirror the same way.
3. After the fix both pass, and `... verify_dis_remove_refused_leaves_entry_unchanged__mutant
   -- --exact` still fails (the twin that makes `dm.remove` ignore references).

## Fix

`remove` (`components/dispatcher/src/lib.rs:3186`) now records the memory-tier state its
pre-pass flips away (`flipped_memory_tier`, the pointer and size of the first lookup's
`MemoryTier` answer, set only when this call's convert succeeded and the second lookup
confirmed the block-device state). When `dm.remove` refuses with `ActiveReferences` and the
write-through-pin takeover does not apply (`WriteThroughPins::remove_entry` returns `None`),
the entry is restored in place with `dm.promote_block_to_memory_tier(key, pointer, size)` —
the dispatch-map's documented inverse of `convert_memory_tier_to_block`, which preserves every
reference count, the eviction handle and the `ssd_offset` — before returning `KeyNotFound`.
Nothing else changes: the success path, the FR-018 write-through takeover path and the
not-found path are untouched, and an entry this call did not flip (already `BlockDevice`, or
re-promoted concurrently) is left alone.

The loom mirror (`verif-loom/src/dispatcher.rs`, `remove`) was updated line for line and
`verif-loom/mirrored_sources.sha256` refreshed. The creusot model
(`verif-creusot/src/methods.rs`, `remove`) mirrors the fix and re-proves, as do all
remove-related drivers (`verify_dis_remove_*`); their mutants correctly stay unprovable.

Alternatives rejected:

- *Flip only after the remove is known to succeed* — impossible with the current
  `IDispatchMap` API: a `MemoryTier` entry's `ssd_offset` is only observable through the
  flip, and once `dm.remove` succeeds the map entry (and its offset) is gone, so the SSD
  extent would leak — regressing DIS-REMOVE-RESOURCES-REUSABLE (18c4f1f9).
- *Use `try_evict_to_block` instead of `convert`* (it refuses pinned entries) — leaves the
  same bug in the window between the evict and `dm.remove` where a reader pins the entry;
  loom explores exactly that interleaving.
- *Learn the offset from the extent manager after the removal* (scan `get_extents` by key) —
  a larger behavioral change, and the real manager's scan does not see a pending job's
  uncommitted reservation.

## Scope beyond the finding

None in the dispatcher source. Two side effects of re-running the verification tooling,
both within the allowed area: the creusot proof artifacts under
`verif-creusot/verif/dispatcher_verif_rlib/` were regenerated (source-span line updates only;
every non-mutant target that proved before proves again), and `verif-loom` /
`verif-creusot` model files were updated as described above.

## Risk

Low. The new code runs only on the path where `remove` is refused with `KeyNotFound` — a
state no spec sanctioned before, so no conformant behavior changes. The restore uses the
map's in-place promote, which keeps every reference count, so a pinned entry stays pinned and
the holder's pin is untouched (asserted by the unit test). The window between the refused
`dm.remove` and the promote carries the same remove-vs-recreate identity race the pre-pass
already tolerates elsewhere (a racing remove/promote that wins there determines the entry's
state; a refused remove never frees anything in that window). Compatibility: no API or
config change; the only observable difference is that a refused `remove` leaves a MemoryTier
entry MemoryTier, as the spec requires.

## Follow-ups

- Racing removes of one key can leak the SSD extent: if remove B's `convert` fails
  "already block" because remove A flipped the entry, B frees the slot but not the extent
  (pre-existing since 18c4f1f9; a second lookup after a failed convert would learn the
  offset). Worth its own finding.
- The loom test `verify_dis_remove_order` fails on the unmodified tree (expects
  `[MtFree, DmRemove, ExtentFree]`, the code frees in `[DmRemove, MtFree, ExtentFree]`
  order) — needs reconciliation with spec FR-024/FR-049; unrelated to this repair.
- `cargo fmt --check` has pre-existing drift in `src/pipeline.rs`, and `cargo clippy
  -D warnings` fails inside the `interfaces` crate (`manual_checked_ops`, newer clippy than
  the code) — both outside this repair's write scope.
- Creusot drivers `verify_dis_inv_not_initialized`,
  `verify_dis_schedule_wt_failure_keeps_entry` and
  `verify_dis_shutdown_checkpoint_before_devices` fail in the SMT solver exactly as on the
  unmodified tree (confirmed by re-running them against the HEAD model); they do not involve
  `remove`.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED`, callee modules ``.

```
ACCEPT-REPO: base a766e01df0056307b2575e5ffa0ec9c1575bfb2c; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (121 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED yes (Loom)
  E. REVERIFY: dispatcher yes (112 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 121 passed in each run
  dispatcher                   | DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED (Loom) | violated before -> PROVED in Loom's search (all interleavings)); twin fails; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 112 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 53 min | 430 min | accepted |

Total wall-clock: 483 min.
