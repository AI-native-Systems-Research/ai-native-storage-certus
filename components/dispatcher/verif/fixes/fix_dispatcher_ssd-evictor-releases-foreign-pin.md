# fix_dispatcher_ssd-evictor-releases-foreign-pin

> **In short** - Problem: The SSD evictor released a read reference even when its lookup answered "key does not exist" — an answer that takes none — stealing a concurrently stored key's write-through pin, so a later remove freed the key's DRAM slot under a live write job. Fix: The probe's NotExist arm releases nothing; only answers that pinned (BlockDevice / MemoryTier) release.


```
component:      dispatcher
finding:        ssd-evictor-releases-foreign-pin
property:       DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE
found by:       see "How it was found"
base commit:    58e30d457881        branch: repair/evictor-20261024
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
The SSD evictor releases only a read reference that its own lookup acquired: a candidate that answers NotExist took none, so none is released. Releasing after NotExist can drop the write-through pin of a concurrently registered entry; a later remove then frees its slot while a job reads it (spec-sync A-11, 2026-10-23; the same defect class as DIS-REMOVE-NO-FOREIGN-PIN-RELEASE).

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:299`
```
- **FR-018**: The `remove(key)` method does NOT block waiting for background write-through to complete. It proceeds immediately with removal. *(Sync 2026-10-14, branch `fix/dispatcher-remove-clear-mt` — code authoritative, commit `0236a3c98`. Before it, a `remove` that came before the key's write-through had run returned `KeyNotFound` and left the entry cached, because the dispatch map refused the read pin the write-through holds.)* The dispatcher MUST record, per key, the read pin that `copy_gpu_to_memory_completed` leaves on the entry for its background write-through (`WriteThroughPins`). The record MUST be made before the job is enqueued, so the job cannot start first. When `dm.remove` refuses with `ActiveReferences` and that recorded pin is held, `remove` MUST take the pin over: it releases the pin and removes the dispatch-map entry. If another reader still holds the key, the pin is given back and `remove` is refused. Once the entry has been removed this way, ownership is as follows. A job still in the queue is dropped when it is dequeued. A job already reading the DRAM slot finishes, then frees the slot and the extent it has just written, since nothing references either any more. In that case `remove` MUST NOT free the slot itself. A job whose pin was not taken over settles normally (`convert_to_storage`, or `release_read` if the write failed).
- **FR-019**: All block device I/O operations MUST be segmented to respect MDTS (typically 128 KiB). The promotion path uses zero-copy pipelines (`pipelined_multi_object_zero_copy` for the single-entry promotion path, `scatter_gather_multi_drive_zero_copy` for the batch path, and `pipelined_ssd_to_gpu_zero_copy` for the cold-load staging serve) with a configurable `max_queue_depth` parameter. The pipeline maintains up to `max_queue_depth` concurrent in-flight NVMe reads directly into unique offsets of the memory-tier slot (CUDA-pinned + SPDK-registered via FR-035). The multi-object pipeline uses **coalesced per-object H2D**: as each NVMe segment completes, the next NVMe read is resubmitted immediately to keep the device queue saturated (preserving NVMe/GPU overlap), but the GPU H2D copy is deferred until ALL segments for an object complete — then ONE `memcpy_h2d_async` copies the entire contiguous DRAM slot to the GPU. This replaces per-segment `dma_copy_to_device_async` calls (~40 for a 5 MiB object) with a single call per object, reducing GPU kernel launch overhead. Tag-based completion routing (`obj_idx * max_segments + seg_idx`) identifies which segment completed for out-of-order handling. A periodic stream sync every `PIPELINE_RING_SIZE` (32) completed-object H2D copies bounds the CUDA command queue depth. `pipelined_ssd_to_gpu_zero_copy` (per-segment `dma_copy_to_device_async` H2D, `max_queue_depth=16`) is no longer a promotion fallback; it is the read primitive of the cold-load staging serve (`serve_cold_staged`, FR-053). The single-entry cold path `promote_and_serve` uses `pipelined_multi_object_zero_copy` (with a single `ColdReadJob`) at `max_queue_depth=128` to saturate the NVMe submission queue. `batch_lookup`'s cold path instead uses the multi-drive **scatter-gather** pipeline `scatter_gather_multi_drive_zero_copy` (FR-061), which shares the same coalesced per-object H2D machinery but multiplexes all drives' NVMe I/O in one poll loop on the caller thread, distributing a total `max_queue_depth=128` proportionally across drives by segment count; it has no periodic sync — it reaps NVMe continuously and makes one final dual pipe-stream sync *(Sync 2026-09-21 — code authoritative, commit `6f35b13a`)*. Memory-tier SPDK+CUDA registration (FR-034) is required for the zero-copy paths. These pipelines drain every submitted read to completion and do not break early on error (see FR-054).
- **FR-020**: ~~REMOVED~~ *(superseded 2026-07-02)* — The prepare_store/commit_store/cancel_store direct-write workflow has been removed. Data ingestion uses the GPU DMA memory-tier path exclusively.
```

