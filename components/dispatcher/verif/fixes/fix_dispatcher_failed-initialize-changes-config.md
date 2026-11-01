# fix_dispatcher_failed-initialize-changes-config

> **In short** - Problem: An `initialize()` that failed validation had already overwritten the dispatcher's eviction-attempt limit and store-backpressure budget, silently retuning a running dispatcher with a rejected config. Fix: The two tunables are committed only at the end of a successful `initialize()`, so a failing call changes nothing. ``` component:      dispatcher finding:        failed-initialize-changes-config property:       DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE found by:       creusot refutation `refute_dis_initialize_failure_no_config_change` (machine-proved negation) base commit:    850a00403        branch: repair/j33-rc26-20261030 status:         found -> test red -> fixed -> verified (creusot) -> awaiting acceptance check reviewer:       (name, date) ```


```
component:      dispatcher
finding:        failed-initialize-changes-config
property:       DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE
found by:       see "How it was found"
base commit:    850a00403ce3        branch: repair/j33-rc26-20261030
status:         found -> test red -> fixed -> verified (acceptance check) -> PR: (to open) -> merged: (no)
reviewer:       (name, date)
```

## The property
A call to initialize that fails validation does not change the dispatcher's eviction-attempt limit or store-backpressure budget from what they were before the call.

## At a glance: spec, code, test

### What the spec says
(no spec line cited - a property the code itself documents)

### The code: before (-) and after (+)
```diff
@@ -2091,8 +2091,10 @@ impl IDispatcher for DispatcherComponent {
         self.log_info("dispatcher: initializing");
 
-        self.max_eviction_attempts
-            .store(config.max_eviction_attempts, Ordering::Relaxed);
-        self.store_backpressure_ms
-            .store(config.store_backpressure_ms, Ordering::Relaxed);
+        // The config's tunables (eviction-attempt limit, store-backpressure
+        // budget) are committed only once every validation below has passed:
+        // an initialize that fails — unbound receptacle, invalid config, drive
+        // creation — changes nothing (DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE).
+        // Nothing reads them before `initialized` is set, so committing them
+        // just before that release store also publishes them with it.
 
         self.dispatch_map
@@ -2311,4 +2313,12 @@ impl IDispatcher for DispatcherComponent {
         }
 
+        // Every validation has passed: commit the config's tunables now, right
+        // before publishing `initialized`, so only a successful initialize
+        // applies them (see the comment at the top of this function).
+        self.max_eviction_attempts
+            .store(config.max_eviction_attempts, Ordering::Relaxed);
+        self.store_backpressure_ms
+            .store(config.store_backpressure_ms, Ordering::Relaxed);
+
         self.initialized.store(true, Ordering::Release);
 
@@ -8542,2 +8552,6 @@ mod repair_test_duplicate_check_after_eviction;
 #[path = "repair_test_check_present_map_timeout.rs"]
 mod repair_test_check_present_map_timeout;
+
+#[cfg(test)]
+#[path = "repair_test_failed_initialize_changes_config.rs"]
+mod repair_test_failed_initialize_changes_config;
```

### The test that shows the bug: `components/dispatcher/src/repair_test_failed_initialize_changes_config.rs` (fails on the old code, passes on the fix)
```rust
//! Repair test for finding `failed-initialize-changes-config`
//! (DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE).
//!
//! `initialize` stored the config's `max_eviction_attempts` and
//! `store_backpressure_ms` into the component's live atomics as its very
//! first act (src/lib.rs:2093-2096), before any validation. Every later
//! validation failure — a receptacle not bound, an empty `data_pci_addrs`,
//! a drive-creation error — therefore returned `Err` with the component's
//! eviction-attempt limit and store-backpressure budget already overwritten
//! by the rejected config, even though the dispatcher reports itself as not
//! initialized (DIS-INITIALIZE-FAILURE-LEAVES-UNINITIALIZED) and the property
//! record (verif/unified_properties.yaml, DIS-INITIALIZE-FAILURE-NO-CONFIG-
//! CHANGE) promises that "a call to initialize that fails validation does not
//! change the dispatcher's eviction-attempt limit or store-backpressure
//! budget from what they were before the call".
//!
//! The user-visible shape of the bug: a re-initialize attempt with a bad
//! config silently retunes a running dispatcher (how hard eviction tries,
//! how long a store backpressures before failing) even though the caller
//! was told the call failed and nothing was applied.
//!
//! Self-contained: only the public `IDispatcher` API plus same-crate reads
//! of the two atomics, with its own no-op mocks, so it builds against the
//! unfixed code as well.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use component_core::query_interface;
use interfaces::{
    BounceBuffer, BounceError, CacheKey, DispatchMapError, DispatcherConfig, DispatcherError,
    IDispatchMap, IDispatcher, IMemoryTier, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot,
};

use crate::DispatcherComponent;

// --- no-op mocks: nothing on the dispatch map or the memory tier is called
//     on the paths under test (initialize only checks that the receptacles
//     are bound); they exist so the receptacles can be connected. ---
... (246 more lines)
```


