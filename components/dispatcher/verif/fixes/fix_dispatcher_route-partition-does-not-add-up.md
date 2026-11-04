# fix_dispatcher_route-partition-does-not-add-up

> **In short** - Problem: single-key `lookup`/`lookup_async` serves keys but never records `lookup_hits_dram`/`lookup_hits_ssd`, so over intervals with single-key hits, dram+ssd+remote < served keys. Fix: record local hits in `lookup_async`'s warm (DRAM) and cold (SSD, recovery→DRAM) arms.


```
component:      dispatcher
finding:        route-partition-does-not-add-up
property:       DIS-TIER-STATS-ROUTE-PARTITION
found by:       see "How it was found"
base commit:    b7d4e9b1f4f6        branch: repair/j35-rc23-20261103
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
Over any interval, the increase of lookup_hits_dram plus lookup_hits_ssd plus remote_lookup_hits equals the number of keys served by lookups in that interval, and matches the per-key attributions returned for the same batches.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:405`
```
- **FR-058** *(Sync 2026-08-31 — backfilled; the tier-event telemetry method shipped with no requirement)*: The `IDispatcher` interface MUST provide `tier_event_stats() -> TierEventStats` returning a cumulative snapshot of KV-cache tier-movement counters. The counters are backed by a lock-free `TierEventCounters` subsystem (relaxed-atomic increments on the tier-transition paths) and are **always populated** — unlike the telemetry-gated `read_write_stats` (FR-056) — so implementations that do no tiering (e.g. remote-lookup, p2p, or mock dispatchers) return zeroed counters. `TierEventStats` is a `Copy` struct of **sixteen** `u64` fields. All but the two sampling gauges named below are monotonic since process start (take the delta of two successive snapshots for a per-interval rate): `promotions_to_memory` (blocks promoted SSD→DRAM across all promote paths), `promotions_to_gpu` (lookups served up to the GPU, one per successfully-served key, memory-tier or SSD source), `evictions_from_memory` (DRAM memory-tier entries demoted-to-SSD or dropped, by either the foreground clean-eviction path or the background evictor), `evictions_from_ssd` (extents freed on SSD by the background extent evictor), and *(Sync 2026-09-09 — code authoritative, commits `39ffee9b`/`c400ced8`)* `store_backpressure_events` (store-allocation retries taken while backpressuring on a momentarily-full memory tier — a nonzero value means the tier saturated and the store path waited for the evictor to catch up rather than failing; see FR-060) and `store_drops_on_full` (stores dropped best-effort because the tier was still full after the backpressure budget — the block is not cached and a later load recomputes, but the store reports success so a full cache is never fatal to the offloading connector; see FR-060). Also *(Sync 2026-09-28 — CODE CHANGE, commit `9416c997`)* `remote_lookup_hits` (keys a peer actually returned data for, counted requester-side when the remote fetch succeeds — **not** when the block is subsequently delivered to the GPU, so a delivery failure after a successful peer fetch still counts as a remote hit) and `remote_lookup_misses` (keys forwarded to remote lookup that no peer held). A remote `TransportError` is **neither** a hit nor a miss: it is a fault, and folding it into either would make a broken fabric read as a cold cache. Both are zero on any implementation with no `IRemoteLookup` receptacle bound, which is indistinguishable from a bound receptacle that was never asked — absence of remote traffic, not evidence that remote lookup is unwired. Also *(Sync 2026-09-29 — CODE CHANGE, served_by Phase 2/3)* `lookup_hits_dram` and `lookup_hits_ssd`, which with `remote_lookup_hits` **partition** every served key by route: the three MUST sum to the served count for any interval. They are recorded where the dispatcher derives its per-key `ServedBy`, so they cannot disagree with the attribution returned to the caller for the same batch. Route, not residency: an SSD hit that is promoted into DRAM as part of being served counts as `lookup_hits_ssd`, because that is what the request cost. Also *(Sync 2026-10-05 — CODE CHANGE)* six fields taking the struct from ten to **sixteen**, each specified normatively elsewhere and enumerated here so this inventory stays the complete list: `store_already_resident` (reserves refused because the key was already cached rather than for want of space — interfaces `001-interfaces` FR-018a); `evictions_blocked_by_pin` and `evictions_blocked_unpersisted` (which of the two causes refused a clean eviction, counted per candidate examined — spec `002-served-by-tier-attribution` FR-033); `eviction_scans_exhausted` (scans that examined every candidate and freed none, the event that becomes a declined store — FR-034); and the two **sampling gauges** `oldest_sampled` and `oldest_persisted` (FR-036), which are **not monotonic** — each is overwritten with the latest sample, so differencing two snapshots is meaningless and the server publishes both as Prometheus gauges. `dispatcher-p2p` reports none of the four eviction/scan fields and its zeroes there are to be read as *unmeasured*, not as "never blocked" (FR-035).

- **FR-059** *(Sync 2026-09-01 — backfilled; the batch-populate method shipped with no requirement)*: The `IDispatcher` interface MUST provide `batch_populate(entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>`. Semantically equivalent to calling `populate` for each entry, but amortizes GPU stream synchronization across the entire batch: (1) resolves the batch device and picks the dedicated `store` stream (FR-037), (2) for each entry, calls `reserve_memory` to allocate a memory-tier slot and immediately issues `copy_gpu_to_memory_async` on the `store` stream — interleaving allocation with async D2H DMA so early transfers overlap with later allocations, (3) issues a single `stream_synchronize` for the batch, guaranteeing all D2H copies have landed, (4) calls `copy_gpu_to_memory_completed` for each successfully-copied entry to register it in the dispatch-map and enqueue SSD write-through. Returns one `Result` per input entry, in the same order as `entries`. Entries that fail allocation or DMA do not prevent other entries from succeeding. An entry whose D2H copy fails, or whose batch `stream_synchronize` fails, has its reserved slot released (`release_memory`) before its error is reported, as in `populate` (FR-003). *(Sync 2026-09-09 — code authoritative, commit `c400ced8`: like `populate`, an entry whose `reserve_memory` returns `AllocationFailed` after the store-backpressure budget is best-effort **dropped** — its result is `Ok(())` and the block is left uncached (a later load recomputes) rather than failing the entry. See FR-060.)*
```

### The code: before (-) and after (+)
```diff
@@ -82,4 +82,8 @@ mod repair_test_failed_promotion_releases_slot;
 mod repair_test_eviction_errors_counted_unpersisted;
 
+#[cfg(test)]
+#[path = "repair_test_route_partition_does_not_add_up.rs"]
+mod repair_test_route_partition_does_not_add_up;
+
 use parking_lot::RwLock;
 use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
@@ -3448,4 +3452,11 @@ impl IDispatcher for DispatcherComponent {
                         if r.is_ok() {
                             self.tier_counters.record_promotion_to_gpu();
+                            // The local half of the route partition (FR-058,
+                            // SC-017): a memory-tier serve is a DRAM-route hit,
+                            // recorded where this path derives its route so the
+                            // counters cannot disagree with the per-key
+                            // `ServedBy` the batch path attributes to the same
+                            // hit.
+                            self.tier_counters.record_local_hits(1, 0);
                         }
                         r.map(|()| GpuStream(stream_raw as *mut std::ffi::c_void))
@@ -3456,4 +3467,5 @@ impl IDispatcher for DispatcherComponent {
                         // entry, or freeing the fresh slot, while the SSD read and the
                         // in-place promotion are still running.
+                        let mut recovered_warm = false;
                         let r = match self.promote_and_serve(
                             key,
@@ -3468,6 +3480,7 @@ impl IDispatcher for DispatcherComponent {
                             // race: a hit, not a failure — serve it warm once that
                             // promotion lands, as batch_lookup's recovery pass does.
-                            Err(DispatcherError::AlreadyExists(_)) => self
-                                .serve_concurrently_promoted(
+                            Err(DispatcherError::AlreadyExists(_)) => {
+                                recovered_warm = true;
+                                self.serve_concurrently_promoted(
                                     &dm,
                                     &gpu,
@@ -3475,5 +3488,6 @@ impl IDispatcher for DispatcherComponent {
                                     std::slice::from_ref(&ipc_handle),
                                     warm_raw,
-                                ),
+                                )
+                            }
                             r => r,
                         };
@@ -3481,4 +3495,16 @@ impl IDispatcher for DispatcherComponent {
                         r?;
                         self.tier_counters.record_promotion_to_gpu();
+                        // The local half of the route partition (FR-058, SC-017),
+                        // with the same taxonomy batch_lookup derives: a cold
+                        // serve read a data drive (SSD route), but the
+                        // concurrent-promotion recovery served the key out of
+                        // DRAM after the winner's promotion (DRAM route, no
+                        // drive read) — the two must not drift apart, or the
+                        // same hit would be counted differently by the two
+                        // lookup paths.
+                        self.tier_counters.record_local_hits(
+                            u64::from(recovered_warm),
+                            u64::from(!recovered_warm),
+                        );
... (2 more lines in the commit)
```

### The test that shows the bug: `components/dispatcher/src/repair_test_route_partition_does_not_add_up.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `route-partition-does-not-add-up`
//! (DIS-TIER-STATS-ROUTE-PARTITION).
//!
//! FR-058 / SC-017: `lookup_hits_dram + lookup_hits_ssd + remote_lookup_hits`
//! partitions every served key by route — over any interval the three MUST sum
//! to the number of keys the lookups served in that interval, matching the
//! per-key `ServedBy` attributions the same batches return.
//!
//! `batch_lookup` records its half of the partition (derived from the per-key
//! attributions), but the single-key path `lookup`/`lookup_async` serves keys
//! (it bumps `promotions_to_gpu`) without ever bumping `lookup_hits_dram` /
//! `lookup_hits_ssd`. On the unfixed code, any interval containing a
//! successful single-key lookup undercounts the partition, so the route
//! counters and the served count disagree.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    IpcHandle, LookupOutcome, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, ServedBy,
    TierEventStats,
};

