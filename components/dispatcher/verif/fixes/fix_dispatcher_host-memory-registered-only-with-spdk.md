# fix_dispatcher_host-memory-registered-only-with-spdk

> **In short** - Problem: a dispatcher started with a GPU and a memory tier but no SPDK environment (factory-mode and memory-tier-only deployments) never pinned its DRAM pool for zero-copy GPU/NVMe access, and a failed registration was logged at info level instead of as a warning. Fix: `initialize` now registers the pool whenever the GPU and memory tier are ready — mirroring `shutdown`'s unregister — and reports a registration failure via `log_warn` (FR-034).


```
component:      dispatcher
finding:        host-memory-registered-only-with-spdk
property:       DIS-INITIALIZE-REGISTER-HOST-MEMORY
found by:       see "How it was found"
base commit:    850a00403ce3        branch: repair/j33-rc29-20261030
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
During initialize, once the GPU and memory tier are ready, the dispatcher registers the memory-tier pool with the GPU services for pinned zero-copy access, and a registration failure is reported as a warning rather than failing initialization.

## At a glance: spec, code, test

### What the spec says
`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:360`
```
- **FR-034**: During `initialize()`, after GPU and memory-tier are ready, the dispatcher MUST call `IGpuServices::register_host_memory` on the memory-tier pool to CUDA-pin and SPDK-register it for zero-copy NVMe and GPU DMA. Registration failure MUST be logged as a warning (zero-copy pipelines require this registration). Each data drive's `ClientChannels` MUST be managed via a per-drive `ChannelPool` rather than a single cached instance: the pool starts empty at `initialize()` and grows on demand — each concurrent reader (`promote_and_serve`, `serve_cold_staged`, and each `promote_to_memory_tier` per-drive prefetch thread) checks out an exclusive channel via `ChannelPool::checkout()`, which connects a fresh `ClientChannels` on first use or reuses one returned by a prior lease, and returns a `ChannelLease` (RAII) that puts the channel back in the pool on drop. Checkout MUST drain any completions left in `completion_rx` by a previous user before handing out the channel, so a recycled channel's stale tags are never matched against a new caller's segments. `connect_client()` MUST be called outside the pool's lock so a slow connect does not block other checkouts. This replaces a prior design that cached a single `ClientChannels` per drive at init time: a single shared SPSC channel let two concurrent readers on the same drive (e.g. `batch_lookup` and the `promote_to_memory_tier` prefetch path) dequeue each other's NVMe completions ("completion theft"), causing a permanent, non-timing-out hang. The pool's lazy, per-checkout connection is a deliberate trade-off of a small first-use latency for correctness under concurrent per-drive access.
- **FR-035**: During `shutdown()`, before memory-tier teardown, the dispatcher MUST call `IGpuServices::unregister_host_memory` to release CUDA page-locking and SPDK registration on the memory-tier pool.
```

### The code: before (-) and after (+)
```diff
@@ -2143,5 +2143,5 @@ impl IDispatcher for DispatcherComponent {
             }
 
-            // Pre-allocate pipeline ring and register memory with SPDK.
+            // Pre-allocate the pipeline ring's SPDK DMA buffers.
             // Only when SPDK is available — spdk_mem_register segfaults otherwise.
             if let Ok(gpu) = self.gpu_services.get() {
@@ -2186,25 +2186,29 @@ impl IDispatcher for DispatcherComponent {
                         }
                     }
+                }
+            }
+        }
 
