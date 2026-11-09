# fix_dispatcher_clear-mt-frees-pinned-slot

> **In short** - Problem: `clear_memory_tier` freed the DRAM slot of an entry a lookup or the SSD writer was still reading, so the cache pointed at freed memory and readers could get corrupt data. Fix: a pinned entry keeps its slot and is retried until the pin drops; only really cleared entries are counted.


```
component:      dispatcher
finding:        clear-mt-frees-pinned-slot
property:       DIS-SPIN-CLEAR-MT-KEEPS-PINNED
found by:       see "How it was found"
base commit:    01d004c70916        branch: repair/dispatcher-clear-mt
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
When clear_memory_tier meets an entry that it cannot demote or remove because a lookup or the background writer holds a pin on it, it does not release that entry's memory-tier slot: no dispatch-map entry ever points at a released slot, no reader reads a released slot, and when clear_memory_tier returns every entry is BlockDevice with an SSD offset or is removed, and the returned count equals the entries that it really cleared.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:317`
```
- **FR-038**: System MUST provide a `clear_memory_tier() -> Result<usize, DispatcherError>` method on the `IDispatcher` interface. This method evicts ALL entries from the memory-tier pool by calling `IMemoryTier::evict_next()` in a loop until empty. For each evicted key, it transitions the dispatch-map entry to BlockDevice state via `convert_memory_tier_to_block`. If the transition fails (write-through not complete), the entry is removed from the dispatch map entirely. Returns the number of entries cleared. The `IMemoryTier` trait MUST also provide a `clear() -> Result<usize, MemoryTierError>` method that resets the pool (clears slots, eviction-order state, and allocator) in a single operation.
- **FR-039**: System MUST provide a `batch_lookup(entries: &[(CacheKey, Vec<IpcHandle>)]) -> Vec<Result<(), DispatcherError>>` method on the `IDispatcher` interface. *(Sync 2026-08-07 — signature backfilled: each entry now carries a `Vec<IpcHandle>` of destination regions, not a single `IpcHandle`, so one cache key can be scattered to multiple GPU destinations (e.g. sibling tensor-parallel ranks) in a single batched operation.)* This method processes a batch of lookup entries concurrently: (1) classifies all entries by dispatch-map state — a `dm.lookup` that returns `Err` is a transient timeout waiting for a `write_ref` (a concurrent store-commit / promote) to clear, NOT a missing key (dispatch-map entries survive demotion via an in-place flip to BlockDevice), so it is retried locally a few times (up to 5) rather than collapsed into `KeyNotFound`; collapsing it would forward a live key to remote-lookup, whose miss becomes a fatal load failure in the vLLM connector (`EngineDeadError`) *(Sync 2026-09-02 — code authoritative, commit `1d55b9c2`; this retry is defensive and does not fire under the current workload, where `write_ref` windows are sub-millisecond)*, (2) for MemoryTier hits, issues per-region `memcpy_h2d_async` H2D copies on the device's `warm` stream (FR-037), holding a dispatch-map read pin on each hit; the copies are only **queued** here. The warm hits' LRU refresh (`batch_touch`) still runs before cold prep so its evictor sees the same recency order, but the single batched warm `stream_synchronize` is **deferred until after the cold block** (steps 4-5), so the warm H2D drain overlaps the cold NVMe reads. Only after that sync are the warm read pins released — releasing at submission would let the memory-tier evictor reclaim a DRAM slot from under an in-flight DMA — and only then does the remote lookup for local misses (FR-045) run. A multi-region cold entry's `serve_memory_tier_to_gpu` sync on the same warm stream also waits for the still-queued warm copies, which is correct *(Sync 2026-10-08 — code authoritative, commit `ada87967`: the warm sync previously ran immediately after the classify loop, before cold prep)*, (3) cold (BlockDevice) entries are NOT special-cased by count — a lone single-key cold load goes through the same pooled path as a multi-entry batch (steps 4-5); the earlier single-entry inline `promote_and_serve` fast path was removed *(Sync 2026-09-02, commit `1d55b9c2`)* because it had no staging fallback (FR-053) and, under memory-tier pressure, turned a survivable cold miss into a fatal load failure — the pooled path defers tier saturation (`AllocationFailed`) to the staging post-pass so a cold load degrades to an uncached serve instead of failing; (4) groups BlockDevice (cold) entries by target drive (`drive_index(key, num_drives)` = a splitmix64 hash of the key mod `num_drives`, which spreads sequential keys uniformly across drives), and for each drive runs a per-entry `evict_and_insert` cold-prep that allocates the memory-tier slot and builds a `pipeline::ColdReadJob` (deferring entries that hit `AllocationFailed` to the staging post-pass, FR-053), (5) checks out **one** NVMe client channel per active drive (an RAII `ChannelLease` from that drive's `ChannelPool`, FR-034) and issues a **single** `scatter_gather_multi_drive_zero_copy` (FR-061) on the **caller thread** — one multiplexed poll loop that fans reads out to all drives and fans completions in, distributing a total `max_queue_depth = 128` proportionally across drives by segment count and issuing each object's coalesced H2D on the per-device pipeline stream pair (FR-052) the moment its last NVMe segment lands *(Sync 2026-09-21 — code authoritative, commit `6f35b13a`: this replaced the earlier model that spawned up to `MAX_QUEUES_PER_DRIVE` per-drive threads each running `pipelined_multi_object_zero_copy`; there are no longer any spawned cold-path threads and no `ColdReadPool` — see FR-044, retired)*, (6) merges results back in input order. Returns one `Result` per input entry.
```

