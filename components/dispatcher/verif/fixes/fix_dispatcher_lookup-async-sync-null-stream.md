# fix_dispatcher_lookup-async-sync-null-stream

> **In short** - Problem: `lookup_async`'s memory-tier hit synchronized its H2D copy internally and always returned the null stream; an unknown destination device degraded to the synchronous DmaBuffer path instead of the shared warm stream. Fix: queue the copy unsynchronized on the destination device's warm stream (shared `warm_stream` only when the device is unknown) and return that stream; the sync `lookup` wrapper synchronizes it.


```
component:      dispatcher
finding:        lookup-async-sync-null-stream
property:       DIS-LOOKUP-ASYNC-WARM-STREAM,DIS-LOOKUP-ASYNC-DEVICE-STREAM
found by:       see "How it was found"
base commit:    850a00403ce3        branch: repair/j33-rc24-20261030
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
- DIS-LOOKUP-ASYNC-WARM-STREAM: For a memory-tier hit, lookup_async returns the GPU stream on which the copy was queued, and after that stream is synchronized the caller's destination holds the cached data.
- DIS-LOOKUP-ASYNC-DEVICE-STREAM: When the destination GPU device is known, lookup_async issues the copy on a stream bound to the device that owns the destination pointer, and falls back to the shared warm stream only when the device is unknown.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:362`
```
- **FR-036**: System MUST provide a `lookup_async(key, ipc_handle) -> Result<GpuStream, DispatcherError>` method on the `IDispatcher` interface. This method performs the same cache lookup as `lookup` (dispatch-map query, memory-tier hit, BlockDevice promotion) but returns a `GpuStream` handle instead of blocking on the H2D DMA transfer. The caller MUST call `stream_synchronize` on the returned stream before accessing the GPU destination memory. For SSD promotion paths the copy completes synchronously and a null stream is returned. *(Sync 2026-10-14 — code authoritative, commit `1d24ff27f`:)* On a BlockDevice hit, `lookup_async` MUST hold the read pin its `dm.lookup` took until `promote_and_serve` has returned, and release it only then. That pin is what stops a concurrent `remove` or SSD eviction from deleting the entry, or freeing the freshly inserted memory-tier slot, while the SSD read, the GPU copy and the in-place `promote_block_to_memory_tier` are still running. *(Previously the pin was released before promotion started.)* If `promote_and_serve` fails with `AlreadyExists` (a concurrent cold lookup of the same key won the `mt.insert` race), `lookup_async` MUST treat it as a hit and serve the winner's slot via `serve_concurrently_promoted` (FR-051), not fail the load. The synchronous `lookup` method MUST delegate to `lookup_async` internally and call `stream_synchronize` before returning.
- **FR-037**: The dispatcher MUST pre-allocate per-device CUDA streams for the memory-tier warm paths. Each GPU device in the process-global `DEVICE_STREAMS` map is given **two** named streams — `warm` and `store` — plus a pipeline stream pair (FR-052). The `warm` stream carries memory-tier→GPU (H2D) copies for the lookup hot path (`lookup_async`, `batch_lookup`, FR-006 / FR-039) via per-region `memcpy_h2d_async`. The `store` stream is a dedicated GPU→memory-tier (D2H) stream used by both `batch_populate` (FR-059) and the single-entry `populate_from_gpu` path (FR-056), enabling H2D and D2H DMA to overlap on the PCIe bus via the GPU's two copy engines when a populate runs concurrently with lookups. *(Sync 2026-09-02 — code authoritative, commit `9b83ebe1`: `populate_from_gpu` was moved from the `warm` stream to the dedicated `store` stream so its D2H copy does not serialize behind concurrent H2D lookups on `warm` — PCIe full-duplex. The earlier rationale that a single-entry populate has no concurrent H2D to overlap with held only in isolation; under a live workload lookups run on `warm` at the same time.)* A single `AtomicU64` warm stream (`warm_stream`) is retained as a fallback; the warm memory-tier hot path and the D2H path resolve the destination/source GPU device from the client's IPC pointer and select the per-device streams from `DEVICE_STREAMS` (see FR-052). A CUDA stream is bound to the device that was current at creation, so a stream on another GPU makes `cudaMemcpyAsync` fail with "invalid argument" under multi-GPU / tensor parallelism; per-device streams avoid this. The fallback `warm_stream` and the `PipelineRing` streams MUST be destroyed during `shutdown()`. The per-device `DEVICE_STREAMS` entries are not destroyed by `shutdown()`. They are process-global, created lazily per device and deliberately kept until process exit, because one dispatcher serves all local GPUs and CUDA streams are process-wide resources. *(Sync 2026-10-14 — code authoritative, `src/lib.rs:453-455`, `2151-2161`.)*
```
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:378`
```
- **FR-052**: Cold (SSD→GPU) and warm (memory-tier→GPU) DMA transfers MUST use CUDA streams bound to the destination GPU's device. Because `cudaMemcpyAsync` rejects a peer pointer on a different device than its stream ("invalid argument") under multi-GPU / tensor parallelism, the dispatcher maintains per-device streams in a process-global `DEVICE_STREAMS` map — two warm streams (`warm` for H2D and `store` for D2H, FR-037) plus one pipeline stream pair per device, created lazily on the target device via `device_streams_for`. It resolves each request's device from its IPC destination pointer (`IGpuServices::device_of_ptr` / `set_batch_device`), makes that device current on the issuing thread, and selects the streams bound to it. `batch_lookup` resolves one device for the whole batch (all entries in an RPC come from a single rank) and passes that device's pipeline stream pair (FR-052 above) into the caller-thread `scatter_gather_multi_drive_zero_copy` (FR-061), so every drive's coalesced H2D issues on streams bound to the destination GPU *(Sync 2026-09-21, commit `6f35b13a`; the prior `cold_pool::ColdReadRequest { gpu_device: i32 }` mechanism, where each pool worker called `IGpuServices::set_device`, was removed with the `ColdReadPool`)*. When the device is unknown (`-1`) the paths fall back to the shared pipeline-ring / `warm_stream` streams.
- **FR-053**: The dispatcher MUST provide a bounded cold-load staging pool (`StagingPool`) of `cold_staging_slots` pre-registered, CUDA-pinned + SPDK-registered host DRAM buffers of `cold_staging_buf_bytes` each, leased via an RAII `StagingLease` that returns the buffer on drop. When a cold (BlockDevice) load cannot obtain a memory-tier slot because the tier is saturated (`evict_for_space` / `evict_and_insert` returns `AllocationFailed` and at least one data drive is configured), the dispatcher MUST serve the read through a staging buffer (`SSD → staging → GPU` via `serve_cold_staged`) and leave the dispatch-map entry as `BlockDevice` (served uncached, NOT promoted) rather than failing the load. In `batch_lookup` such entries are deferred to a post-pass that runs after the pooled tier jobs and serves them one lease at a time while holding no other lock, so the blocking `checkout` is deadlock-free. This bounds staging memory so a burst of concurrent cold reads cannot exhaust the memory tier and cause fatal `AllocationFailed`. Setting `cold_staging_slots` to 0 disables staging. (Config fields: see FR-033.)
```

### The code: before (-) and after (+)
```diff
@@ -3316,5 +3316,25 @@ impl IDispatcher for DispatcherComponent {
                             }
                         }