-                    // Register memory-tier pool as CUDA-pinned + SPDK DMA-capable
-                    // for zero-copy NVMe reads and async GPU transfers.
-                    if let Ok(mt) = self.memory_tier.get() {
-                        if let Some((pool_ptr, pool_size)) = mt.pool_info() {
-                            match gpu
-                                .register_host_memory(pool_ptr as *mut std::ffi::c_void, pool_size)
-                            {
-                                Ok(()) => {
-                                    self.log_info(&format!(
-                                        "dispatcher: registered memory-tier pool ({} MiB) for zero-copy DMA",
-                                        pool_size / (1024 * 1024)
-                                    ));
-                                }
-                                Err(e) => {
-                                    self.log_info(&format!(
-                                        "memory-tier pool registration failed (non-fatal): {e}"
-                                    ));
-                                }
-                            }
-                        }
+        // Register the memory-tier pool with the GPU services as CUDA-pinned +
+        // SPDK DMA-capable for zero-copy NVMe reads and async GPU transfers
+        // (FR-034). This runs whenever the GPU and the memory tier are ready —
+        // not only when an SPDK environment is connected: factory-mode and
+        // memory-tier-only deployments have a GPU and a pool too. A failure is
+        // a warning, never an initialization failure (the DRAM path does not
+        // need the pinning; the zero-copy pipelines do). Mirrors `shutdown`'s
+        // unregister, which FR-035 gates the same way.
+        if let (Ok(gpu), Ok(mt)) = (self.gpu_services.get(), self.memory_tier.get()) {
+            if let Some((pool_ptr, pool_size)) = mt.pool_info() {
+                match gpu.register_host_memory(pool_ptr as *mut std::ffi::c_void, pool_size) {
+                    Ok(()) => {
+                        self.log_info(&format!(
+                            "dispatcher: registered memory-tier pool ({} MiB) for zero-copy DMA",
+                            pool_size / (1024 * 1024)
+                        ));
+                    }
+                    Err(e) => {
+                        self.log_warn(&format!(
+                            "dispatcher: memory-tier pool registration failed (non-fatal): {e}"
+                        ));
                     }
                 }
@@ -8542,2 +8546,6 @@ mod repair_test_duplicate_check_after_eviction;
 #[path = "repair_test_check_present_map_timeout.rs"]
 mod repair_test_check_present_map_timeout;
... (4 more lines in the commit)
```

### The test that shows the bug: `components/dispatcher/src/repair_test_register_host_memory.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `host-memory-registered-only-with-spdk`
//! (DIS-INITIALIZE-REGISTER-HOST-MEMORY, FR-034).
//!
//! The pool registration was nested inside `if self.spdk_env.is_connected()`
//! (and the whole GPU block inside the drives branch), so a dispatcher whose GPU
//! and memory tier are ready but that has no SPDK environment connected —
//! factory-mode and memory-tier-only deployments — never pinned the pool for
//! zero-copy access; and on the SPDK path a registration failure was logged at
//! info level, not as the warning FR-034 demands.
//!
//! Self-contained: it drives only the public `IDispatcher` API through its own
//! minimal mocks, so it builds against the dispatcher as it was before the
//! repair as well as after it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer,
    GpuIpcHandle, GpuStream, IDispatchMap, IDispatcher, IGpuServices, ILogger, IMemoryTier,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

const POOL_BYTES: usize = 1024 * 1024;

// --- dispatch map: empty; initialize in memory-tier-only mode touches it not ---

struct EmptyDm;

impl IDispatchMap for EmptyDm {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, _key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        Ok(LookupResult::NotExist)
    }
    fn convert_to_storage(&self, _key: CacheKey, _offset: u64) -> Result<(), DispatchMapError> {
... (400 more lines)
```


Traces: FR-034

## How it was found
- creusot: unscored - 
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem — DIS-INITIALIZE-REGISTER-HOST-MEMORY (`host-memory-registered-only-with-spdk`)

**One sentence:** when the dispatcher starts up with a GPU and a memory-tier pool
available but no SPDK environment connected (factory-mode or memory-tier-only
deployments), it never pins the pool for zero-copy GPU/NVMe access, and when
registration *is* attempted it reports a failure at info level where the spec
demands a warning.

**Spec:** `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:360`
(FR-034) — "During `initialize()`, after GPU and memory-tier are ready, the
dispatcher MUST call `IGpuServices::register_host_memory` on the memory-tier pool
to CUDA-pin and SPDK-register it for zero-copy NVMe and GPU DMA. Registration
failure MUST be logged as a warning (zero-copy pipelines require this
registration)." The matching teardown, FR-035 (spec.md:361), gates
`unregister_host_memory` on exactly "GPU and memory tier ready" — no SPDK
condition.

**Code:** `components/dispatcher/src/lib.rs:2158` nests the pool registration
(lib.rs:2189-2210) inside `if self.spdk_env.is_connected()`, and that whole block
sits inside the drives branch `if self.spdk_env.is_connected() || has_bd_factory`
(lib.rs:2115). So:

* factory mode (drives via `block_device_factory`, no `ISPDKEnv` receptacle) and
  memory-tier-only mode (no drives at all) both skip registration even though the
  GPU receptacle resolves and `mt.pool_info()` returns a pool — compare
  `shutdown` (lib.rs:2350-2355), which unregisters under the bare
  `if let (Ok(gpu), Ok(mt)) = ...` gate;
* on the SPDK path, a registration failure is reported with `self.log_info`
  (lib.rs:2203-2206), not `self.log_warn`.

**Verdict: code wrong.** The registration's guard is the wrong predicate (SPDK
connected, instead of GPU + memory tier ready, as FR-034/FR-035 state and as the
shutdown side already implements), and the failure log level disagrees with
FR-034's "MUST be logged as a warning".

# Repair — DIS-INITIALIZE-REGISTER-HOST-MEMORY (`host-memory-registered-only-with-spdk`)

## In short

Problem: a dispatcher started with a GPU and a memory tier but no SPDK environment (factory-mode
and memory-tier-only deployments) never pinned its DRAM pool for zero-copy GPU/NVMe access, and a
failed registration was logged at info level instead of as a warning.
Fix: `initialize` now registers the pool whenever the GPU and memory tier are ready — mirroring
`shutdown`'s unregister — and reports a registration failure via `log_warn` (FR-034).

## Bug

`initialize` nested the memory-tier pool registration inside `if self.spdk_env.is_connected()`
(and inside the drives branch), so it ran only on the SPDK path, and its failure arm called
`log_info` where FR-034 requires a warning.

## Impact

In factory-mode deployments (block devices injected via `set_block_device_factory`, e.g. the
certus-server-yaml file/NBD profiles) and in memory-tier-only mode, the memory-tier pool was never
CUDA-pinned/SPDK-registered even though the GPU services and the pool were both ready. A user would
see zero-copy pipelines that depend on the pinning (FR-019: NVMe DMA and async GPU transfers
directly into pool slots) fail or fall back at first use, with nothing in the log pointing at the
cause — initialization reported success and, on the SPDK path, a registration failure was buried at
info level instead of warning the operator (the spec notes the zero-copy pipelines require this
registration).

## How to reproduce

1. `cargo test -p dispatcher --lib repair_test_register_host_memory` on the unfixed tree: both
   tests panic — `initialize_registers_pool_when_gpu_and_tier_ready_without_spdk` because no
   `register_host_memory` call was made, and
   `registration_failure_is_a_warning_and_initialization_succeeds` because no warning was logged.
2. On the repaired tree the same command passes (2 tests).
3. Creusot (from `components/dispatcher/verif-creusot`,
   `PATH="$HOME/.local/share/creusot/bin:$PATH"`): `cargo creusot
   verify_dis_initialize_register_host_memory` prints `Proved ✔`; the
   `verify_dis_initialize_register_host_memory__mutant` twin (addressed with the collapsed single
   underscore, as `cargo creusot` emits it) stays unproved, as required.

## Fix

`components/dispatcher/src/lib.rs` (`initialize`): the registration block moved out of the
`if self.spdk_env.is_connected()` guard and out of the drives branch, to a standalone block right
after the drives branch and before the background writer starts. It is now gated on exactly what
FR-034 states and what `shutdown`'s matching unregister (FR-035, lib.rs:2354) already uses —
`if let (Ok(gpu), Ok(mt)) = (self.gpu_services.get(), self.memory_tier.get())` plus
`mt.pool_info()` — and the failure arm calls `self.log_warn` instead of `self.log_info`. On the
SPDK path the behaviour is unchanged (same call, same arguments, only slightly later in the same
init sequence and now warned on failure); the pipeline ring and warm-stream setup stay SPDK-gated
as before.

Alternatives rejected:

* *Only lift the registration out of the SPDK guard but keep it inside the drives branch* — this
  would fix factory mode but not memory-tier-only mode, so the property ("once the GPU and memory
  tier are ready") would still be violated, and the code would keep disagreeing with the
  symmetric `shutdown` unregister.
* *Fail initialization when registration fails* — FR-034 explicitly says a failure is a warning;
  the DRAM path does not need the pinning, and refusing to start would break deployments whose
  GPU services is compiled without CUDA.
* *Also move the warm-stream/pipeline-ring setup out of the SPDK guard* — out of scope: the ring
  allocates SPDK DMA buffers, which genuinely require SPDK (its failure mode is a segfault, not an
  error), and no property requires it.

Verification (Creusot, `verif-creusot/`): the `initialize` model in `src/init.rs` mirrors the new
code — registration after the drives branch, gated on `has_gpu && pool_info`, with two new
postconditions (registration logged whenever init succeeds with GPU+pool ready; the
entry-tick-oracle failure answered false implies the warning event). `gpu_register` in
`src/boundary.rs` now logs `Ev::RegWarn` after `Ev::Register` when its oracle answers false, and
`Ev` in `src/core.rs` gains that variant. `verify_dis_initialize_register_host_memory`
(`src/drivers.rs`) proves the property; its `__mutant` twin (negated registration clause) fails.
Every module that proved before still proves: I re-ran all 155 non-mutant `verify_*`/`refute_*`
modules — the 25 that fail do so with byte-identical goal counts on the pre-repair model (they
are the known goal-unproved set, e.g. PARTITION-GUARD, DRIVE-FAILURE-IO-ERROR,
LOOKUP-WARM-COPIES-DATA), and every other module, including all twelve `initialize` drivers,
prints `Proved ✔`.

## Scope beyond the finding

None. The only other source change is the two-line comment above the pipeline-ring block in
`initialize`, corrected because it described the registration that moved out of that block.

## Risk

Behaviour change: deployments without an SPDK environment connected now call
`IGpuServices::register_host_memory` on the pool at init where they previously did not. With a
real `gpu-services` this pins the pool with `cudaHostRegister` and registers it with SPDK; both
failure modes (`Err`) are warnings, and a gpu-services that is not initialized returns
`Err("Not initialized")` — a warning, not a crash. Double registration is tolerated by
gpu-services (`CUDA_ERROR_HOST_MEMORY_ALREADY_REGISTERED` and SPDK `-EBUSY` are treated as
success), so re-initialize cycles are unaffected. On the SPDK path the registration now happens a
few statements later in the same init sequence (after drive setup instead of inside it), still
before any lookup can be served. Log-level compatibility: the failure message moves from info to
warning and gains a `dispatcher: ` prefix.

## Follow-ups

* `dispatcher-p2p` has the same shape (`register_host_memory` inside its SPDK/ring guard, failure
  at info level, `components/dispatcher-p2p/src/lib.rs:1341`); the same FR-034 argument applies
  there and should be repaired in its own component scope.
* The 25 goal-unproved Creusot modules predate this repair (identical results before/after);
  they remain open verification work, not regressions.


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-INITIALIZE-REGISTER-HOST-MEMORY`, callee modules `initialize`.

```
ACCEPT-REPO: base 850a00403ce3a8ae4815c3abcb59f5d7f264e99f; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (163 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-INITIALIZE-REGISTER-HOST-MEMORY yes
  E. REVERIFY: dispatcher yes (112 proved after the repair; 0 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 163 passed in each run
  dispatcher                   | DIS-INITIALIZE-REGISTER-HOST-MEMORY      | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 112 proved, 0 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 78 min | 358 min | accepted |

Total wall-clock: 437 min.
