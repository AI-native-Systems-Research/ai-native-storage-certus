# fix_dispatcher_remove-frees-ssd-extent

> **In short** - Problem: `remove`'s lookup pre-pass blindly released a read reference on `NotExist` (dropping a foreign pin) and freed extents only for `BlockDevice` answers, leaking a written-through entry's SSD extent. Fix: release only self-acquired pins; flip a written-through resident entry to block state (`convert_memory_tier_to_block`) to learn its extent, then free it.


```
component:      dispatcher
finding:        remove-frees-ssd-extent
property:       DIS-REMOVE-RESOURCES-REUSABLE,DIS-REMOVE-NO-FOREIGN-PIN-RELEASE
found by:       see "How it was found"
base commit:    85fe77b5f810        branch: repair/dispatcher-remove
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
- DIS-REMOVE-RESOURCES-REUSABLE: After remove succeeds, the memory-tier bytes and SSD extent the entry used are available for reuse by later entries.
- DIS-REMOVE-NO-FOREIGN-PIN-RELEASE: Remove only releases a read reference that its own lookup acquired, so removing a key never decrements a read reference held by another caller.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:392`
```
- **SC-003**: Cache removal frees all associated resources (memory-tier slots and SSD extents) so that they can be reused.
- **SC-004**: The dispatcher correctly handles concurrent populate and lookup operations on different keys without data corruption or deadlock.
```

### The code: before (-) and after (+)
```diff
@@ -3193,4 +3193,10 @@ impl IDispatcher for DispatcherComponent {
 
         // Lookup the entry to determine its location (waits for any active writer).
+        // Only a found entry carries this lookup's read reference: `NotExist`
+        // (and `MismatchSize`, which the map never answers from `lookup`)
+        // acquires none, so none may be released either — a blind release
+        // there would decrement a reference held by another caller, e.g. the
+        // write-through pin a concurrent populate leaves on the entry it
+        // registers in the window between this lookup and the release.
         let block_offset = match dm.lookup(key) {
             Ok(LookupResult::BlockDevice { offset }) => {
@@ -3198,8 +3204,43 @@ impl IDispatcher for DispatcherComponent {
                 Some(offset)
             }
-            Ok(_) => {
+            Ok(LookupResult::MemoryTier { .. }) => {
+                // A resident entry may already carry its write-through SSD
+                // copy (`ssd_offset`), which this lookup result does not
+                // expose. Flip the entry to its block-device state — the
+                // transition that records that offset — and look it up once
+                // more to learn which extent to free. Hold this lookup's read
+                // reference across the flip: the memory-tier evictor's
+                // demotion (`try_evict_to_block`) refuses a pinned entry, so
+                // the state cannot change underneath us. `InvalidState` means
+                // no SSD copy exists yet (write-through not landed): nothing
+                // to free — a pending job's extent is freed by the takeover
+                // below or by the job itself.
+                let converted = dm.convert_memory_tier_to_block(key).is_ok();
                 let _ = dm.release_read(key);
-                None
+                if converted {
+                    match dm.lookup(key) {
+                        Ok(LookupResult::BlockDevice { offset }) => {
+                            let _ = dm.release_read(key);
+                            Some(offset)
+                        }
+                        // The entry vanished (a concurrent remove won the
+                        // race) or was re-promoted in place (its extent stays
+                        // the live entry's): the map removal below settles
+                        // the outcome either way.
+                        Ok(LookupResult::MemoryTier { .. }) => {
+                            let _ = dm.release_read(key);
+                            None
+                        }
+                        // NotExist takes no read reference: nothing to release.
+                        Ok(_) => None,
+                        Err(_) => None,
+                    }
+                } else {
+                    None
+                }
             }
+            // NotExist / MismatchSize: this lookup took no read reference, so
+            // there is none of this caller's to release.
+            Ok(_) => None,
             Err(_) => return Err(DispatcherError::KeyNotFound(key)),
         };
@@ -7356,2 +7397,6 @@ mod repair_test_populate_slot;
 #[path = "repair_test_remove_no_wait.rs"]
 mod repair_test_remove_no_wait;
... (4 more lines in the commit)
```