### The code: before (-) and after (+)
```diff
@@ -570,8 +570,10 @@ impl BackgroundEvictor {
                 None
             }
-            Ok(_) => {
-                let _ = dm.release_read(key);
-                None
-            }
+            // NotExist (and MismatchSize, which `lookup` never answers): the
+            // lookup took no read reference — it only pins an entry it finds —
+            // so none is released. Releasing here would drop a reference
+            // another caller holds, e.g. the write-through pin a concurrently
+            // registered entry carries.
+            Ok(_) => None,
             Err(_) => None,
         }
@@ -907,2 +909,6 @@ mod tests {
 #[path = "repair_test_ssd_evictor_release.rs"]
 mod repair_test_ssd_evictor_release;
+
+#[cfg(test)]
+#[path = "repair_test_evictor_no_foreign_pin_release.rs"]
+mod repair_test_evictor_no_foreign_pin_release;
```

### The test that shows the bug: `components/dispatcher/src/repair_test_evictor_no_foreign_pin_release.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `ssd-evictor-releases-foreign-pin`
//! (DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE).
//!
//! The SSD evictor's candidate probe (`get_evictable_offset`) releases a read
//! reference in its catch-all `Ok(_)` arm — the arm that answers
//! `LookupResult::NotExist`. But `lookup` takes a read reference only for an
//! entry it *finds*: a candidate that answers NotExist acquired none, so the
//! release is foreign. In the race where a concurrent `populate` registers the
//! candidate between the probe's lookup and its release — reserving the slot,
//! creating the entry (write ref) and downgrading that ref to the write-through
//! pin (read ref = 1) — the probe's release drops *that* pin. A later `remove`
//! then no longer sees active references and frees the memory-tier slot while
//! the entry's write-through job is still reading it.
//!
//! Self-contained: it drives `BackgroundEvictor::start` through its own minimal
//! mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use interfaces::{
    CacheKey, DispatchMapError, Extent, ExtentKey, ExtentManagerError, FormatParams, IDispatchMap,
    IExtentManager, IMemoryTier, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
    WriteHandle,
};

use crate::background::{BackgroundEvictor, EvictorConfig};
use crate::TierEventCounters;

const KEY: CacheKey = 1;
const BLOCK: u32 = 4096;
const SLOT_ADDR: usize = 0x1000;

// --- memory tier: the set of allocated slots ---