use crate::DispatcherComponent;

// --- dispatch map: fixed locations, with an optional flip to a memory-tier
// --- location after N reads (the concurrent-promotion recovery scenario) ---

#[derive(Clone, Copy)]
enum Loc {
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory(usize, u32),
    Block(u64),
}
... (753 more lines)
```


Traces: FR-058, SC-017, idispatcher.rs TierEventStats.lookup_hits_dram, interfaces/src/idispatcher.rs:323, dispatcher/src/lib.rs:179, dispatcher/src/lib.rs:2988

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem — route-partition-does-not-add-up (DIS-TIER-STATS-ROUTE-PARTITION)

**One sentence.** When a key is served by a *single-key* lookup (`lookup` /
`lookup_async`), the dispatcher bumps `promotions_to_gpu` but never bumps
`lookup_hits_dram` / `lookup_hits_ssd`, so the three route counters no longer add
up to the number of keys the lookups served.

**Spec.** `TierEventStats.lookup_hits_dram` / `lookup_hits_ssd`
(`components/interfaces/src/idispatcher.rs:344-357`, FR-058, SC-017 in
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`): with
`remote_lookup_hits` the two counters *"partition every served key by route, so
the three sum to the served count"*, and SC-017 makes that an interval property —
*"lookup_hits_dram + lookup_hits_ssd + remote_lookup_hits MUST equal the
served-key count over the same interval"*, where the served-key count includes
single-key lookups (SC-017: "after a populate/lookup/evict cycle,
`promotions_to_gpu` reflects served lookups" — the single-key path does move
that counter).