### The code: before (-) and after (+)
```diff
@@ -3872,19 +3872,52 @@ impl IDispatcher for DispatcherComponent {
             .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;
 
+        // A pinned entry (a lookup or the write-through holds a reference) can be
+        // neither demoted nor removed, and its slot must NOT be freed: the entry
+        // still points at it and the pin holder may be copying from it. Such an
+        // entry keeps its slot and is retried until the pin drops. The wait is
+        // bounded so a pin that is never released cannot hang the call: at the
+        // deadline the still-pinned entries stay resident and the call fails with
+        // `Timeout`, so `Ok(count)` always means the memory tier was emptied.
+        let timeout = std::time::Duration::from_secs(5);
+        let start = std::time::Instant::now();
         let mut count = 0;
+        let mut pinned = 0;
         loop {
-            let candidates = mt.oldest_keys(64);
+            // Widen the window past the entries left pinned on the last pass, so
+            // they cannot hide the clearable ones behind them.
+            let candidates = mt.oldest_keys(64 + pinned);
             if candidates.is_empty() {
                 break;
             }
+            let mut cleared = 0;
+            pinned = 0;
             for key in candidates {
-                if dm.try_evict_to_block(key).is_ok() {
+                let done = match dm.try_evict_to_block(key) {
+                    Ok(()) => true,
+                    Err(interfaces::DispatchMapError::ActiveReferences(_)) => false,
+                    // Entry not evictable (no SSD copy) — force remove. The entry
+                    // goes first: a pin taken since the demotion attempt refuses
+                    // the remove, and the slot is then kept for the retry.
+                    Err(_) => !matches!(
+                        dm.remove(key),
+                        Err(interfaces::DispatchMapError::ActiveReferences(_))
+                    ),
+                };
+                if done {
                     let _ = mt.remove(key);
+                    count += 1;
+                    cleared += 1;
                 } else {
-                    // Entry not evictable (no SSD copy) — force remove.
-                    let _ = mt.remove(key);
-                    let _ = dm.remove(key);
+                    pinned += 1;
                 }
-                count += 1;
+            }
+            if cleared == 0 {
+                if start.elapsed() >= timeout {
+                    return Err(DispatcherError::Timeout(format!(
+                        "clear_memory_tier: {pinned} pinned entries still resident \
+                         after clearing {count}"
+                    )));
+                }
+                std::thread::sleep(std::time::Duration::from_micros(50));
             }
         }
@@ -3972,4 +4005,8 @@ impl IDispatcher for DispatcherComponent {
... (8 more lines in the commit)
```

### The test that shows the bug: `components/dispatcher/src/repair_test_clear_mt_pinned.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `clear-mt-frees-pinned-slot`
//! (DIS-SPIN-CLEAR-MT-KEEPS-PINNED).
//!
//! `clear_memory_tier` must not release the memory-tier slot of an entry that a
//! reader (a lookup, or the background writer's write-through pin) still holds:
//! no dispatch-map entry may ever point at a released slot. When the call
//! returns, every entry it reports as cleared is BlockDevice or removed, and
//! the count is the number of entries really cleared.
//!
//! The memory-tier mock checks the invariant at the moment a slot is freed: it
//! records a violation if the dispatch map still has a `MemoryTier` entry
//! pointing at that slot. Each scenario pins the key before the clear and
//! releases the pin from another thread a little later, as a lookup or the
//! writer would.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