Traces: dispatcher/src/lib.rs:1887, dispatcher/src/lib.rs:1889

## How it was found
- creusot: refuted - REFUTED — the negation 'refute_dis_initialize_failure_no_config_change' is machine-proved, so the code violates this obligation. This is a finding, not a gap: see the spec and code locations on the property record.
- kani: unscored - 

## The problem (agent's statement and verdict)
# Problem: `failed-initialize-changes-config` (DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE)

**One sentence (plain):** When `initialize()` is called with a configuration that
turns out to be invalid, the dispatcher has *already* overwritten its live tuning
knobs — the eviction-attempt limit (`max_eviction_attempts`) and the store
backpressure budget (`store_backpressure_ms`) — with the values from that bad
config, even though the call returned an error and the dispatcher reports itself
as not initialized.

**Spec location:** the property record
`components/dispatcher/verif/unified_properties.yaml:614`
(id `DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE`, statement: "A call to initialize
that fails validation does not change the dispatcher's eviction-attempt limit or
store-backpressure budget from what they were before the call."). The property is
code-origin (a frame property of `initialize`); the machine-checked refutation is
`refute_dis_initialize_failure_no_config_change` in
`components/dispatcher/verif-creusot/src/drivers.rs:225`, which proves the
negation: a state with `max_att = 5, bp_ms = 9` and no dispatch map bound, after a
failing `initialize` with `max_att = 7, bp_ms = 11`, ends with `max_att = 7` and
`bp_ms = 11`.

**Code location:** `components/dispatcher/src/lib.rs:2093-2096` — the first thing
`IDispatcher::initialize` does, before any validation, is

```rust
self.max_eviction_attempts
    .store(config.max_eviction_attempts, Ordering::Relaxed);
self.store_backpressure_ms
    .store(config.store_backpressure_ms, Ordering::Relaxed);
```

Every later validation failure — dispatch map not bound, memory tier not bound,
empty `data_pci_addrs`, and every drive-creation/partition/format error from
`create_data_drives` — returns `Err` *after* these two stores, so the component's
config is left mutated by a call that promised to change nothing.

**Verdict: code wrong.** The spec-level expectation (a failed initialize changes
no configuration) is the sensible one and matches the sibling property
`DIS-INITIALIZE-FAILURE-LEAVES-UNINITIALIZED` (a failed initialize leaves
`initialized` false). The fix is to commit the two config values only on the
success path, after every validation that can fail, rather than before them.

# fix_dispatcher_failed-initialize-changes-config

## In short

Problem: An `initialize()` that failed validation had already overwritten the
dispatcher's eviction-attempt limit and store-backpressure budget, silently
retuning a running dispatcher with a rejected config.
Fix: The two tunables are committed only at the end of a successful
`initialize()`, so a failing call changes nothing.

```
component:      dispatcher
finding:        failed-initialize-changes-config
property:       DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE
found by:       creusot refutation `refute_dis_initialize_failure_no_config_change` (machine-proved negation)
base commit:    850a00403        branch: repair/j33-rc26-20261030
status:         found -> test red -> fixed -> verified (creusot) -> awaiting acceptance check
reviewer:       (name, date)
```

## Bug

`IDispatcher::initialize` stored `config.max_eviction_attempts` and
`config.store_backpressure_ms` into the component's atomics as its very first
act — before checking that the dispatch map and memory tier are bound, before
validating `data_pci_addrs`, and before drive creation — so every `Err` return
left the component's eviction/backpressure tunables overwritten by a config
that was never accepted.

## Impact

A user who calls `initialize` with a bad config (a receptacle not yet bound, an
empty PCI list, a drive that fails to come up) gets an error, but the
dispatcher's tuning has silently changed anyway: eviction gives up after a
different number of attempts and stores backpressure for a different time
before surfacing `AllocationFailed`. The user-visible worst case is a
re-initialize attempt on an already-running dispatcher: one rejected call with,
say, `store_backpressure_ms: 0` flips the live dispatcher to fail-fast stores
even though the caller was told nothing was applied. (After a *first* failed
initialize on a fresh component the values are also wrong, but nothing is
serviced yet, so it is less visible.)

## How to reproduce

1. `cargo test -p dispatcher --lib repair_test_failed_initialize_changes_config`
   — on the unfixed code both tests panic: `a failed initialize must not change
   the eviction-attempt limit ... left: 777 right: 0` (and `left: 777
   right: 111` for the re-initialize case).
2. The second test in that file is the user-visible scenario: initialize
   successfully with `max_eviction_attempts: 111, store_backpressure_ms: 222`,
   then call `initialize` again with an empty `data_pci_addrs` and
   `777`/`999` — the call returns `InvalidParameter` but the unfixed code
   leaves the running limits at `777`/`999`.
3. Formally: on the unfixed code, Creusot machine-proves the violation
   (`refute_dis_initialize_failure_no_config_change`: a state with
   `max_att = 5, bp_ms = 9` and no dispatch map ends at the rejected config's
   `7`/`11` after a failing `initialize`).

## Fix

`components/dispatcher/src/lib.rs` (`initialize`): the two `.store()` calls
moved from the top of the function to the success commit point — immediately
before `self.initialized.store(true, Ordering::Release)` — so they execute only
after every validation that can fail (receptacles bound, config valid, drives
created, writers/evictors started) has passed. Nothing reads the two atomics
before `initialized` is published (all readers are `IDispatcher` methods gated
on `ensure_initialized`; the background writer and both evictors do not touch
them), and placing the relaxed stores just before the release store of
`initialized` publishes them with it.

Model (`verif-creusot/src/init.rs`): the mirror `initialize` moves
`s.p.max_att`/`s.p.bp_ms` assignment to just before `s.p.init = true`, and its
contract now states `Ok ==> max_att/bp_ms == cfg's` and
`Err ==> max_att/bp_ms == s.p's`. Drivers
(`verif-creusot/src/drivers.rs`): the refutation is replaced by
`verify_dis_initialize_failure_no_config_change` (proved) and its
`__mutant` twin asserting the old behaviour (fails to prove, as required).

Alternatives rejected:
- *Restore-on-error* (save the old values and put them back on each `Err`
  return): more code, and every future early-return would have to remember it;
  committing at the end makes the property structural.
- *Move the stores just past the early validation checks* (after the
  `data_pci_addrs` check): still violates the property for the drive-creation
  failures, which are exactly the realistic hardware-path errors.
- *Reject a failing initialize's config into a staging struct applied on next
  success*: a behaviour change nobody asked for; the property only demands that
  a failed call changes nothing.

## Scope beyond the finding

None. The source change is the two moved `.store()` calls plus comments; the
test file and the Creusot model/driver updates are this finding's own test and
proof. (The regenerated `.coma`/`proof.json` files under
`verif-creusot/verif/dispatcher_verif_rlib/` are cargo-creusot outputs that
rewrite wholesale on every run, including path changes from the worktree name.)

## Risk

Low. On the success path the behaviour is identical — the same values are
stored by the same call, only microseconds later and now ordered under the
`initialized` release store (a strict improvement for readers that acquire
`initialized`). The only observable difference is on failure paths, which is
the intended fix. No API, config-struct or persistence-format change; no other
component reads these atomics. All 5 `cargo test -p dispatcher` runs green
(144 lib + 5 integration + 14 lazy-migration tests), and every Creusot module
that proved before still proves (the two `verify_dis_initialize_*` modules that
fail — `partition_guard`, `drive_failure_io_error` — failed identically at the
base commit, confirmed against the committed `proof.json` nulls).

## Follow-ups

- `DIS-INITIALIZE-PARTITION-GUARD` and `DIS-INITIALIZE-DRIVE-FAILURE-IO-ERROR`
  Creusot modules still have unproved goals (pre-existing at the base commit,
  unrelated to this fix) — worth their own repair tickets.
- The property bundle (`verif/unified_properties.yaml`) still records this
  property as `refuted` with the old refutation as evidence; it should be
  re-scored to `proved` against `verify_dis_initialize_failure_no_config_change`
  in the next scoring pass (out of scope for this repair: the bundle is not
  writable here).
- Consider documenting in the spec's `initialize` contract that the config is
  applied atomically with success (the code now guarantees it; the spec text
  for `max_eviction_attempts`/`store_backpressure_ms` does not say when they
  take effect).


## Verification after the fix (acceptance check, last lines)
Target `dispatcher:DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE`, callee modules `initialize`.

```
ACCEPT-REPO: base 850a00403ce3a8ae4815c3abcb59f5d7f264e99f; components touched: dispatcher
  A. BUILDS   : 6 of 6 crates checked OK
  B. TESTED   : dispatcher yes (163 passed, 5/5 runs)
  C. TARGET   : dispatcher DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE yes
  E. (before side for dispatcher reused from cache: same base, proof crate and gate)
  E. REVERIFY: dispatcher yes (113 proved after the repair; 1 newly proved; 0 lost — same gate, same scope, before vs after)

EVIDENCE (for the PR description):
  component                    | check                                    | result
  workspace                    | build of touched and dependent crates    | 6 OK
  dispatcher                   | unit tests (5 runs)                      | 163 passed in each run
  dispatcher                   | DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE  | not proved before -> PROVED; new test fails on the old code, passes on the new
  dispatcher                   | re-verification (Creusot gate, before vs after) | 113 proved, 1 newly proved, 0 lost
ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost.
```

## Effort
| attempt | agent | check | verdict |
|---|---|---|---|
| 1 | 26 min | 109 min | accepted |

Total wall-clock: 135 min.