**Code.** The route partition is recorded in exactly one place,
`components/dispatcher/src/lib.rs:3354-3360` (end of `batch_lookup`):
`record_local_hits(dram, ssd)` derived from the per-key attributions of that
batch. The single-key path `lookup_async`
(`components/dispatcher/src/lib.rs:3364-3489`) derives the same route
information — warm `MemoryTier` serve (lib.rs:3438-3451, DRAM route), cold
`BlockDevice` promote (lib.rs:3453-3484, SSD route), concurrent-promotion
recovery (lib.rs:3470-3477, DRAM route, the same taxonomy `batch_lookup` uses
at lib.rs:3307-3316) — and records `record_promotion_to_gpu()` at lib.rs:3449
and lib.rs:3482, but no `record_local_hits` anywhere. So over any interval that
contains a successful single-key lookup, `dram + ssd + remote` increases by less
than the number of served keys (the partition undercounts by exactly the
single-key hits), while `promotions_to_gpu` counts them — the two aggregate
views of the same traffic disagree.

**Verdict: code wrong.** The spec's partition is stated over every served key
and any interval; the code implements it only for the batch path. The fix is to
record the local route partition on the single-key path, at the same points
where it already records `promotions_to_gpu`, using the same route taxonomy the
batch path derives (warm hit → DRAM, cold promote → SSD, concurrent-promotion
recovery → DRAM).

A note on what is *not* wrong: `remote_lookup_hits` counting a peer-answered key
even when the later local GPU delivery fails (lib.rs:3206-3209) is deliberate,
spec-sanctioned behavior (FR-058, DIS-TIER-STATS-REMOTE-HIT-ON-FETCH) and is not
touched by this repair.