#[derive(Default)]
struct Mt {
    slots: Mutex<HashMap<CacheKey, (usize, u32)>>,
}
... (428 more lines)
```


Traces: dispatcher/src/background.rs:581, dispatcher/src/background.rs:595, dispatcher/src/lib.rs:3784

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem — DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE (finding `ssd-evictor-releases-foreign-pin`)

**One sentence:** The SSD evictor's candidate probe gives back a read reference even when the
dispatch map answered "the key does not exist" — an answer that never took a reference — so the
evictor can silently take away a reference somebody else holds, and a slot can then be freed while
a background write job is still reading it.

**Spec location:** `components/dispatcher/verif/unified_properties.yaml` property
`DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE` (spec-sync proposal A-11, 2026-10-23; the same defect class
as `DIS-REMOVE-NO-FOREIGN-PIN-RELEASE`, which was fixed in `remove`'s pre-pass on 2026-10-15):

> The SSD evictor releases only a read reference that its own lookup acquired: a candidate that
> answers NotExist took none, so none is released.

**Code location:** `components/dispatcher/src/background.rs:572-575` — the catch-all arm of
`BackgroundEvictor::get_evictable_offset`:

```rust
Ok(_) => {
    let _ = dm.release_read(key);   // covers LookupResult::NotExist
    None
}
```

`IDispatchMap::lookup` takes a read reference only for an entry it *finds*: the real dispatch map
returns `Ok(LookupResult::NotExist)` before incrementing `read_ref`
(`components/dispatch-map/src/lib.rs:142-150`). `LookupResult::NotExist` is an `Ok` variant, so
the catch-all `Ok(_)` arm above releases a read reference the evictor never acquired. In the race
where a concurrent `populate` registers the candidate in the window between the evictor's lookup
and its release (`create_memory_tier_entry` leaves `write_ref = 1`, then `downgrade_reference`
converts it to the FR-018 write-through pin `read_ref = 1`), the evictor's release drops that pin.
A later `remove` then no longer sees `ActiveReferences`, so it frees the entry and its memory-tier
slot while the entry's write-through job is still reading the slot.

(The property record's trace lines `background.rs:581/:595` and `lib.rs:3784` predate the
`DIS-REMOVE-REFUSED-LEAVES-ENTRY-UNCHANGED` repair of 2026-10-24/26; on today's tree the release
lives at `background.rs:573` inside `get_evictable_offset`, and `lib.rs:3784` region is the
`pin`/`unpin` client API that owns legitimate read references — no defect there.)

**Verdict: code wrong.** The spec is right (it matches `lookup`'s real reference semantics, and
mirrors the already-repaired `remove` discipline); the catch-all release in
`get_evictable_offset` violates it.

# Repair report — DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE (finding `ssd-evictor-releases-foreign-pin`)

## In short

Problem: The SSD evictor released a read reference even when its lookup answered "key does not
exist" — an answer that takes none — stealing a concurrently stored key's write-through pin, so
a later remove freed the key's DRAM slot under a live write job.
Fix: The probe's NotExist arm releases nothing; only answers that pinned (BlockDevice /
MemoryTier) release.

## Bug

`BackgroundEvictor::get_evictable_offset`'s catch-all `Ok(_)` arm (which answers
`LookupResult::NotExist`) called `dm.release_read(key)` although `IDispatchMap::lookup` takes a
read reference only for an entry it finds — the real dispatch map returns `Ok(NotExist)` before
incrementing `read_ref` (`dispatch-map/src/lib.rs:142-150`) — so the evictor released a reference
it never acquired.

## Impact

In the window between the evictor's probe (answer: NotExist) and its release, a concurrent
`populate` of the same key registers the entry and downgrades its write reference to the FR-018
write-through pin (`read_ref == 1`). The evictor's foreign release drops that pin to zero. A
later `remove` then no longer gets `ActiveReferences`, so it deletes the entry and frees the
memory-tier slot (and the SSD extent) while the entry's background write-through job is still
reading the slot — use-after-free of a DRAM slot, corrupted data written to SSD, and a map that
points at freed resources. This is the same defect class as DIS-REMOVE-NO-FOREIGN-PIN-RELEASE,
which was fixed in `remove`'s pre-pass on 2026-10-15; the evictor's probe had the same catch-all
release.

## How to reproduce

1. `cargo test -p dispatcher --lib repair_test_evictor_no_foreign_pin_release` — on the unfixed
   code it fails with `the evictor released a read reference its own lookup did not acquire`
   (left `Some(0)`, right `Some(1)`); on the fixed code it passes. The test drives the real
   `BackgroundEvictor` thread against a mock dispatch map whose raced `lookup` holds the
   NotExist answer until a concurrent populate has registered the key and left its write-through
   pin, exactly the A-11 race.
2. `cd components/dispatcher/verif-loom && RUSTFLAGS="--cfg loom" cargo test --release
   verify_dis_evictor_no_foreign_pin_release -- --exact` — passes (all interleavings); the
   `__mutant` twin, which re-introduces the foreign release, still fails.

## Fix

`components/dispatcher/src/background.rs` (`get_evictable_offset`): the catch-all arm became
`Ok(_) => None` — no release. `LookupResult::NotExist` and `MismatchSize` (which the map's
`lookup` never answers) are the only answers that reach this arm, and neither acquires a read
reference, so there is nothing of the probe's to release. The BlockDevice and MemoryTier arms are
unchanged: they release exactly the reference their own lookup acquired.

Mirrors updated to follow the fixed code line for line:

- `verif-loom/src/background.rs` — the `NotExist` arm no longer releases; the
  `EvictorReleasesForeign` mutant now performs the foreign release the fix removed (previously
  the mirrored release was the faithful behavior and the mutant added a second one), so the twin
  still catches the regression. `verif-loom/mirrored_sources.sha256` refreshed against the fixed
  sources.
- `verif-creusot/src/bg.rs` — the model's `Ok(_)` arm likewise no longer calls
  `dm_release_read`; the model crate builds cleanly.

Alternatives rejected:

- Releasing only when the release "would" decrement (e.g. probing `entry_size` first): adds a
  TOCTOU window of its own and more map traffic for no benefit — the correct rule is simply "an
  answer that took no reference releases none".
- Making `release_read` tolerant of NotExist at the dispatch-map boundary: the release is still
  foreign whenever the entry appears in the window; the defect is the caller's discipline, and
  the dispatch-map is a separate component outside this repair's scope.
- Enumerating `Ok(LookupResult::NotExist) | Ok(LookupResult::MismatchSize) => None` explicitly:
  same behavior, but the catch-all matches the "nothing else acquires" invariant with less
  ceremony and stays correct if the interface grows another reference-free answer.

## Scope beyond the finding

None. The source change is the one catch-all arm; everything else is the regression test, the
two mirrors of the changed function, and the checksum refresh. No other dispatcher release site
has this defect: every other `release_read` follows a lookup answer that acquired the reference
(`remove`'s pre-pass was repaired on 2026-10-15; `lookup`/`batch_lookup`/`promote` release their
own warm pins).

## Risk

Behaviour change: an evictor probe of a missing candidate no longer touches the map's reference
counts. For any correct client this is unobservable — the released reference was never the
evictor's. The only visible difference is the bug disappearing: entries registered during a
probe keep their write-through pin until their job settles. No API, ordering or persistence
change; the eviction decisions themselves (which keys are evicted) are unchanged, since the arm
still returns `None`. The removed release could previously surface as a spurious
`RefCountUnderflow` error (ignored via `let _`) or, worse, as the stolen pin — both are gone.

## Follow-ups

- `verify_dis_remove_order` fails and `verify_dis_flush_reports_failed_write__mutant` aborts
  during cleanup in the loom suite on the pristine HEAD tree as well (checked by running both
  against the unmodified mirror) — pre-existing, unrelated to this property, but worth a repair
  of their own.
- `cargo clippy -p dispatcher` currently fails in the `interfaces` crate (pre-existing, outside
  this component's write scope); `cargo fmt --check` has pre-existing diffs in
  `dispatcher/src/pipeline.rs`.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE`, callee modules ``.

```
ACCEPT-REPO: base 58e30d45788155046194bf8c25fb91415eb6281f; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (122 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE yes (Loom)
  E. REVERIFY: dispatcher yes (112 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 122 passed in each run
  dispatcher                   | DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE (Loom) | violated before -> PROVED in Loom's search (all interleavings)); twin fails; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 112 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 22 min | 439 min | accepted |

Total wall-clock: 461 min.
