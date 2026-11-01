# fix_dispatcher_no-drives-promote-without-read

> **In short** - Problem: In memory-tier-only mode (no data drives) a cold serve reported success without promoting: the block was never delivered to its GPU destination, the dispatch-map entry was never re-registered as a memory-tier entry, and the reserved slot was stranded. Fix: both no-drives cold paths now run the with-drives serve-then-promote sequence (deliver to GPU, in-place `promote_block_to_memory_tier`, release the slot on failure).


```
component:      dispatcher
finding:        no-drives-promote-without-read
property:       DIS-BATCH-LOOKUP-COLD-PROMOTES,DIS-LOOKUP-NO-HW-PROMOTE
found by:       see "How it was found"
base commit:    850a00403ce3        branch: repair/j33-rc4-20261030
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
- DIS-BATCH-LOOKUP-COLD-PROMOTES: Each successfully served block-device entry of a batch is delivered to its GPU destination and is afterwards registered as a memory-tier entry, unless it was served through the staging fallback.
- DIS-LOOKUP-NO-HW-PROMOTE: In memory-tier-only mode a cold promotion creates a memory-tier slot, copies it to the GPU, and re-registers the entry as a memory-tier entry.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:365`
```
- **FR-039**: System MUST provide a `batch_lookup(entries: &[(CacheKey, Vec<IpcHandle>)]) -> Vec<Result<(), DispatcherError>>` method on the `IDispatcher` interface. *(Sync 2026-08-07 — signature backfilled: each entry now carries a `Vec<IpcHandle>` of destination regions, not a single `IpcHandle`, so one cache key can be scattered to multiple GPU destinations (e.g. sibling tensor-parallel ranks) in a single batched operation.)* This method processes a batch of lookup entries concurrently: (1) classifies all entries by dispatch-map state — a `dm.lookup` that returns `Err` is a transient timeout waiting for a `write_ref` (a concurrent store-commit / promote) to clear, NOT a missing key (dispatch-map entries survive demotion via an in-place flip to BlockDevice), so it is retried locally a few times (up to 5) rather than collapsed into `KeyNotFound`; collapsing it would forward a live key to remote-lookup, whose miss becomes a fatal load failure in the vLLM connector (`EngineDeadError`) *(Sync 2026-09-02 — code authoritative, commit `1d55b9c2`; this retry is defensive and does not fire under the current workload, where `write_ref` windows are sub-millisecond)*, (2) for MemoryTier hits, issues per-region `memcpy_h2d_async` H2D copies on the device's `warm` stream (FR-037), holding a dispatch-map read pin on each hit; the copies are only **queued** here. The warm hits' LRU refresh (`batch_touch`) still runs before cold prep so its evictor sees the same recency order, but the single batched warm `stream_synchronize` is **deferred until after the cold block** (steps 4-5), so the warm H2D drain overlaps the cold NVMe reads. Only after that sync are the warm read pins released — releasing at submission would let the memory-tier evictor reclaim a DRAM slot from under an in-flight DMA — and only then does the remote lookup for local misses (FR-045) run. A multi-region cold entry's `serve_memory_tier_to_gpu` sync on the same warm stream also waits for the still-queued warm copies, which is correct *(Sync 2026-10-08 — code authoritative, commit `ada87967`: the warm sync previously ran immediately after the classify loop, before cold prep)*, (3) cold (BlockDevice) entries are NOT special-cased by count — a lone single-key cold load goes through the same pooled path as a multi-entry batch (steps 4-5); the earlier single-entry inline `promote_and_serve` fast path was removed *(Sync 2026-09-02, commit `1d55b9c2`)* because it had no staging fallback (FR-053) and, under memory-tier pressure, turned a survivable cold miss into a fatal load failure — the pooled path defers tier saturation (`AllocationFailed`) to the staging post-pass so a cold load degrades to an uncached serve instead of failing; (4) groups BlockDevice (cold) entries by the drive recorded in their SSD location *(store-time placement: the drive is chosen round-robin per committing thread at `copy_gpu_to_memory_completed`/`schedule_write_through` — a per-thread cursor seeded once from a global counter, so one commit batch's keys get consecutive drives — and recorded in bits 56-63 of the SSD location, `(drive << 56) | extent_offset`; drive 0 encodes to the raw offset. The dispatch map stores the location opaquely. At recovery the drive is re-derived from the drive whose extent manager lists the extent. Objects stay whole on one drive. An entry whose recorded drive is out of range fails with `IoError("invalid drive in SSD location")` before any slot is reserved)*, and for each drive runs a per-entry `evict_and_insert` cold-prep that allocates the memory-tier slot and builds a `pipeline::ColdReadJob` (deferring entries that hit `AllocationFailed` to the staging post-pass, FR-053), (5) checks out **one** NVMe client channel per active drive (an RAII `ChannelLease` from that drive's `ChannelPool`, FR-034) and issues a **single** `scatter_gather_multi_drive_zero_copy` (FR-061) on the **caller thread** — one multiplexed poll loop that fans reads out to all drives and fans completions in, distributing a total `max_queue_depth = 128` proportionally across drives by segment count and issuing each object's coalesced H2D on the per-device pipeline stream pair (FR-052) the moment its last NVMe segment lands *(Sync 2026-09-21 — code authoritative, commit `6f35b13a`: this replaced the earlier model that spawned up to `MAX_QUEUES_PER_DRIVE` per-drive threads each running `pipelined_multi_object_zero_copy`; there are no longer any spawned cold-path threads and no `ColdReadPool` — see FR-044, retired)*, (6) merges results back in input order. Returns one `Result` per input entry.
- **FR-040**: System MUST provide `promote_to_memory_tier(keys: &[CacheKey])` on the `IDispatcher` interface. This is a best-effort, fire-and-forget method that asynchronously promotes SSD-resident (BlockDevice) entries to the memory-tier without GPU DMA. For each key: if in BlockDevice state, reads data from SSD into a new memory-tier slot (via `pipelined_ssd_to_dram_only`) and updates the dispatch-map to MemoryTier; if in MemoryTier or Staging, refreshes the eviction timestamp; if not found, silently skips. Cold keys are grouped by the drive recorded in their SSD location; a key whose recorded drive is out of range is skipped without a log message *(Sync 2026-10-27 — code authoritative, commit `4bb69289`, `src/lib.rs:4017-4026`)*. Errors on individual keys are logged via ILogger but not propagated. The shm-queue control transport (the sole control transport since gRPC was removed) spawns this as a detached background task when a batch-touch request sets `promote = true`.
- **FR-041**: The pipeline module MUST provide `pipelined_ssd_to_dram_only` and `pipelined_multi_ssd_to_dram_only` functions that perform pipelined NVMe reads directly into memory-tier slots without any GPU DMA. These use the same NVMe queue depth saturation strategy as FR-019 (submit up to `max_queue_depth` reads, sliding-window completion loop) but omit all CUDA stream management and GPU copy operations. The `promote_to_memory_tier` implementation currently calls `pipelined_ssd_to_dram_only` for each entry sequentially per drive thread; the multi-object variant exists for future batch promotion optimization.
```
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:19`
```
**Last Synced**: 2026-09-21 — drift sweep on branch `evolve-throughput` after the scatter-gather batch cold path (`6f35b13a` "optimizer iter-005: scatter-gather"). BACKFILL (code authoritative): `batch_lookup`'s cold path no longer spawns per-drive threads that each run `pipelined_multi_object_zero_copy`; instead it checks out **one** channel per active drive and runs a **single multiplexed scatter-gather poll loop on the caller thread** (`pipeline::scatter_gather_multi_drive_zero_copy`), fanning NVMe reads out to all drives and fanning completions in, with the total `max_queue_depth` (128) distributed proportionally per drive by segment count. New **FR-061** documents `scatter_gather_multi_drive_zero_copy` + the `DriveWork` descriptor; FR-039 steps (4)/(5), FR-019, User Story 11 (narrative + scenarios 1 & 3), and the Key Entities "Pipelined Reader" updated to the scatter-gather model (and FR-039(4) `key % num_drives` corrected to the splitmix64 hash of the key mod `num_drives` that `drive_index` actually computes). CODE CHANGE: the now-unused `ColdReadPool` (module `src/cold_pool.rs`, field, init/shutdown wiring) was **deleted** — commit `6f35b13a` routed the batch cold path through scatter-gather and left the pool constructed at init and torn down at shutdown but never submitted to; **FR-044** is retired (`~~REMOVED~~`) and the `cold_pool::ColdReadRequest` reference in FR-052 removed. `promote_and_serve` (single-entry cold) still uses `pipelined_multi_object_zero_copy`. No behavior change from the pool deletion (dead code); the scatter-gather behavior itself shipped in `6f35b13a`.