# Repair report: route-partition-does-not-add-up (DIS-TIER-STATS-ROUTE-PARTITION)

## In short

Problem: single-key `lookup`/`lookup_async` serves keys but never records `lookup_hits_dram`/`lookup_hits_ssd`, so over intervals with single-key hits, dram+ssd+remote < served keys.
Fix: record local hits in `lookup_async`'s warm (DRAM) and cold (SSD, recovery→DRAM) arms.

## Bug

FR-058 / SC-017 (interfaces/src/idispatcher.rs:344-357, TierEventStats docs) require that over
any interval, the increase of `lookup_hits_dram + lookup_hits_ssd + remote_lookup_hits` equals
the number of keys served by lookups in that interval, and matches the per-key `ServedBy`
attributions returned for the same batches.

The code implemented this partition only in `batch_lookup` (src/lib.rs:3354-3360, now moved
per lookup outcome). The single-key `lookup`/`lookup_async` path served keys — bumping
`promotions_to_gpu` at the old lib.rs:3449/3482 — but never called `record_local_hits`, so a
warm hit moved no route counter at all and a cold hit moved no route counter either. Over any
interval containing single-key hits, the route sum under-counted the served keys, contradicting
the property and the discordance D-FR-058-idispatcher.rs-c69d68.

The concurrent-promotion recovery arm (cold lookup whose map entry flips to `Loc::Memory` via
`AlreadyExists`) was additionally unattributed anywhere.

## Impact

Monitoring/telemetry only. `tier_event_stats()` consumers computing hit-rate-by-route
(DRAM vs SSD vs remote) from single-key `lookup` traffic read `0` for routes that actually
served traffic; per-key `ServedBy` attributions returned by `lookup` disagreed with the
counters for the same key. No data-path, correctness, or persistence effect; counters are
advisory u64 atomics. Affected any deployment issuing single-key lookups (the GPU-friendly
API) while reading tier stats.

## How to reproduce

1. `cd /home/cornel/repair-j35-rc23`
2. `cargo test -p dispatcher repair_test_route_partition_does_not_add_up` — on the unfixed
   code, four assertions fail with `left: 0, right: 1` (warm, cold, recovery, mixed-interval
   tests); after the fix all 5 tests pass.
3. The mixed-interval test reproduces the exact property shape: warm single-key lookup plus a
   `[warm, miss]` batch, asserting `Δ(dram+ssd+remote) == served == Δpromotions_to_gpu` and
   `Δdram == batch Dram attributions + 1`.

## Fix

`components/dispatcher/src/lib.rs`, `lookup_async`:

- Warm arm (MemoryTier serve, after `mt.touch`): on `is_ok`, `record_local_hits(1, 0)` next to
  the existing `record_promotion_to_gpu()` — the serve came through the DRAM route.
- Cold arm (BlockDevice serve): track `recovered_warm` when the concurrent-promotion
  `AlreadyExists` recovery serves from the memory tier; after the successful promote,
  `record_local_hits(u64::from(recovered_warm), u64::from(!recovered_warm))`. Recovery is a
  DRAM-route hit (matches `batch_lookup`'s taxonomy from repair a7ccc6e48 and the counter docs:
  route, not residency); a plain cold serve is an SSD-route hit.

Tests: `src/repair_test_route_partition_does_not_add_up.rs` (5 tests, self-contained mocks,
`flip_at` models concurrent promotion), wired via `#[cfg(test)] #[path = ...] mod ...;` in
lib.rs. Verified red-first: 4 failing assertions on unfixed code; 5/5 pass after. Full
`cargo test -p dispatcher` run 5 times, all green, no flakes (165 lib + 5 + 14 tests).

Creusot (`verif-creusot/src/`):

- `lookup.rs`: new `rd_ctr_hits` relation (route hit counters may increase, everything else as
  `rd_ctr`); `lookup_async`'s general ensures switched to it, plus a new ensures pinning the
  exact `hit_dram`/`hit_ssd` delta per `LPath`; model body records `rec_local_hits` in the
  warm and cold Ok arms.
- `batch.rs`: two new Pearlite lemmas, `count_sb_none` (no outcome carries `x` ⇒ `count_sb == 0`)
  and `count_ok_route_split` (Ok ⟺ served_by ∈ {Dram,Ssd,Remote} ⇒ `count_ok == Dram+Ssd+Remote`).
