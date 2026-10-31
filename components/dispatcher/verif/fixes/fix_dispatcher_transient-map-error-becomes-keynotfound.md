# fix_dispatcher_transient-map-error-becomes-keynotfound

> **In short** - Problem: When a batch lookup's index read kept failing because another thread was writing the entry, the dispatcher reported the live key as "not found" — a cache miss — and asked peer nodes for it, which can crash a vLLM connector. Fix: After the five retries, the map's own error (a timeout) is reported with Error attribution, so a writer-held key is never called a miss or forwarded.


```
component:      dispatcher
finding:        transient-map-error-becomes-keynotfound
property:       DIS-BATCH-LOOKUP-TRANSIENT-RETRY
found by:       see "How it was found"
base commit:    850a00403ce3        branch: repair/j33-rc17-20261030
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
A transient dispatch-map lookup error caused by a concurrent writer is retried up to five times and is not reported as KeyNotFound or forwarded to remote lookup merely because of that error.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:365`
```
- **FR-039**: System MUST provide a `batch_lookup(entries: &[(CacheKey, Vec<IpcHandle>)]) -> Vec<Result<(), DispatcherError>>` method on the `IDispatcher` interface. *(Sync 2026-08-07 — signature backfilled: each entry now carries a `Vec<IpcHandle>` of destination regions, not a single `IpcHandle`, so one cache key can be scattered to multiple GPU destinations (e.g. sibling tensor-parallel ranks) in a single batched operation.)* This method processes a batch of lookup entries concurrently: (1) classifies all entries by dispatch-map state — a `dm.lookup` that returns `Err` is a transient timeout waiting for a `write_ref` (a concurrent store-commit / promote) to clear, NOT a missing key (dispatch-map entries survive demotion via an in-place flip to BlockDevice), so it is retried locally a few times (up to 5) rather than collapsed into `KeyNotFound`; collapsing it would forward a live key to remote-lookup, whose miss becomes a fatal load failure in the vLLM connector (`EngineDeadError`) *(Sync 2026-09-02 — code authoritative, commit `1d55b9c2`; this retry is defensive and does not fire under the current workload, where `write_ref` windows are sub-millisecond)*, (2) for MemoryTier hits, issues per-region `memcpy_h2d_async` H2D copies on the device's `warm` stream (FR-037), holding a dispatch-map read pin on each hit; the copies are only **queued** here. The warm hits' LRU refresh (`batch_touch`) still runs before cold prep so its evictor sees the same recency order, but the single batched warm `stream_synchronize` is **deferred until after the cold block** (steps 4-5), so the warm H2D drain overlaps the cold NVMe reads. Only after that sync are the warm read pins released — releasing at submission would let the memory-tier evictor reclaim a DRAM slot from under an in-flight DMA — and only then does the remote lookup for local misses (FR-045) run. A multi-region cold entry's `serve_memory_tier_to_gpu` sync on the same warm stream also waits for the still-queued warm copies, which is correct *(Sync 2026-10-08 — code authoritative, commit `ada87967`: the warm sync previously ran immediately after the classify loop, before cold prep)*, (3) cold (BlockDevice) entries are NOT special-cased by count — a lone single-key cold load goes through the same pooled path as a multi-entry batch (steps 4-5); the earlier single-entry inline `promote_and_serve` fast path was removed *(Sync 2026-09-02, commit `1d55b9c2`)* because it had no staging fallback (FR-053) and, under memory-tier pressure, turned a survivable cold miss into a fatal load failure — the pooled path defers tier saturation (`AllocationFailed`) to the staging post-pass so a cold load degrades to an uncached serve instead of failing; (4) groups BlockDevice (cold) entries by the drive recorded in their SSD location *(store-time placement: the drive is chosen round-robin per committing thread at `copy_gpu_to_memory_completed`/`schedule_write_through` — a per-thread cursor seeded once from a global counter, so one commit batch's keys get consecutive drives — and recorded in bits 56-63 of the SSD location, `(drive << 56) | extent_offset`; drive 0 encodes to the raw offset. The dispatch map stores the location opaquely. At recovery the drive is re-derived from the drive whose extent manager lists the extent. Objects stay whole on one drive. An entry whose recorded drive is out of range fails with `IoError("invalid drive in SSD location")` before any slot is reserved)*, and for each drive runs a per-entry `evict_and_insert` cold-prep that allocates the memory-tier slot and builds a `pipeline::ColdReadJob` (deferring entries that hit `AllocationFailed` to the staging post-pass, FR-053), (5) checks out **one** NVMe client channel per active drive (an RAII `ChannelLease` from that drive's `ChannelPool`, FR-034) and issues a **single** `scatter_gather_multi_drive_zero_copy` (FR-061) on the **caller thread** — one multiplexed poll loop that fans reads out to all drives and fans completions in, distributing a total `max_queue_depth = 128` proportionally across drives by segment count and issuing each object's coalesced H2D on the per-device pipeline stream pair (FR-052) the moment its last NVMe segment lands *(Sync 2026-09-21 — code authoritative, commit `6f35b13a`: this replaced the earlier model that spawned up to `MAX_QUEUES_PER_DRIVE` per-drive threads each running `pipelined_multi_object_zero_copy`; there are no longer any spawned cold-path threads and no `ColdReadPool` — see FR-044, retired)*, (6) merges results back in input order. Returns one `Result` per input entry.
- **FR-040**: System MUST provide `promote_to_memory_tier(keys: &[CacheKey])` on the `IDispatcher` interface. This is a best-effort, fire-and-forget method that asynchronously promotes SSD-resident (BlockDevice) entries to the memory-tier without GPU DMA. For each key: if in BlockDevice state, reads data from SSD into a new memory-tier slot (via `pipelined_ssd_to_dram_only`) and updates the dispatch-map to MemoryTier; if in MemoryTier or Staging, refreshes the eviction timestamp; if not found, silently skips. Cold keys are grouped by the drive recorded in their SSD location; a key whose recorded drive is out of range is skipped without a log message *(Sync 2026-10-27 — code authoritative, commit `4bb69289`, `src/lib.rs:4017-4026`)*. Errors on individual keys are logged via ILogger but not propagated. The shm-queue control transport (the sole control transport since gRPC was removed) spawns this as a detached background task when a batch-touch request sets `promote = true`.
- **FR-041**: The pipeline module MUST provide `pipelined_ssd_to_dram_only` and `pipelined_multi_ssd_to_dram_only` functions that perform pipelined NVMe reads directly into memory-tier slots without any GPU DMA. These use the same NVMe queue depth saturation strategy as FR-019 (submit up to `max_queue_depth` reads, sliding-window completion loop) but omit all CUDA stream management and GPU copy operations. The `promote_to_memory_tier` implementation currently calls `pipelined_ssd_to_dram_only` for each entry sequentially per drive thread; the multi-object variant exists for future batch promotion optimization.
```
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:334`
```
- **FR-008**: The `check(key)` method MUST return whether a cache entry exists for the given key without performing any data transfer. A dispatch-map lookup that returns `Err` is a timeout behind a writer that holds the entry (a concurrent store-commit or promotion), not a missing key: `check` MUST NOT report such a key as absent. It retries the lookup up to 5 more times, as `batch_lookup` does (FR-039 step 1), and if every retry times out it returns `Err(DispatcherError::Timeout)`; other dispatch-map errors return `Err(DispatcherError::IoError)`. One call can therefore wait up to about 12 s (6 waits of 2 s) behind a writer. *(Sync 2026-10-28 — code authoritative, commit `898bba7eb`; root cause #14: before it, a timeout was reported as `Ok(false)`.)*
- **FR-009**: The `remove(key)` method MUST free the memory-tier slot (if present via `mt.remove()`), free the extent on SSD (if the entry has one: a BlockDevice entry, or a MemoryTier entry whose write-through has landed and recorded its `ssd_offset`), and remove the dispatch map entry. The extent is freed on the drive decoded from the entry's SSD location (`select_drive`, FR-039 step (4)), at the decoded raw offset; a location whose drive has no extent manager frees nothing. Its own dispatch-map lookup MUST release only a read reference that lookup acquired: a `NotExist` answer takes none, so none is released. *(Sync 2026-10-23, branch `fix/dispatcher-remove-ssd-extent` — code authoritative, commits `a7444f4ab` and `765f75ba3`, repair of root causes #9 and #12. Before it, `LookupResult::MemoryTier` hid the `ssd_offset`, so the extent of a persisted resident entry leaked and the key came back from SSD after a `format_on_init=false` restart (SC-003); and a lookup that answered `NotExist` still released a read reference, which could decrement the write-through pin of a concurrently registered entry.)* *(Sync 2026-10-27 — code authoritative, commit `4bb69289`; previously the drive was recomputed as `drive_index(key, N)`.)*
- **FR-010**: The dispatcher component MUST use the component framework's `define_component!` macro and expose only the `IDispatcher` interface.
```

### The code: before (-) and after (+)
```diff
@@ -2591,4 +2591,11 @@ impl IDispatcher for DispatcherComponent {
             // the one remaining fatal classification — Err(Timeout) misread as
             // KeyNotFound — should lock contention ever widen that window.
+            //
+            // And when every retry DOES time out, the map's own error is what
+            // this entry reports (FR-039 step 1): the key is live — held by a
+            // writer — so an exhausted retry is a `Timeout` to surface (as
+            // `check` does, FR-008), never a `KeyNotFound`, which would both
+            // attribute the live key a Miss and forward it to remote lookup
+            // (the remote pass below collects exactly the KeyNotFound results).
             const LOOKUP_RETRIES: u32 = 5;
             let lookup = {
@@ -2665,6 +2672,16 @@ impl IDispatcher for DispatcherComponent {
                     }
                 },
-                Err(_) => {
-                    results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
+                // Every retry timed out behind a writer (or the read count
+                // overflowed): the key's presence could not be determined,
+                // but the entry is live. That is a wait to report — the
+                // map's own error, never an absent key (see the comment
+                // above the retry loop).
+                Err(interfaces::DispatchMapError::Timeout(k)) => {
+                    results[i] = Some(Err(DispatcherError::Timeout(format!(
+                        "timeout waiting on key: {k}"
+                    ))));
+                }
+                Err(other) => {
+                    results[i] = Some(Err(DispatcherError::IoError(other.to_string())));
                 }
             }
@@ -8542,2 +8559,6 @@ mod repair_test_duplicate_check_after_eviction;
 #[path = "repair_test_check_present_map_timeout.rs"]
 mod repair_test_check_present_map_timeout;
+
+#[cfg(test)]
+#[path = "repair_test_transient_map_error.rs"]
+mod repair_test_transient_map_error;
```

### The test that shows the bug: `components/dispatcher/src/repair_test_transient_map_error.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `transient-map-error-becomes-keynotfound`
//! (DIS-BATCH-LOOKUP-TRANSIENT-RETRY, FR-039 step 1).
//!
//! `batch_lookup`'s classification loop retries a `dm.lookup` that answers
//! `Err` (a writer holds the entry's write reference past the map's 2 s
//! `DEFAULT_TIMEOUT`) up to five more times. But when every retry also times
//! out, the classification arm collapses the remaining error into
//! `Err(DispatcherError::KeyNotFound(key))` (lib.rs:2667-2669). A KeyNotFound
//! result is then forwarded to remote lookup (lib.rs:3025-3032) and attributed
//! `ServedBy::Miss` (lib.rs:3239) — for a key that HAS a cache entry, held by
//! a concurrent store-commit / promote. The spec (FR-039 step 1) forbids
//! exactly that: "retried locally a few times (up to 5) rather than collapsed
//! into `KeyNotFound`; collapsing it would forward a live key to
//! remote-lookup".
//!
//! This test models the writer hold deterministically, the same way
//! `repair_test_check_present_map_timeout` does for `check`: the mock map's
//! `lookup` answers `Err(Timeout)` for the first `timeouts_left` calls on a
//! writer-held entry (each an expired 2 s wait), and the writer settles within
//! the next attempt's wait when `timeouts_left` reaches 0.
//!
//! - KEY_SETTLES: the writer outlasts three lookup deadlines, then settles
//!   inside the retry budget → the key must be served warm (Ok, DRAM), never
//!   reported KeyNotFound and never forwarded to remote lookup.
//! - KEY_HELD: the writer outlasts every retry (all six attempts time out) →
//!   the key must be reported as the map's own error (a timeout to wait out),
//!   never as KeyNotFound / a Miss, and never forwarded to remote lookup.
//! - KEY_ABSENT: no cache entry at all → KeyNotFound, Miss, and forwarded.
//!
//! Self-contained (its own mocks, the public `IDispatcher` API only), so it
//! builds against the dispatcher as it was before the repair as well as
//! after it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DispatcherError, DmaBuffer, GpuDeviceInfo,
    GpuDmaBuffer, GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger,
... (747 more lines)
```


Traces: FR-039 step 1, dispatcher/src/lib.rs:2377, dispatcher/src/lib.rs:2358, dispatcher/src/lib.rs:2445

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Finding: transient-map-error-becomes-keynotfound (DIS-BATCH-LOOKUP-TRANSIENT-RETRY)

**Plain sentence**: When a batch lookup cannot read a cache entry because another
thread is briefly writing to it, the dispatcher asks its index again (up to five
times) — but if the index still cannot answer, the dispatcher tells the caller
"key not found" and asks other machines for the key, even though the key is very
much present locally.

**Spec location**: `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`,
FR-039 step (1) (line 365): "a `dm.lookup` that returns `Err` is a transient
timeout waiting for a `write_ref` (a concurrent store-commit / promote) to clear,
NOT a missing key … so it is retried locally a few times (up to 5) rather than
collapsed into `KeyNotFound`; collapsing it would forward a live key to
remote-lookup, whose miss becomes a fatal load failure in the vLLM connector
(`EngineDeadError`)".

**Code location**: `components/dispatcher/src/lib.rs:2569-2670` — the
classification loop of `batch_lookup`. The retry wrapper (`lib.rs:2593-2602`,
`LOOKUP_RETRIES = 5`) is correct, but the exhausted-error arm at `lib.rs:2667-2669`
maps any remaining `Err` to `Err(DispatcherError::KeyNotFound(key))`. Downstream,
`lib.rs:3025-3032` collects exactly the `KeyNotFound` results and forwards them to
`IRemoteLookup::batch_lookup` (`lib.rs:3050`), and `lib.rs:3239` attributes them
`ServedBy::Miss`.

**Verdict: code wrong.** The retry loop already exists and is sound; the defect is
the collapse of the retried-and-still-failing lookup into `KeyNotFound`, which
(1) reports a live writer-held key as a cache miss and (2) forwards it to remote
lookup, exactly what FR-039 step (1) forbids. The sibling method `check` was fixed
the same way in commit `898bba7eb` / spec FR-008 (`lib.rs:3399-3415`: exhausted
retries report `Err(DispatcherError::Timeout)`, other map errors
`Err(DispatcherError::IoError)`); `batch_lookup`'s classification arm was never
given the same treatment. The fix is to mirror `check`: report the map's own
error (`Timeout` / `IoError`) so the entry is attributed `ServedBy::Error` and is
not eligible for remote forwarding.

# Repair: transient-map-error-becomes-keynotfound (DIS-BATCH-LOOKUP-TRANSIENT-RETRY)

## In short

Problem: When a batch lookup's index read kept failing because another thread
was writing the entry, the dispatcher reported the live key as "not found" —
a cache miss — and asked peer nodes for it, which can crash a vLLM connector.
Fix: After the five retries, the map's own error (a timeout) is reported with
Error attribution, so a writer-held key is never called a miss or forwarded.

## Bug

`batch_lookup`'s classification loop retried a failing dispatch-map lookup up
to five times, but when every retry failed it collapsed the remaining error
into `Err(DispatcherError::KeyNotFound(key))` (`src/lib.rs:2667-2669` before
the fix) — and a KeyNotFound result is exactly what the remote-lookup pass
collects and forwards to peers (`src/lib.rs:3025-3032`) and what the
attribution pass labels `ServedBy::Miss` (`src/lib.rs:3239`).

## Impact

A user running lookups concurrent with stores/promotions that hold a
dispatch-map `write_ref` longer than the retry budget (about 12 s of waits)
would see a live, cached key reported as a cache miss (`KeyNotFound`, MISS
attribution, deflating the hit rate) and forwarded to remote lookup — whose
miss escalates to a fatal LOAD failure (`EngineDeadError`) in the vLLM
connector, since that path has no graceful degrade (spec FR-039 step 1).
The `check` method had already been fixed this way (FR-008, commit
`898bba7eb`); `batch_lookup` — the primary benchmark path — had not.

## How to reproduce

1. `cargo test -p dispatcher --lib repair_test_transient_map_error` — the
   test seeds a writer-held entry whose writer never settles; on the unfixed
   code it panics with "a live key behind a writer must never be reported
   KeyNotFound (got Err(KeyNotFound(2)))".
2. The same test asserts the writer that settles inside the retry budget is
   served warm (Ok, DRAM, data delivered, pin released) and that only a
   genuinely absent key is forwarded to the mock remote lookup.
3. `cd components/dispatcher/verif-creusot && cargo creusot
   verify_dis_batch_lookup_transient_retry` (Proved) and `cargo creusot
   verify_dis_batch_lookup_transient_retry_mutant` (must fail).

## Fix

`components/dispatcher/src/lib.rs`, classification loop of `batch_lookup`:
the exhausted-retry arm now maps the dispatch map's own error the same way
`check` does (FR-008) — `DispatchMapError::Timeout(k)` →
`Err(DispatcherError::Timeout("timeout waiting on key: {k}"))`, any other
map error → `Err(DispatcherError::IoError(...))`. A non-KeyNotFound result
is attributed `ServedBy::Error` and is not collected by the remote-forward
pass, so a live key is neither miscounted as a miss nor sent to peers. The
retry loop itself (initial attempt + up to five retries) was already correct
and is unchanged; the comment above it now documents the exhausted-retry
reporting.

Alternatives rejected:
- **Retry forever / raise the retry count**: an unbounded wait would hang the
  batch behind a stuck writer; the five-retry budget is the spec's
  (FR-039 step 1) and matches `check`.
- **Report `ServedBy::Miss` but skip remote forwarding**: would keep
  miscounting a live key as a cache miss in the hit-rate metrics; the error
  taxonomy already has the right bucket (`Error`).
- **Fixing `lookup_async`'s single-key Err arm the same way too**: that is
  the same bug class but a different method with its own property bundle
  (DIS-LOOKUP-*); changing it here would exceed this finding's scope. Noted
  as a follow-up.

Creusot mirror (`components/dispatcher/verif-creusot/`): `bl_classify`'s Err
arm now pushes a new `Cls::Retry { e }` classification carrying the map error
and its `DE` mapping (`Timeout` → `DE::Timeout`, else `DE::Io(IoK::Dm)`)
instead of `Cls::Miss` + `KeyNotFound`; `lookup_retry` returns a ghost retry
count bounded by 5; `batch_lookup`'s contract adds the Retry arms (tier
`None`, result `Timeout`/`Io(Dm)`, never Miss, never forwarded). New driver
pair `verify_dis_batch_lookup_transient_retry` (proved: retry count ≤ 5; the
map error means a live writer-held entry; a Retry entry is attributed
`Error`, is never `KeyNotFound`, and no forwarded position is a Retry) and
its `__mutant` (asserts the old KeyNotFound behaviour — fails, 6/8 goals).
`tools/gen_drivers.py` carries the matching entry (its output for the new
row is byte-identical to the hand-applied `src/drivers.rs`).

## Scope beyond the finding

None in the shipped code. In the mirror, `lookup_retry` gained a ghost
(second return component) retry count — needed to make "retried up to five
times" observable to the new driver; its control flow is unchanged.

## Risk

Behaviour change: an entry whose lookup still fails after five retries now
returns `Err(Timeout)`/`Err(IoError)` instead of `Err(KeyNotFound)`. Callers
that matched on `KeyNotFound` to mean "absent" will now see an error — which
is the spec's intent (a live key is not absent) and matches `check`'s
established behaviour. The vLLM connector's miss path (`certus_lookup_misses_total`)
no longer receives these keys, so hit-rate accounting improves. No test in
the crate relied on the old behaviour (full suite passes five consecutive
runs: 143 + 5 + 14 tests). Note: `cargo clippy -p dispatcher` fails inside
the `gpu-services` dependency built with the spdk feature — verified
identical on the untouched HEAD, i.e. pre-existing and outside this
component's scope; `cargo test -p dispatcher` (the acceptance criterion) is
unaffected.

## Follow-ups

- `lookup_async`/`lookup` (single-key path, `src/lib.rs:3368`) still collapses
  a dispatch-map `Err` into `KeyNotFound` with no retry — same bug class,
  different property bundle (DIS-LOOKUP-DM-ERROR-KEY-NOT-FOUND currently
  *specifies* that behaviour, so changing it needs a spec decision first).
- The Creusot mirror's `check` model (`methods.rs`) still carries the
  pre-FR-008 body (`Err(_) => Ok(false)`); `refute_dis_check_present`
  accordingly still demonstrates the old bug. Left alone here: it is not on
  this finding's path and reworking it would touch modules this repair must
  keep proving.
- The `serve_concurrently_promoted` mirror models the racing lookup with an
  oracle (`dm_lookup_racing`); the "writer settles within the budget is then
  served warm" direction of this property could be proven with the same
  technique if `bl_classify` is ever given a racing variant — covered today
  by the repair test.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-BATCH-LOOKUP-TRANSIENT-RETRY`, callee modules `batch_lookup,lookup_retry`.

```
ACCEPT-REPO: base 850a00403ce3a8ae4815c3abcb59f5d7f264e99f; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (162 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-BATCH-LOOKUP-TRANSIENT-RETRY yes
  E. (before side for dispatcher reused from cache: same base, proof crate and gate)
  E. REVERIFY: dispatcher yes (112 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 162 passed in each run
  dispatcher                   | DIS-BATCH-LOOKUP-TRANSIENT-RETRY         | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 112 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 34 min | 115 min | accepted |

Total wall-clock: 149 min.