-                        // Synchronous serve via the shared, device-correct helper.
+                        // Resolve the stream the copy will be queued on: the warm
+                        // stream of the destination's device when that device is
+                        // known (FR-052 — a stream on another GPU fails
+                        // cudaMemcpyAsync under multi-GPU), and the shared
+                        // `warm_stream` fallback when it is not (FR-037). A known
+                        // device whose per-device stream setup failed (warm_raw ==
+                        // 0) deliberately does NOT take the shared fallback — that
+                        // stream may be bound to another GPU — but degrades to
+                        // stream 0: the helper's `DmaBuffer` branch then copies
+                        // synchronously and the null stream is the honest answer,
+                        // as on the SSD path.
+                        let stream_raw = if device >= 0 {
+                            warm_raw
+                        } else {
+                            self.warm_stream.load(Ordering::Acquire)
+                        };
+                        // Queue the H2D copy WITHOUT synchronizing and hand the
+                        // caller the stream it was queued on: the caller must
+                        // `stream_synchronize` that stream before touching the
+                        // destination (FR-036). The synchronous `lookup` wrapper
+                        // delegates here and performs that sync before returning.
                         let r = self.serve_memory_tier_to_gpu(
                             &gpu,
@@ -3322,6 +3342,6 @@ impl IDispatcher for DispatcherComponent {
                             size,
                             std::slice::from_ref(&ipc_handle),
-                            warm_raw,
-                            true,
+                            stream_raw,
+                            false,
                         );
                         let _ = dm.release_read(key);
@@ -3330,5 +3350,5 @@ impl IDispatcher for DispatcherComponent {
                             self.tier_counters.record_promotion_to_gpu();
                         }
-                        r.map(|()| null_stream)
+                        r.map(|()| GpuStream(stream_raw as *mut std::ffi::c_void))
                     }
                     LookupResult::BlockDevice { offset } => {
@@ -8542,2 +8562,6 @@ mod repair_test_duplicate_check_after_eviction;
 #[path = "repair_test_check_present_map_timeout.rs"]
 mod repair_test_check_present_map_timeout;
+
+#[cfg(test)]
+#[path = "repair_test_lookup_async_stream.rs"]
+mod repair_test_lookup_async_stream;
```

### The test that shows the bug: `components/dispatcher/src/repair_test_lookup_async_stream.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `lookup-async-sync-null-stream`
//! (DIS-LOOKUP-ASYNC-WARM-STREAM, DIS-LOOKUP-ASYNC-DEVICE-STREAM).
//!
//! For a memory-tier (warm) hit, `lookup_async` must queue the H2D copy
//! asynchronously and return the CUDA stream it was queued on, so the caller
//! can `stream_synchronize` that stream before touching the destination
//! (spec 001 FR-036, `IDispatcher::lookup_async` doc). When the destination's
//! GPU device is known, the copy goes out on a stream bound to that device
//! (the per-device warm stream, FR-037/FR-052); only when the device is
//! unknown does it fall back to the shared warm stream.
//!
//! On the unfixed code the warm hit is copied synchronously inside the call
//! and a null stream is returned, and an unknown device degrades to the fully
//! synchronous `DmaBuffer` path instead of the shared warm stream.
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

// --- dispatch map: entries are fixed memory-tier locations for this test ---

#[derive(Clone, Copy)]
enum Loc {
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory(usize, u32),
}

struct Ent {
    loc: Loc,
... (644 more lines)
```


Traces: FR-036, FR-037, idispatcher.rs lookup_async, dispatcher/src/lib.rs:3061, dispatcher/src/lib.rs:3067, interfaces/src/idispatcher.rs:550, FR-052

## How it was found
- creusot: DIS-LOOKUP-ASYNC-WARM-STREAM: unscored; DIS-LOOKUP-ASYNC-DEVICE-STREAM: unscored -  | 
- kani: DIS-LOOKUP-ASYNC-WARM-STREAM: unscored; DIS-LOOKUP-ASYNC-DEVICE-STREAM: unscored -  | 

## The problem (agent's statement and verdict)
# Problem — finding `lookup-async-sync-null-stream`

**One sentence (plain):** when a requested block is cached in DRAM, `lookup_async` copies it to the
caller's GPU *synchronously* (blocking the caller) and then hands back a null stream, instead of
queueing the copy asynchronously and returning the stream it was queued on — and when the caller's
GPU device cannot be determined it does a fully synchronous copy instead of falling back to the
shared warm stream.

## Spec vs code

**Properties violated** (one root cause):

- **DIS-LOOKUP-ASYNC-WARM-STREAM** — for a memory-tier hit, `lookup_async` returns the GPU stream on
  which the copy was queued, and after that stream is synchronized the caller's destination holds
  the cached data.
- **DIS-LOOKUP-ASYNC-DEVICE-STREAM** — when the destination GPU device is known, `lookup_async`
  issues the copy on a stream bound to the device that owns the destination pointer, and falls back
  to the shared warm stream only when the device is unknown.

**Spec says** (`specs/001-dispatcher-cache-interface/spec.md`):

- **FR-036** (spec.md:362): "`lookup_async(key, ipc_handle) -> Result<GpuStream, DispatcherError>`
  … returns a `GpuStream` handle instead of blocking on the H2D DMA transfer. The caller MUST call
  `stream_synchronize` on the returned stream before accessing the GPU destination memory. For SSD
  promotion paths the copy completes synchronously and a null stream is returned. … The synchronous
  `lookup` method MUST delegate to `lookup_async` internally and call `stream_synchronize` before
  returning."
- **FR-037** (spec.md:363): "A single `AtomicU64` warm stream (`warm_stream`) is retained as a
  fallback …".
- **FR-052** (spec.md:378): "When the device is unknown (`-1`) the paths fall back to the shared
  pipeline-ring / `warm_stream` streams."
- Interface doc (`components/interfaces/src/idispatcher.rs:568-573`): "Returns the CUDA stream the
  copy was issued on. The caller must call `stream_synchronize` on the returned stream before
  accessing the GPU destination memory."

**Code does** (`components/dispatcher/src/lib.rs`, `lookup_async`, MemoryTier arm):

- `lib.rs:3319-3326` — the warm hit is served via
  `serve_memory_tier_to_gpu(…, warm_raw, /* synchronize: */ true)`: the copy is queued on the
  per-device warm stream but then *synchronized inside the call*, so the H2D DMA blocks
  `lookup_async` — exactly what FR-036 says it must not do.
- `lib.rs:3332` — `r.map(|()| null_stream)`: a null stream is returned even though the copy ran on
  a real (non-null) stream, so the caller cannot know which stream to synchronize and the
  "returns the stream the copy was issued on" contract is broken (DIS-LOOKUP-ASYNC-WARM-STREAM).
- `lib.rs:3293-3294` — when the destination device is unknown, `device_streams_for` returns `None`
  so `warm_raw == 0`, and `serve_memory_tier_to_gpu` then takes the fully synchronous
  `DmaBuffer`/`dma_copy_to_device` path (lib.rs:1451-1485) instead of falling back to the shared
  `warm_stream` retained for exactly that case (FR-037/FR-052,
  DIS-LOOKUP-ASYNC-DEVICE-STREAM).

The delegating synchronous `lookup` (lib.rs:2408-2419) already handles a non-null returned stream
(`if !stream.0.is_null() { stream_synchronize }`), so it was built for the specified behavior —
`lookup_async` simply never returned a stream.

## Verdict

**Code wrong.** The interface, FR-036, FR-037 and FR-052 all describe an asynchronous warm path
that returns the queueing stream; the implementation synchronizes internally, returns a null
stream, and skips the shared-warm-stream fallback. One root cause: the MemoryTier arm of
`lookup_async` resolves no "stream to hand to the caller" — it must queue the copy asynchronously
on the per-device warm stream (device known) or the shared `warm_stream` (device unknown) and
return that stream.

# Repair report: `lookup-async-sync-null-stream`

## In short

Problem: `lookup_async`'s memory-tier hit synchronized its H2D copy internally and always returned the null stream; an unknown destination device degraded to the synchronous DmaBuffer path instead of the shared warm stream.
Fix: queue the copy unsynchronized on the destination device's warm stream (shared `warm_stream` only when the device is unknown) and return that stream; the sync `lookup` wrapper synchronizes it.

## Bug

`components/dispatcher/src/lib.rs`, `lookup_async`, `LookupResult::MemoryTier` arm (baseline lines 3318–3332):

1. The warm copy was served with `synchronize: true` and the result was mapped to
   `r.map(|()| null_stream)`. The caller therefore never received the stream the
   copy was queued on and could not perform the FR-036 contract
   ("`lookup_async` returns the GPU stream on which the copy was queued; after
   synchronizing that stream the destination holds the cached data"). The
   synchronous `lookup` wrapper (`src/lib.rs:2408`) already skips synchronization
   for null streams, so the whole async path was effectively synchronous.
2. When `set_batch_device` could not resolve the destination device (`device ==
   -1`), `device_streams_for` returns `None`, so `warm_raw == 0` and
   `serve_memory_tier_to_gpu` took its synchronous `DmaBuffer` branch. FR-037 /
   FR-052 require the fallback to the shared `warm_stream` in exactly that case.

Both properties (DIS-LOOKUP-ASYNC-WARM-STREAM, DIS-LOOKUP-ASYNC-DEVICE-STREAM)
fail for the same root cause: the warm path was written as a synchronous serve
that hid its stream.

## Impact

- Every memory-tier hit paid a full `stream_synchronize` inside `lookup_async`;
  callers could not overlap lookups with computation — the async interface
  existed but was never asynchronous on the warm path.
- The returned `GpuStream` was always null for warm hits, so any caller that
  (per the interface contract) synchronized the returned stream actually
  synchronized nothing. Data was still delivered (the internal sync had
  completed it), so there was no silent-data-corruption hazard, but the
  contract DIS-LOOKUP-ASYNC-WARM-STREAM was unimplementable for callers.
- Under multi-GPU with an unresolvable destination device, the copy silently
  degraded to the synchronous DmaBuffer path instead of the shared warm stream
  (DIS-LOOKUP-ASYNC-DEVICE-STREAM violation; also a performance cliff).

## How to reproduce

`components/dispatcher/src/repair_test_lookup_async_stream.rs` (wired as
`mod repair_test_lookup_async_stream` in `src/lib.rs`), three tests with
self-contained mocks (recording `IGpuServices`, `WarmDm`, `PoolMt`,
`QuietLogger`):

```bash
cargo test -p dispatcher repair_test_lookup_async_stream
```

- `warm_hit_returns_the_device_stream_the_copy_was_queued_on` — device 5 known;
  asserts the returned stream is non-null, is the stream the H2D was queued on,
  was created on device 5, that the dispatcher performed no internal
  `stream_synchronize`, and that the data lands in the destination only after
  the caller synchronizes the returned stream. **Fails on the unfixed code**
  (null stream returned, internal sync recorded).
- `warm_hit_unknown_device_falls_back_to_shared_warm_stream` — device resolution
  fails, shared warm stream pre-seeded at 0x2000; asserts the copy is queued on
  0x2000 and that stream is returned. **Fails on the unfixed code** (DmaBuffer
  synchronous path, null stream returned).
- `sync_lookup_still_delivers_after_delegation` — device 6 (distinct ordinal to
  avoid the process-global `DEVICE_STREAMS` map racing across parallel tests);
  asserts the synchronous `lookup` delivers data via delegation. Regression
  guard for the wrapper; passes before and after (the old code also delivered,
  just synchronously).

On the baseline commit both reproducing tests panic at their first assertion
(null stream / wrong copy path), not at compile time.

## Fix

`components/dispatcher/src/lib.rs`, MemoryTier arm of `lookup_async`:

- Resolve the stream first: `stream_raw = if device >= 0 { warm_raw } else {
  self.warm_stream.load(Acquire) }`. A *known* device whose per-device stream
  setup failed (`warm_raw == 0`) deliberately does **not** take the shared
  fallback — that stream may be bound to another GPU and would fail
  `cudaMemcpyAsync` — but degrades to stream 0, where the helper's `DmaBuffer`
  branch copies synchronously and the null stream is the honest answer (same
  as the SSD/cold path).
- Call `serve_memory_tier_to_gpu(..., stream_raw, false)` — queue without
  synchronizing.
- Return `r.map(|()| GpuStream(stream_raw as *mut c_void))`.

The synchronous `lookup` wrapper needed no change: it already synchronizes
non-null returned streams (`src/lib.rs:2408-2419`). The SSD/cold arm is
untouched (still null stream).

Rejected alternatives:

- **Fall back to the shared warm stream whenever `warm_raw == 0`** (i.e. gate on
  the stream, not the device): rejected — for a *known* device the shared
  stream may be bound to another GPU; the device-gated form above is the
  spec-correct one (and is what the Creusot model was written against).
- **Inline the serve+sync logic in `lookup`** instead of delegating: rejected —
  duplicates the copy/segmentation logic and breaks the delegation property
  (`verify_dis_lookup_delegates_async`).
- **Create the shared warm stream in staging-mode `initialize`** so the fallback
  exists without SPDK: rejected — ripples through `initialize`'s Creusot model
  and many proofs; FR-037 does not mandate creating it in staging mode. The
  reproduction test pre-seeds `warm_stream` instead.

Formal verification (`components/dispatcher/verif-creusot/`):

- `W` gained `sdev: Mapping<u64, i32>` (stream → device it was created on);
  trusted `device_streams_for` strengthened to guarantee `sdev[ds.warm] ==
  device`; `LaG` gained `dev` (resolved device, -1 = unknown).
- `lookup_async` model: `stream_raw = if device >= 0 { warm_raw } else {
  s.p.warm }`, serve called with `synchronize: false`, Warm arm returns
  `Ok(stream_raw)`; ensures state the path-indexed stream contract plus the two
  device conjuncts; added `Ok(_) ==> has_gpu` (needed by `lookup`'s sync step).
- `lookup` model: the old ensures `Ok(_) ==> Ok(())` and `^s.g == recorded W`
  were **only provable at baseline because `Ok(st) ==> st == 0` made the whole
  sync branch dead code**; with live streams they are false when
  synchronization fails or logs its `Sync` event. Restated honestly: errors
  propagate, null stream ⇒ `Ok(())`, non-null stream ⇒ `Ok(())` or
  `Err(Io(Sync))`; the ghost world is captured at each exit (so it equals the
  final world including the `Sync` event); the stream contract is stated via
  the new named predicate `ret_stream_ok`.
- New drivers `verify_dis_lookup_async_warm_stream` and
  `verify_dis_lookup_async_device_stream` (both Prove) with `__mutant` twins
  (both correctly fail to prove).
- Updated for the new semantics: `verify_dis_lookup_delegates_async` (honest
  delegation form; Proves, mutant fails), `verify_dis_lookup_warm_copies_data`,
  `verify_dis_lookup_after_downgrade`, `verify_dis_tier_stats_promotions_to_gpu`
  (its `promo_gpu` counting predicate assumed `Ok(0)` meant success; now counts
  any `Ok`, matching the real code's `if r.is_ok() {
  record_promotion_to_gpu() }`).
- `serve_memory_tier_to_gpu` regressed (timeout) from the larger `W` frame; a
  targeted `proof_assert!` of the `scattered` invariant for `i+1` in the loop
  body restores it (Proves).

Tests: `cargo test -p dispatcher` run **five times**, all green (145 unit
tests incl. the 3 new ones, plus 5 + 14 in the other binaries; no flakes, no
crashes).

## Scope beyond the finding

- The `device >= 0` gating fixes a latent multi-GPU hazard beyond the literal
  finding: previously a known device with failed per-device setup would have
  used whatever stream `warm_raw` gave (0 → DmaBuffer, so actually safe), but
  any future non-zero cross-device fallback would have been a correctness bug;
  the gate now encodes the invariant explicitly.
- `lookup`'s model spec was materially wrong (vacuously true at baseline) even
  though `lookup`'s Rust code was not; the repair made the model honest, which
  is required for the proof chain to mean anything.
- Full-module Creusot regression sweep: all 224 non-lookup-family modules run
  (208 Proved); the lookup family (38 modules) run separately. Every module
  that proves at the pristine baseline still proves. Modules failing here
  fail identically at the pristine baseline (extracted to `/tmp/base-vc`):
  this environment lacks Coq 8.20, which several baseline goals require —
  `verify_dis_lookup_warm_copies_data`, `verify_dis_lookup_after_downgrade`,
  `verify_dis_lookup_async_dma_error`, `verify_dis_lookup_miss_key_not_found`,
  `verify_dis_lookup_cold_no_slot`, `verify_dis_lookup_cold_staging_fallback`,
  `verify_dis_copy_async_no_slot`, `verify_dis_copy_async_overflow`,
  `verify_dis_batch_populate_register_after_sync`,
  `verify_dis_initialize_drive_failure_io_error`,
  `verify_dis_initialize_partition_guard`, `verify_dis_inv_not_initialized`,
  `verify_dis_inv_staging_bounded`, `verify_dis_pin_missing`,
  `verify_dis_pin_writer_timeout`,
  `verify_dis_schedule_wt_failure_keeps_entry`,
  `verify_dis_shutdown_checkpoint_before_devices`, `shutdown` (timeout), and
  the `refute_dis_*` drivers (which fail by design). Mutant twins of every
  lookup-family driver fail as required.

## Risk

- **Read-pin window (pre-existing class, now more visible):** the warm read pin
  is released (and `mt.touch` taken) right after the copy is *queued*, before
  the caller synchronizes the returned stream. Until the caller syncs, the
  memory-tier slot is unpinned and could in principle be evicted and reused
  while the DMA is in flight. `batch_lookup` closed exactly this window for
  itself (FR-039: pin held until after the warm stream sync). For
  `lookup_async` the dispatcher cannot know when the caller will sync, so this
  is inherent to the async interface contract rather than a bug in this fix;
  the synchronous `lookup` wrapper syncs before returning and is unaffected.
- **Staging mode:** `initialize` creates the shared `warm_stream` only on the
  SPDK/factory path. In staging mode an unknown-device warm hit therefore
  degrades to the synchronous DmaBuffer copy (stream 0) — correct, but not
  async. Pre-existing behavior, unchanged by this repair.
- The `DEVICE_STREAMS` map is process-global; tests using the same device
  ordinal in parallel can race (the new tests use distinct ordinals 5 and 6).
- `cargo clippy -p dispatcher --all-targets` fails inside the `gpu-services`
  dependency (14 pre-existing errors, outside this component's write scope);
  `cargo fmt --check` fails on untouched files at baseline (rustfmt version
  drift). Neither is caused by this repair; the new test file is
  rustfmt-clean.

## Follow-ups

- Design a pin-handoff for `lookup_async` (e.g. return a guard that releases
  the read pin when the caller synchronizes), closing the FR-039-class window
  for the single-key async path.
- Consider creating the shared warm stream in all `initialize` paths so the
  FR-037 fallback exists in staging mode.
- Re-run the `verify_dis_batch_lookup_*` mutant twins in an environment with
  Coq 8.20 installed (their contracts are unchanged by this repair, so no
  behavioral change is expected).
- Install Coq 8.20 (or `tools-creusot-coq-install`) so the baseline-failing
  modules listed above can be re-established in this environment.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-LOOKUP-ASYNC-WARM-STREAM,DIS-LOOKUP-ASYNC-DEVICE-STREAM`, callee modules `lookup_async`.

```
ACCEPT-REPO: base 850a00403ce3a8ae4815c3abcb59f5d7f264e99f; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (164 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-LOOKUP-ASYNC-WARM-STREAM yes
  C. TARGET   : dispatcher DIS-LOOKUP-ASYNC-DEVICE-STREAM yes
  E. (before side for dispatcher reused from cache: same base, proof crate and gate)
  E. REVERIFY: dispatcher yes (112 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 164 passed in each run
  dispatcher                   | DIS-LOOKUP-ASYNC-WARM-STREAM             | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | DIS-LOOKUP-ASYNC-DEVICE-STREAM           | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 112 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 248 min | 120 min | accepted |

Total wall-clock: 369 min.