- `drivers.rs`: `verify_dis_tier_stats_route_partition` proves Δ(hit_dram+hit_ssd+r_hit) ==
  served-count across a single-key `lookup_async` followed by a `batch_lookup`, tied to the
  per-key `ServedBy` attributions via the new lemmas. **PROVED.** Its `__mutant` (asserts an
  unconditional +1 after `lookup_async`, false on a Miss) **FAILS**, as required.
- Callee modules re-proved with the final code: `lookup_async` PROVED, `batch_lookup` PROVED,
  `lookup` PROVED, plus `verify_dis_tier_stats_promotions_to_gpu` re-proved.

Rejected alternatives:

- Counting the concurrent-promotion recovery as SSD (route the counter by where the promote
  targeted rather than where the serve came from) — rejected: contradicts `batch_lookup`'s
  accepted per-key attribution (a7ccc6e48) and the "route, not residency" counter docs; the
  partition must match the per-key attributions, so both paths must agree.
- Moving all recording into `tier_event_stats()` read time or into `record_promotion_to_gpu`
  — rejected: promotion counts and route hits are separate properties (DIS-TIER-STATS-
  PROMOTIONS-TO-GPU already covers the former); conflating them would break that property's
  driver.
- Also "fixing" remote-on-fetch counting (remote hit counted when the peer answers even if
  local delivery fails) — rejected: that behavior is spec-sanctioned (FR-058, covered by
  DIS-TIER-STATS-REMOTE-HIT-ON-FETCH / DIS-TIER-STATS-REMOTE-ACCOUNTING) and out of scope;
  the new driver therefore drives its batch half with `!has_rl` and a comment explaining why.

## Scope beyond the finding

None. The fix touches only `lookup_async`'s two serve arms and its verification model. The
pre-existing abstraction gap where the `lookup_async` model folds the `AlreadyExists`
recovery arm into the cold path (documented in the model comment) is unchanged, and the
pre-existing proof failures listed under Follow-ups were left as-is.

## Risk

Low. Behavior change: `lookup_hits_dram`/`lookup_hits_ssd` now move on the single-key path —
dashboards reading them see correct, non-zero values (an improvement, but a visible change if
anyone alert-ed on the old zeros). No API, ABI, on-disk, or protocol change; counters are
advisory atomics with no ordering requirements beyond what `record_local_hits` already had.
`batch_lookup` counting is untouched. All 184 crate tests green across 5 runs; broad Creusot
re-verification of 76 affected modules shows no regression caused by this change (failing
modules were re-checked against the pre-change tree and fail identically there).

## Follow-ups

- Pre-existing unproved modules on this branch, confirmed failing identically on the
  pre-change tree (not caused by this repair): verify drivers
  `verify_dis_inv_not_initialized`, `verify_dis_lookup_after_downgrade`,
  `verify_dis_lookup_cold_no_slot`, `verify_dis_lookup_cold_staging_fallback`,
  `verify_dis_lookup_miss_key_not_found`, `verify_dis_lookup_warm_copies_data`; refute
  drivers `refute_dis_inv_eviction_demoted_still_readable`,
  `refute_dis_lookup_cold_failure_releases_slot`, `refute_dis_lookup_cold_promotes`,
  `refute_dis_lookup_no_foreign_write_release`. Likely branch degradation from earlier
  accepted repairs and/or machine load (a concurrent session runs full-suite proofs);
  worth a dedicated re-baselining pass.
- The `lookup_async` Creusot model could gain an explicit `AlreadyExists`-recovery arm
  (currently folded into Cold) now that the real code distinguishes the two routes.
- The spec-internal tension between FR-058's interval partition (remote counted only when
  the key is actually served) and remote-on-fetch (remote counted at peer-answer time) is
  documented in the new driver comment; a spec clarification would let a future driver drop
  the `!has_rl` precondition.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-TIER-STATS-ROUTE-PARTITION`, callee modules `lookup_async,batch_lookup`.

```
ACCEPT-REPO: base b7d4e9b1f4f60637662bf6b97997ce096f8348e1; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (184 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-TIER-STATS-ROUTE-PARTITION yes
  E. REVERIFY: dispatcher yes (114 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 184 passed in each run
  dispatcher                   | DIS-TIER-STATS-ROUTE-PARTITION           | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 114 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 289 min | 437 min | accepted |

Total wall-clock: 726 min.