### The test that shows the bug: `components/dispatcher/src/repair_test_remove_frees_ssd_extent.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `remove-frees-ssd-extent`
//! (DIS-REMOVE-RESOURCES-REUSABLE, DIS-REMOVE-NO-FOREIGN-PIN-RELEASE).
//!
//! Two defects of `remove`'s location pre-pass, one root cause — it treats
//! `dm.lookup`'s answer as "a read pin I own plus the entry's full location",
//! which it is not:
//!
//! * RESOURCES-REUSABLE: a resident entry that already carries its
//!   write-through copy (`MemoryTier { ssd_offset: Some(_) }` — the state
//!   after `flush_to_ssd`, or after a promote from SSD) answers
//!   `MemoryTier`, which hides the offset, so `remove` never frees the SSD
//!   extent. Removing and re-storing keys steadily consumes SSD capacity.
//! * NO-FOREIGN-PIN-RELEASE: the `Ok(_)` arm of the pre-pass releases a read
//!   reference even when the lookup answered `NotExist`, which acquires
//!   none — so the release can decrement a reference another caller holds.
//!
//! Self-contained (its own mocks, the public `IDispatcher` API only), so it
//! builds against the dispatcher as it was before the repair as well as
//! after it.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
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
const EXTENT: u64 = 16 * BLOCK as u64;
... (999 more lines)
```


Traces: SC-003, dispatcher/src/lib.rs:3110, dispatcher/src/lib.rs:2885

## How it was found
- creusot: DIS-REMOVE-RESOURCES-REUSABLE: unscored; DIS-REMOVE-NO-FOREIGN-PIN-RELEASE: refuted -  | REFUTED — the negation 'refute_dis_remove_no_foreign_pin_release' is machine-proved, so the code violates this obligation. This is a finding, not a gap: see the spec and code locations on the property
- kani: DIS-REMOVE-RESOURCES-REUSABLE: unscored; DIS-REMOVE-NO-FOREIGN-PIN-RELEASE: unscored -  | 

## The problem (agent's statement and verdict)
# Problem — finding `remove-frees-ssd-extent`

## One sentence

When `remove` deletes a cache entry it leaks the SSD extent of an entry that is
resident in DRAM but already written through, and it can also drop a read
reference held by another caller when its own lookup found nothing.

## Spec vs code

- **Spec**: `specs/001-dispatcher-cache-interface/spec.md:392` — SC-003:
  "Cache removal frees all associated resources (memory-tier slots and SSD
  extents) so that they can be reused." (Properties
  DIS-REMOVE-RESOURCES-REUSABLE and DIS-REMOVE-NO-FOREIGN-PIN-RELEASE.)
- **Code**: `components/dispatcher/src/lib.rs:3195` (in `remove`): the
  pre-removal `dm.lookup` match releases a read reference in *every* `Ok(_)`
  arm — including `NotExist`, which acquires none, so the release can
  decrement a reference another caller holds (the write-through pin a
  concurrent `populate` just left on the entry it registered between the
  lookup and the release). Creusot machine-refuted this obligation
  (`refute_dis_remove_no_foreign_pin_release`).
- **Code**: `components/dispatcher/src/lib.rs:3226` (in `remove`): the SSD
  extent is freed only when the lookup answered `BlockDevice { offset }`. A
  resident entry that already carries its write-through copy
  (`MemoryTier { ssd_offset: Some(off) }`) — the normal state after
  `flush_to_ssd`, or after a promote from SSD — answers `MemoryTier`, so
  `block_offset` stays `None` and the extent at `off` is never returned to
  the extent manager. Removing and re-storing keys therefore steadily
  consumes SSD capacity.

Both defects share one root cause: `remove`'s location pre-pass treats
`dm.lookup`'s answer as "a pin I own plus the entry's full location", when it
is neither — `NotExist`/`MismatchSize` take no pin, and the `MemoryTier`
answer hides the persisted `ssd_offset`.

## Verdict

**Code wrong.** The spec (SC-003) is what the component promises; the code
disagrees with it in `remove`.

# Repair report: `remove-frees-ssd-extent`

## In short

Problem: `remove`'s lookup pre-pass blindly released a read reference on `NotExist` (dropping a foreign pin) and freed extents only for `BlockDevice` answers, leaking a written-through entry's SSD extent.
Fix: release only self-acquired pins; flip a written-through resident entry to block state (`convert_memory_tier_to_block`) to learn its extent, then free it.

## Bug

One root cause in `DispatcherComponent::remove` (`components/dispatcher/src/lib.rs:3186`, pre-fix line numbers 3105-3113): the pre-pass that locates the entry before deleting it treated the dispatch-map `lookup` answer as "a read reference I own plus the entry's full location". Neither half holds:

- **DIS-REMOVE-NO-FOREIGN-PIN-RELEASE** (refuted by Creusot): `lookup` takes a read reference only for a *found* entry; `NotExist` (and `MismatchSize`, which `lookup` never answers) takes none. The old `Ok(_) => { let _ = dm.release_read(key); .. }` arm covered `NotExist` and released anyway. A concurrent `populate` registers its entry with the FR-018 write-through pin in exactly the window between this lookup and the release, so `remove` decremented *that* caller's pin.
- **DIS-REMOVE-RESOURCES-REUSABLE** (SC-003, `specs/001-dispatcher-cache-interface/spec.md:392`): a resident `MemoryTier` entry may already carry its persisted write-through copy (`ssd_offset: Some(off)`), but `LookupResult::MemoryTier` does not expose it. Only the `BlockDevice` arm produced an offset to free, so removing a written-through entry freed its DRAM slot and never called `remove_extent` — the SSD extent leaked permanently.

Verdict recorded in `problem.md`: **code wrong** (the Creusot refutation `refute_dis_remove_no_foreign_pin_release` proved the pin property false on the old mirror).

## Impact

- Silent refcount corruption: any caller's read pin (write-through pin of a concurrent populate, an explicit `pin()`, an in-flight cold lookup's promotion pin) could be decremented without its knowledge, enabling premature demotion/eviction of an entry somebody was actively reading (the memory-tier evictor's `try_evict_to_block` refusal relies on that count) — a use-after-free-class hazard for the DRAM slot.
- Permanent capacity leak: every remove of a successfully written-through entry leaked one SSD extent; a long-running cache steadily loses extent-manager space until allocation starts failing.

## How to reproduce

Unit tests (fail on the unfixed code by assertion, pass on the fix), in `components/dispatcher/src/repair_test_remove_frees_ssd_extent.rs`:

```bash
cargo test -p dispatcher --lib repair_test_remove_frees_ssd_extent
```

- `remove_frees_ssd_extent_of_written_through_entry`: real background write-through of key 1 (`store` + `wait_persisted`, precondition asserts `Loc::Memory{ssd: Some(EXTENT)}`); after `remove(1)`, `em.used_extents()` is still 1 on the unfixed code (`left: 1, right: 0`); a subsequent `store(2)` must reuse slot and the same `EXTENT` offset.
- `remove_never_releases_a_foreign_read_reference`: deterministic registration race — a mock dispatch-map lookup of the absent key blocks until a second thread has registered and pinned it, then answers `NotExist`; on the unfixed code `remove` still returns `Ok(())` while the mock's `foreign_releases()` count is 1 (`left: 1, right: 0`) and the entry's `read_refs` drops from 2 to 1.

Loom (all interleavings; PASS on the fix, `__mutant` twins FAIL), from `components/dispatcher/verif-loom`:

```bash
RUSTFLAGS="--cfg loom" cargo test --release verify_dis_remove_resources_reusable -- --exact
RUSTFLAGS="--cfg loom" cargo test --release verify_dis_remove_resources_reusable__mutant -- --exact
RUSTFLAGS="--cfg loom" cargo test --release verify_dis_remove_no_foreign_pin_release -- --exact
RUSTFLAGS="--cfg loom" cargo test --release verify_dis_remove_no_foreign_pin_release__mutant -- --exact
```

Creusot (from `components/dispatcher/verif-creusot`, `PATH="$HOME/.local/share/creusot/bin:$PATH"`): `cargo creusot remove`, `cargo creusot verify_dis_remove_no_foreign_pin_release[_mutant]`, `cargo creusot verify_dis_remove_resources_reusable[_mutant]` — the two `verify_*` prove, the two `__mutant` fail as required.

## Fix

`components/dispatcher/src/lib.rs:3201-3246` — the pre-pass now handles each `lookup` answer according to what it actually carries:

- `BlockDevice { offset }`: release the pin this lookup took; remember `offset` (unchanged).
- `MemoryTier { .. }`: the answer hides a possible persisted `ssd_offset`. Call `dm.convert_memory_tier_to_block(key)` — the dispatch-map transition that records that offset by flipping the entry to its block-device state, keeping every reference count — then release the lookup's pin, and if the flip succeeded, look the entry up once more to learn the extent. Holding the lookup's read pin across the flip is race-safe: the memory-tier evictor's `try_evict_to_block` refuses a pinned entry, so the state cannot change underneath; the second lookup's `MemoryTier` (re-promoted in place) / `NotExist` (a concurrent remove won) / error outcomes carry no pin of ours and fall through to the `dm.remove` below, which settles the outcome. `InvalidState` from the flip means no SSD copy yet: nothing to free — a pending job's extent is freed by the FR-018 takeover (`write_through_pins.remove_entry`) or by the running job itself (`finish(owned = false)`, lib.rs:1436-1442). The two extent frees are mutually exclusive: the job frees its extent only when its write had not settled (`ssd_offset` unset — exactly when our flip returns `InvalidState`).
- `NotExist` / `MismatchSize`: this lookup took no read reference, so none is released.

`Err` from the lookup still returns `KeyNotFound` (unchanged). The rest of `remove` (map removal, FR-018 takeover, slot free, extent free) is unchanged.

**Rejected alternatives:**

- *Expose `ssd_offset` in `LookupResult::MemoryTier`*: requires changing `interfaces/` and `dispatch-map` — outside this repair's write scope, and widens a public interface for one caller.
- *New `IDispatchMap` method (e.g. `ssd_offset(key)`)*: same scope problem, same new-API cost; `convert_memory_tier_to_block` already exists and expresses exactly the needed transition.
- *Track the offset in dispatcher-side state at write-through time*: duplicates dispatch-map state and is racy against promotion/demotion.
- *Reorder `mt.remove` before `dm.remove`*: unrelated to the root cause and contradicts the deliberate map-first safety order (prevents new readers from obtaining a freed pointer); the spec/code disagreement there is the separate DIS-REMOVE-ORDER divergence.
- *Also fix the identical blind-release shape in `BackgroundEvictor::get_evictable_offset` (background.rs:572-575) and the `lookup_async` mismatch arm*: same bug pattern but different functions, outside this finding; recorded as follow-up.

## Scope beyond the finding

No production code beyond `remove` changed. The rest is test wiring and verification mirrors, as required by the check:

- `src/lib.rs`: `#[cfg(test)] #[path = ...] mod repair_test_remove_frees_ssd_extent;` wiring; `src/repair_test_remove_frees_ssd_extent.rs` (new).
- `verif-loom/src/dispatcher.rs`: `remove` mirror rewritten line-for-line after the fix (Mutation `RemoveDoubleRelease` still injects its extra release after every acquired one); `verif-loom/src/dmap.rs`: new mirror of `convert_memory_tier_to_block` (dispatch-map lib.rs:441); `verif-loom/mirrored_sources.sha256` refreshed (src/lib.rs `c34573ea…`).
- `verif-creusot/src/boundary.rs`: new trusted boundary `dm_convert_memory_tier_to_block`; `src/methods.rs`: `remove` mirror updated to the fixed pre-pass, old `NotExist`-releases-own-0 clause removed, `own_reads` now unconditional, plus a new clause (an unreferenced entry's remove always succeeds) needed by the drivers; `src/drivers.rs`: `refute_dis_remove_no_foreign_pin_release` replaced by `verify_dis_remove_no_foreign_pin_release(+__mutant)` and new `verify_dis_remove_resources_reusable(+__mutant)`; `tools/gen_drivers.py` kept in sync (not run — it also rewrites the out-of-scope advisory yaml).

## Risk

- *Failed remove leaves a state change*: if `dm.remove` refuses after our flip (a reader holds the entry), `remove` returns `KeyNotFound` but the entry is now in block-device state with its memory-tier slot still present. The data is identical (the write-through had landed), lookups then serve from SSD, and the entry is reclaimable: a later `remove` works, and the SSD evictor's `get_evictable_offset` answers `Some` for block-device entries, freeing slot and extent once unreferenced. All loom interleavings of the pin race are covered by `verify_dis_remove_no_foreign_pin_release` (PASS).
- *Latency*: one extra dispatch-map roundtrip (flip + second lookup) on the memory-tier remove path; no new locks or waits (the flip never blocks on references).
- *Double free*: excluded by the `ssd_offset` flag as described above; the loom twins (`RemoveDoubleRelease`, `RemoveSkipsExtentFree`) still fail on their injected mutations.
- The loom `__mutant` twins abort with SIGABRT in loom 0.7's post-assertion cleanup — identical behavior on the pristine HEAD mirror (pre-existing, not introduced here); the intended assertion fires first in both.
- Crate tests pass 5 consecutive runs (`cargo test -p dispatcher`: 101 unit, 5 lazy_migration, 14 reserve_memory); `cargo fmt`/`clippy` clean for the touched files.

## Follow-ups

1. `BackgroundEvictor::get_evictable_offset` (background.rs:572-575) releases a read reference in its `Ok(_)` arm after a `NotExist` answer — the same foreign-pin drop; needs the same fix (new finding candidate).
2. The `lookup_async` mismatch arm carries the same blind-release shape (verify and fix).
3. DIS-REMOVE-ORDER remains divergent (spec: slot freed first; code: map entry deleted first — discordances.yaml "Remove order", candidate). `verify_dis_remove_order` failed at HEAD and still fails, now only on the `MtFree`/`DmRemove` ordering (the missing `ExtentFree` half is fixed by this repair); needs a design decision, not a mechanical fix.
4. Creusot `verify_dis_inv_not_initialized` fails identically at HEAD (21/23 goals) — pre-existing, unrelated to remove.
5. Re-populating an existing written-through key appears to leak the old extent (new reserve before the old entry's extent is freed) — worth a dedicated finding.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-REMOVE-RESOURCES-REUSABLE,DIS-REMOVE-NO-FOREIGN-PIN-RELEASE`, callee modules ``.

```
ACCEPT-REPO: base 85fe77b5f8101152f174c30947166cba15537c26; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (120 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-REMOVE-RESOURCES-REUSABLE yes (Loom)
  C. TARGET   : dispatcher DIS-REMOVE-NO-FOREIGN-PIN-RELEASE yes (Loom)
  E. REVERIFY: dispatcher yes (112 proved after the repair; 1 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 120 passed in each run
  dispatcher                   | DIS-REMOVE-RESOURCES-REUSABLE (Loom)     | violated before -> PROVED in Loom's search (all interleavings)); twin fails; new test fails on the old code, passes on the new
  dispatcher                   | DIS-REMOVE-NO-FOREIGN-PIN-RELEASE (Loom) | violated before -> PROVED in Loom's search (all interleavings)); twin fails; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 112 proved, 1 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 158 min | 436 min | accepted |

Total wall-clock: 594 min.