**Last Synced**: 2026-09-28 — branch `certus-lookup-observability`, spec 002 Phase 1. CODE CHANGE: a remote lookup that comes back **not found** now returns `KeyNotFound`, not `IoError`. `batch_lookup`'s remote-forward arm collapsed every `RemoteLookupError` into `DispatcherError::IoError`, discarding the `NotFound` vs `TransportError` distinction `IRemoteLookup` already made. Two consequences, and the second is worse: the transport host counts only `KeyNotFound` as a miss and drops the rest, so **every remotely-unfound key vanished from the server's accounting** — `certus_lookup_misses_total` read 0 against ~4M reserves on the cluster; and a key that simply is not cached anywhere was reported as an I/O failure, so anything watching error rates saw transport failures that never happened. FR-011 gains the unsuccessful-fetch case, which it did not previously state; `contracts/errors.md` updated for both variants. Verified on hardware: hits **and** misses now match the client's own independent counts exactly, and hits + misses = total key references, in both a peer-reachable and a peer-isolated configuration (605 635 and 656 039 misses where both read 0 before). Guarded by `a_key_no_peer_holds_is_a_miss_not_an_io_error` and `a_remote_transport_failure_is_still_an_error`, the second so the fix cannot degenerate into calling a broken fabric a cold cache.
```

### The code: before (-) and after (+)
```diff
@@ -863,5 +863,13 @@ impl DispatcherComponent {
         let drives = self.data_drives.read();
         if drives.is_empty() {
-            // No hardware: just copy zeros to GPU (test/staging-only mode).
+            // No hardware (test/staging-only mode): the fresh memory-tier slot
+            // stands in for the SSD read. Run the same serve-then-promote
+            // sequence as the with-drives path below, so a memory-tier-only
+            // promotion lands the same end state: deliver the slot to the GPU
+            // destination, then re-register the entry in place. The flip must
+            // be `promote_block_to_memory_tier` — `create_memory_tier_entry`
+            // refuses existing keys, and this key's entry exists (BlockDevice)
+            // by construction, so it could only ever answer AlreadyExists and
+            // leave the entry unregistered and the slot stranded.
             let aligned = total_bytes.next_multiple_of(4096).max(4096);
             let temp_buf = unsafe {
@@ -869,5 +877,5 @@ impl DispatcherComponent {
             }
             .map_err(|e| DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}")))?;
-            let result = gpu.dma_copy_to_device(
+            let copy = gpu.dma_copy_to_device(
                 &temp_buf,
                 ipc_handle.address as *mut std::ffi::c_void,
@@ -875,15 +883,28 @@ impl DispatcherComponent {
             );
             std::mem::forget(temp_buf);
-            // Register promoted entry in dispatch-map.
-            if dm
-                .create_memory_tier_entry(key, mem_ptr, ipc_handle.size)
-                .is_ok()
-            {
-                self.tier_counters.record_promotion_to_memory();
+            if let Err(e) = copy {
+                // The GPU delivery failed: release the reserved slot — the
+                // dispatch map still points the key at the SSD, so keeping it
+                // would strand dead tier capacity.
+                if !staged {
+                    let _ = mt.remove(key);
+                }
+                return Err(DispatcherError::IoError(format!(
+                    "GPU DMA copy (promote) failed: {e}"
+                )));
             }
-            let _ = dm.release_write(key);
-            return result.map_err(|e| {
-                DispatcherError::IoError(format!("GPU DMA copy (promote) failed: {e}"))
-            });
+            // In-place BlockDevice->MemoryTier transition, as the with-drives
+            // path (retains the SSD offset, preserves the pin the lookup
+            // holds, no remove/recreate).
+            if let Err(e) = dm.promote_block_to_memory_tier(key, mem_ptr, ipc_handle.size) {
+                if !staged {
+                    let _ = mt.remove(key);
+                }
+                return Err(DispatcherError::IoError(format!(
+                    "promote transition failed: {e}"
+                )));
+            }
+            self.tier_counters.record_promotion_to_memory();
+            return Ok(());
         }
... (72 more lines in the commit)
```

### The test that shows the bug: `components/dispatcher/src/repair_test_no_drives_promote_without_read.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `no-drives-promote-without-read`
//! (DIS-BATCH-LOOKUP-COLD-PROMOTES, DIS-LOOKUP-NO-HW-PROMOTE).
//!
//! In memory-tier-only mode (no ISPDKEnv bound, so `data_drives` is empty) a
//! cold serve must run the same serve-then-promote sequence the with-drives
//! cold path runs: reserve a memory-tier slot, deliver it to the entry's GPU
//! destination, and only then re-register the dispatch-map entry as a
//! memory-tier entry (the in-place `promote_block_to_memory_tier` flip).
//!
//! The unfixed code skips steps of that sequence: `batch_lookup`'s no-drives
//! arm never delivers to the GPU destination at all, and `promote_and_serve`'s
//! no-drives arm re-registers with `create_memory_tier_entry` — which can only
//! answer `AlreadyExists` for a key that already exists as BlockDevice — so the
//! entry is never re-registered, the fresh slot is stranded, and a foreign
//! `release_write` runs. On a failed GPU delivery neither path released the
//! reserved slot.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    IpcHandle, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

const KEY: CacheKey = 1;
const BLOCK: u32 = 4096;

/// The byte a fresh pool slot holds at byte `off` of its 4 KiB extent: the
/// first half of every slot is `0x5A`, the second half `0xA5`, so a delivery
/// assertion also pins down *which* slot bytes each GPU region received.
fn slot_byte(off: usize) -> u8 {
    if off % 4096 < 2048 {
... (700 more lines)
```


Traces: US-11 AS-1, FR-039, FR-061, dispatcher/src/lib.rs:2681, interfaces/src/idispatch_map.rs:146, US-9 AS-3

## How it was found
- creusot: DIS-BATCH-LOOKUP-COLD-PROMOTES: unscored; DIS-LOOKUP-NO-HW-PROMOTE: unscored -  | 
- kani: DIS-BATCH-LOOKUP-COLD-PROMOTES: unscored; DIS-LOOKUP-NO-HW-PROMOTE: unscored -  | 

## The problem (agent's statement and verdict)
# Problem — DIS-BATCH-LOOKUP-COLD-PROMOTES, DIS-LOOKUP-NO-HW-PROMOTE (finding `no-drives-promote-without-read`)

**One sentence:** In memory-tier-only mode (no data drives), a cold serve reports success without
doing the promote sequence — `batch_lookup` never delivers the block to its GPU destination, and
`lookup`'s promotion never re-registers the entry as a memory-tier entry (and releases a write
reference it does not hold).

**Spec location:** `components/dispatcher/verif/unified_properties.yaml`:
- `DIS-BATCH-LOOKUP-COLD-PROMOTES` (traces US-11 AS-1, FR-039, FR-061, `dispatcher/src/lib.rs:2681`,
  `interfaces/src/idispatch_map.rs:146`): "Each successfully served block-device entry of a batch is
  delivered to its GPU destination and is afterwards registered as a memory-tier entry, unless it
  was served through the staging fallback."
- `DIS-LOOKUP-NO-HW-PROMOTE` (traces US-9 AS-3, methods: `lookup`): "In memory-tier-only mode a
  cold promotion creates a memory-tier slot, copies it to the GPU, and re-registers the entry as a
  memory-tier entry."

**Code location:** `components/dispatcher/src/lib.rs`, the two `num_drives == 0` cold-promotion
arms (one root cause: both skip steps of the serve-then-promote sequence that the with-drives cold
path runs):
- `components/dispatcher/src/lib.rs:2703-2735` — `batch_lookup`'s no-drives loop: after
  `mt.insert` it flips the entry with `dm.promote_block_to_memory_tier` but never delivers the
  slot to the entry's GPU destination (`entry.regions` is unused), and it reports `Ok` even when
  the promote flip failed (only the slot is released).
- `components/dispatcher/src/lib.rs:863-888` — `promote_and_serve`'s no-drives arm: it copies the
  slot to the GPU, but then calls `dm.create_memory_tier_entry(key, ...)`, which can only fail
  `AlreadyExists` here (the key's entry already exists as BlockDevice — that is what made this a
  *cold* lookup), so the entry is never re-registered as a memory-tier entry, the fresh tier slot
  is stranded, and the arm then releases a write reference nobody holds
  (`let _ = dm.release_write(key)`).

**Verdict: code wrong.** The spec matches the with-drives cold path's serve-then-promote sequence
and `IDispatchMap`'s documented semantics (`promote_block_to_memory_tier` is the in-place
BlockDevice→MemoryTier re-registration; `create_memory_tier_entry` refuses existing keys —
`interfaces/src/idispatch_map.rs:128-161`); both no-drives arms violate it.

# Repair — `no-drives-promote-without-read` (DIS-BATCH-LOOKUP-COLD-PROMOTES, DIS-LOOKUP-NO-HW-PROMOTE)

## In short

Problem: In memory-tier-only mode (no data drives) a cold serve reported success without
promoting: the block was never delivered to its GPU destination, the dispatch-map entry was never
re-registered as a memory-tier entry, and the reserved slot was stranded. Fix: both no-drives cold
paths now run the with-drives serve-then-promote sequence (deliver to GPU, in-place
`promote_block_to_memory_tier`, release the slot on failure).

## Bug

In `components/dispatcher/src/lib.rs`, the two `num_drives == 0` cold-promotion arms each skipped
steps of the serve-then-promote sequence that the with-drives cold path runs, so a successfully
served cold entry was never delivered to its GPU destination and/or never re-registered as a
memory-tier entry.

## Impact

A user running the dispatcher without SPDK data drives (staging/test mode, `ISPDKEnv` unbound)
would see cold lookups and cold `batch_lookup` entries "succeed" while the GPU destination buffer
kept its old bytes (nothing was copied), the dispatch map kept pointing the key at the SSD, and
the freshly reserved memory-tier slot stayed allocated but unreachable — dead tier capacity that
later evictions could not reclaim. `lookup`'s no-drives arm additionally called
`release_write` on a write reference nobody held, and neither arm released the reserved slot when
the GPU delivery failed, so a failing GPU leaked one slot per attempt.

## How to reproduce

1. `cargo test -p dispatcher repair_test_no_drives` — five tests drive the public `IDispatcher`
   API (`lookup`, `batch_lookup`) against in-file mocks of `IDispatchMap`/`IMemoryTier`/
   `IGpuServices` in memory-tier-only mode; on the unfixed code all five fail (assertions on the
   delivered bytes, the re-registered entry, and the slot release).
2. To see it by hand: seed a BlockDevice entry (`recover_extent`), call `lookup` with a host
   buffer, and check the dispatch map still says `BlockDevice` and the tier slot exists but the
   GPU buffer was never written — on the unfixed code all three observations hold.

## Fix

`components/dispatcher/src/lib.rs` (one root cause, both arms):

- `promote_and_serve`'s no-drives arm (lib.rs:863): after reserving the slot it now (a) delivers
  the slot to the GPU destination with the synchronous `dma_copy_to_device` (as before), (b)
  re-registers the entry with `promote_block_to_memory_tier` — the in-place BlockDevice→MemoryTier
  flip — instead of `create_memory_tier_entry`, which refuses existing keys and could only answer
  `AlreadyExists` here (the key exists as BlockDevice by construction of a *cold* lookup), and
  (c) no longer calls `release_write` on a reference it does not hold. On a failed delivery or a
  failed flip it releases the reserved slot (`mt.remove`) and returns the error.
- `batch_lookup`'s no-drives arm (lib.rs:2703): after `mt.insert` it now delivers the slot to the
  entry's GPU destination via `serve_memory_tier_to_gpu` (scatter over `entry.regions`) *before*
  flipping the entry with `promote_block_to_memory_tier`, instead of flipping immediately and
  never delivering; a failed delivery or flip releases the slot and fails the entry instead of
  reporting `Ok`.

Verification (`components/dispatcher/verif-creusot/`): the models of `promote_and_serve`
(lookup.rs), `bl_cold_nodrive` (batch.rs, now also carrying the per-entry serve/promote record
`NdG`) and their callers were updated to the new bodies, and two new driver proofs were added:
`verify_dis_batch_lookup_cold_promotes`(+ `__mutant` twin, which correctly fails) and
`verify_dis_lookup_no_hw_promote`(+ twin, correctly fails).

Rejected alternatives:

- Re-register with `create_memory_tier_entry` after removing the BlockDevice entry — loses the
  in-place guarantee (pin-safety, SSD offset retention) the with-drives path relies on and races
  a concurrent reader between remove and create.
- Keep the old "flip first, deliver never" `batch_lookup` shape and only fix `promote_and_serve`
  — DIS-BATCH-LOOKUP-COLD-PROMOTES explicitly requires the delivery, and both arms share the one
  root cause.

Second-iteration fix (why the first attempt was rejected): the new success-guarantee `ensures` on
the model's `promote_and_serve` omitted `s.p.nd@ == 0` from its precondition, so it claimed
`Ok` even when the *with-drives* path's SSD read fails — an unprovable (false) contract that left
`vc_promote_and_serve` unproved, a whole-crate regression against the base. The precondition now
carries `s.p.nd@ == 0` (memory-tier-only mode, exactly the property's scope); the with-drives
outcome is governed by the existing SSD-oracle ensures.

## Scope beyond the finding

The failure exits (slot release on failed GPU delivery / failed promote flip) are exercised by
two of the five regression tests and stated in the model, but they are the same root cause: the
arms skipped steps of one sequence. No other behaviour changed. `serve_memory_tier_to_gpu`'s model
gained a monotone-tick `ensures` needed by its own proof; nothing else.

## Risk

Behaviour change is confined to memory-tier-only mode (`data_drives` empty): cold entries that
used to "succeed" without delivery now either really deliver and re-register or fail with
`IoError` and release the slot. Callers in that mode get strictly more information; the
with-drives paths are untouched. No interface change; `cargo test -p dispatcher` passes 166 tests
(5/5 runs), and the whole-crate Creusot gate shows 157 unproved goals at base and after (same
set, plus the two target proofs now proved) — no non-mutant proof regressed.

## Follow-ups

- `refute_dis_lookup_cold_promotes` and the other base-unproved obligations are untouched
  (pre-existing, out of scope for this finding).
- The staging fallback arm (`Path::Staged`) still deliberately does not promote; that matches the
  property's "unless served through the staging fallback" exemption.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-BATCH-LOOKUP-COLD-PROMOTES,DIS-LOOKUP-NO-HW-PROMOTE`, callee modules `batch_lookup,bl_cold_nodrive`.

```
ACCEPT-REPO: base 850a00403ce3a8ae4815c3abcb59f5d7f264e99f; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (166 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-BATCH-LOOKUP-COLD-PROMOTES yes
  C. TARGET   : dispatcher DIS-LOOKUP-NO-HW-PROMOTE yes
  E. (before side for dispatcher reused from cache: same base, proof crate and gate)
  E. REVERIFY: dispatcher yes (113 proved after the repair; 1 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 166 passed in each run
  dispatcher                   | DIS-BATCH-LOOKUP-COLD-PROMOTES           | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | DIS-LOOKUP-NO-HW-PROMOTE                 | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 113 proved, 1 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 300 min | 117 min | rejected |
| 2 | 22 min | 94 min | accepted |

Total wall-clock: 533 min.