// --- dispatch map: entries with reference counts, as the real one enforces them ---

#[derive(Clone, Copy, Debug, PartialEq)]
enum Loc {
    Block(u64),
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory {
        ptr: usize,
... (616 more lines)
```


Traces: Spin J22 clear_memory_tier PA, PA1, PA2, PB, PD (node7, reproduced on green 2026-10-19), components/dispatcher/src/lib.rs clear_memory_tier (unstable af221dd82: the else branch frees the slot after try_evict_to_block is refused), dispatcher spec FR-038; interfaces idispatcher.rs clear_memory_tier doc

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem: clear-mt-frees-pinned-slot (DIS-SPIN-CLEAR-MT-KEEPS-PINNED)

When `clear_memory_tier` meets a cache entry that a reader (a lookup or the
background SSD writer) is still using, it frees the entry's DRAM slot anyway,
so the cache keeps pointing at memory that has been handed back and may be
reused, and the call still counts that entry as cleared.

## Where spec and code disagree

- **Spec**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:317`
  (FR-038): each entry is demoted to BlockDevice; if the demotion fails it is
  removed from the dispatch map entirely; the return value is the number of
  entries cleared. Also `components/interfaces/src/idispatcher.rs:806-822` (the
  `clear_memory_tier` doc). Neither allows a slot to be freed while a
  dispatch-map entry still points at it. The property (Spin J22) says this
  directly: a pinned entry keeps its slot, and on return every entry is
  BlockDevice or removed, and the count matches.
- **Code**: `components/dispatcher/src/lib.rs:3881-3888`. When
  `dm.try_evict_to_block(key)` is refused, the `else` branch calls
  `mt.remove(key)` (frees the slot) **before** `dm.remove(key)`, and ignores the
  result of `dm.remove`. When the refusal is caused by a pin
  (`ActiveReferences`), `dm.remove` is refused for the same reason. The entry
  stays `MemoryTier { pointer }` and points at a released slot. The lookup or
  writer that holds the pin then reads freed memory, and `count += 1` counts the
  entry as cleared anyway.

## Verdict

**Code wrong.** The spec's "remove it if it cannot be demoted" can only be met
when the entry is not pinned. The code frees the slot without checking that the
removal worked. That breaks the dispatcher's own rule that a pinned slot is
never reclaimed, which `evict_one_clean` (lib.rs:1055) and `remove` already
follow.

# Repair report: clear-mt-frees-pinned-slot (DIS-SPIN-CLEAR-MT-KEEPS-PINNED)

## In short

Problem: `clear_memory_tier` freed the DRAM slot of an entry a lookup or the SSD writer was still reading, so the cache pointed at freed memory and readers could get corrupt data.
Fix: a pinned entry keeps its slot and is retried until the pin drops; only really cleared entries are counted.

## Bug

When `try_evict_to_block` refused an entry because a reader held a pin on it, `clear_memory_tier` freed the entry's memory-tier slot anyway (`components/dispatcher/src/lib.rs:3881-3888` on af221dd82). The `dm.remove` that followed was refused for the same reason. That left a `MemoryTier` dispatch-map entry pointing at a released slot, and the entry was still counted as cleared.

## Impact

- A lookup that overlaps `clear_memory_tier` can copy from a slot that has been freed and may already hold another key's data. The GPU then receives wrong KV-cache bytes, or the DMA fails.
- The background writer can write freed (possibly reused) DRAM to the SSD under the original key. A later cold load of that key then returns another key's data.
- After the call the entry still says `MemoryTier` with a dangling pointer, so later lookups of that key keep reading freed memory.
- The returned count was too high. Callers such as `remote-lookup-bench demote` read the count as "entries cleared".

## How to reproduce

1. `git checkout af221dd82 -- components/dispatcher/src/lib.rs`, then add `repair_test_clear_mt_pinned.rs` and its `mod` line to `src/lib.rs`. The test uses only APIs that exist there.
2. `cargo test -p dispatcher --lib repair_test_clear_mt_pinned`. All three tests fail with "clear_memory_tier released a memory-tier slot that a dispatch-map entry still points at".
3. Loom: `cd components/dispatcher/verif-loom && RUSTFLAGS="--cfg loom" cargo test --release verify_dis_spin_clear_mt_keeps_pinned -- --exact`. It fails on the old mirror and passes on the fixed one.

## Fix

`clear_memory_tier` (`src/lib.rs:3861`) now handles each candidate this way:

- **Demoted** (`try_evict_to_block` Ok): free the slot and count it.
- **Pinned** (`ActiveReferences`): leave both the slot and the entry alone. The key stays in the memory tier and is tried again on the next pass.
- **Not persisted** (any other refusal): remove the dispatch-map entry first. Free the slot only if that removal was not refused by a pin. A pin taken between the two calls makes the entry count as pinned.

Other details:

- The candidate window grows by the number of entries left pinned. Pinned keys at the head of the LRU therefore cannot hide clearable keys behind them.
- When a whole pass clears nothing, the loop sleeps for 50 µs before trying again. This is the same pattern as `serve_concurrently_promoted`.
- The wait is bounded at 5 s. If a pin is never released, the call returns `DispatcherError::Timeout` with the pinned entries still resident and intact. So `Ok(n)` always means "memory tier emptied, n entries really cleared".

Alternatives I rejected:

- **Skip pinned entries and return at once.** This leaves entries in `MemoryTier` when the call returns, which breaks "every entry is BlockDevice or removed on return". Callers (benchmarks, `cmd_demote`) rely on the tier being empty afterwards.
- **Free the slot but leave the entry.** That is the bug.
- **Block in the dispatch map until the references drain.** This needs a new `IDispatchMap` API in another component and blocks while the map lock is held. It is also the `RemoveWaitsForRefs` design that the remove-no-wait property forbids.
- **Return `Ok` at the timeout with a smaller count.** This would hide from the caller that the tier is not empty.

Verification:

- **New unit test** `src/repair_test_clear_mt_pinned.rs`: three cases (lookup pin, writer pin then persisted, pin on an unpersisted entry). It fails on the old code and passes on the fixed code.
- **`cargo test -p dispatcher`**: 5 of 5 runs green (95 lib tests plus the integration suites).
- **Loom**: the mirror of `clear_memory_tier` in `verif-loom/src/dispatcher.rs` follows the fix line for line; the 50 µs sleep becomes `yield_now`, and the timeout is abstracted as for the 5 s promotion timeout. `mirrored_sources.sha256` is refreshed, and the line references for `flush_to_ssd` and the mock memory tier are updated.
  - `verify_dis_spin_clear_mt_keeps_pinned`: PASS.
  - `verify_dis_spin_clear_mt_keeps_pinned__mutant`: FAIL (as required).
  - `verify_dis_flush_then_clear_demotes`: PASS.
- **Creusot**: the model of `clear_memory_tier` in `verif-creusot/src/misc.rs` is updated to the new loop. Its postcondition now also allows `Err(Timeout)`; `Ok` still requires an empty memory tier. A new invariant and postcondition say that a pinned resident entry keeps its slot.
  - Proved: `clear_memory_tier`, `verify_dis_clear_mt_empties`, `refute_dis_clear_mt_demotes_persisted`, `refute_dis_clear_mt_removes_unpersisted`, and the new `verify_dis_clear_mt_pin_safe`.
  - Do not prove, as expected: `verify_dis_clear_mt_empties__mutant` and the new `verify_dis_clear_mt_pin_safe__mutant`.
  - No longer proves: `refute_dis_clear_mt_pin_safe`, the earlier machine proof that the code broke DIS-CLEAR-MT-PIN-SAFE. That is the expected result now that the bug is fixed.

### Second acceptance run: "3 properties lost"

The first acceptance run rejected the repair on leg E ("107 proved, 3 lost"). Comparing `/tmp/repair_gate_dispatcher_{before,after}.log` property by property, the only differences are four consecutive `copy_gpu_to_memory_async` properties. The three that were lost are DIS-COPY-ASYNC-DMA-ERROR, DIS-COPY-ASYNC-GATHER-LAYOUT and DIS-COPY-ASYNC-NULL-STREAM-SYNC; DIS-COPY-ASYNC-NO-SLOT was already unproved before the repair. On the after side all four were reported as "translation/compile error". Everything else, including every `clear_memory_tier` property, matched.

I found nothing that ties these failures to the repair:
- The repair does not touch `copy_gpu_to_memory_async`, its model or its drivers.
- Their `.coma` files match the base apart from the absolute paths.
- Their `proof.json` files are unchanged.
- In the repaired tree, `cargo creusot verify_dis_copy_async_dma_error` and `cargo creusot verify_dis_copy_async_gather_layout` print `Proved ✔`. That was a fresh rebuild after `touch src/*.rs`, with no compile error.
- I ran the same `scorer_creusot.py` the same way the acceptance check does: on a `cp -a` copy, with statuses stripped, `--cap-seconds 60 --cap-max 300`. Over the 16 properties around this spot (`--only`), DMA-ERROR, GATHER-LAYOUT and NULL-STREAM-SYNC prove, and so do all their neighbours. NO-SLOT and OVERFLOW stay goal-unproved, exactly as before the repair.

The gate deletes its working copy, so the compiler output from the failed run cannot be recovered. I found no toolchain, creusot-std or system change in that window. The failure looks like a transient environment fault during those four module runs. I made no code change for it, because there is nothing in the repair to fix.

## Scope beyond the finding

- The repair also fixes the same root cause for the closely related Creusot property DIS-CLEAR-MT-PIN-SAFE. I added the `verify_dis_clear_mt_pin_safe` driver and its twin for it. The property bundle (`verif/unified_properties.yaml`) still records it as REFUTED; I did not edit the bundle (outside my rules).
- Running `cargo creusot` regenerated the tracked `.coma` and `proof.json` files under `verif-creusot/verif/dispatcher_verif_rlib/`. These are tool output, not hand edits.
- `verif-loom/**` is not in the RULES write list, but step 4 tells me to edit it. I changed only the mirror of `clear_memory_tier`, two `mirrors`/line comments, and `mirrored_sources.sha256`. The Loom tests and twins are unchanged.

## Risk

- **New error result.** `clear_memory_tier` can now return `DispatcherError::Timeout` if a pin is held for more than 5 s. Before, it returned `Ok` while leaving a corrupted entry. Callers that `?` or unwrap the result will see the error instead of silent corruption.
- **Longer calls.** If a lookup or write-through overlaps the clear, the call now waits for it, typically microseconds to milliseconds and at most 5 s.
- **Count semantics.** `count` no longer includes entries that were not cleared. The Python spec test asserts `cleared >= len(keys)`, which still holds once pins drop.
- **No API change.** The method signature is the same.

## Follow-ups

- Update the `clear_memory_tier` doc in `components/interfaces/src/idispatcher.rs` and spec FR-038 so that they describe the wait for pins and the `Timeout` error (another component / specs, outside this repair's rules).
- Re-score DIS-CLEAR-MT-PIN-SAFE in the bundle. Its refutation no longer proves, and the new `verify_dis_clear_mt_pin_safe` proves.
- Not run to completion here:
  - `verify_dis_flush_then_clear_demotes__mutant`. It ran for more than 10 minutes and I stopped it. Its mutation is in the writer, not in code I changed.
  - The full Loom suite, which exceeds the 10-minute foreground limit. Only the tests that use `clear_memory_tier` were run.
- Clippy errors in `src/lib.rs` and fmt diffs in `src/pipeline.rs` were already there; I did not touch them.
- Acceptance gate: when a module is classed "translation/compile error", keep its raw `cargo creusot` output, or retry it once. In the second acceptance run, DIS-COPY-ASYNC-DMA-ERROR, DIS-COPY-ASYNC-GATHER-LAYOUT and DIS-COPY-ASYNC-NULL-STREAM-SYNC were reported lost this way, even though they prove on a rerun of the same tree, and the deleted working copy left nothing to diagnose.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-SPIN-CLEAR-MT-KEEPS-PINNED`, callee modules ``.

```
ACCEPT-REPO: base 01d004c70916fc0d5c29aaaa0fc109850b4eb180; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (114 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-SPIN-CLEAR-MT-KEEPS-PINNED yes (Loom)
  E. (before side for dispatcher reused from cache: same base, proof crate and gate)
  E. REVERIFY: dispatcher yes (110 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 114 passed in each run
  dispatcher                   | DIS-SPIN-CLEAR-MT-KEEPS-PINNED (Loom)    | violated before -> PROVED in Loom's search (all interleavings)); twin fails; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 110 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 60 min | 432 min | rejected |
| 2 | 99 min | 214 min | accepted |

Total wall-clock: 805 min.
