# Check→Pin Eviction Race (fixed by `evolve-dispatcher-dw`)

**Scope:** Whole system (vLLM connector ⇄ dispatcher → dispatch-map / memory-tier → background writer → eviction)

This Spin/Promela model reconstructs the race condition that branch
`evolve-dispatcher-dw` fixes (fix commit `1d55b9c2`, *"restore graceful degrade
under Check→Pin eviction race"*) and **demonstrates that Spin would have caught
it**. A single compile-time switch, `BUGGY_DROP_FALLBACK`, selects between the
pre-fix and post-fix dispatcher:

- **Fixed build** (`make safety`) — verification passes: **0 errors**.
- **Buggy build** (`make buggy`) — Spin reports an **assertion violation** and
  emits a replayable counterexample trail.

## The race

A vLLM connector **Checks** a content-addressed key resident, then — in a
*separate* RPC — **Pins + Loads** it. Between the Check and the Load's pin the
dispatch-map entry has `read_ref == 0`. The pre-fix `evict_one_clean` had a
fallback that, when a victim's write-through had not yet landed (unpersisted, so
it could not be demoted), called `dm.remove` to free the DRAM slot. Because a
just-Checked-but-not-yet-Pinned entry is unpinned, the remove **succeeded and
silently dropped the entry**:

```
Check(key) → resident ✓        (read_ref == 0, no pin taken yet)
                    ⇢ evict_one_clean drops the unpersisted, unpinned victim
Load(key)  → NotExist           → remote-lookup miss
                    → fatal IoError in the connector
                    → assert(transfer_result.success) → EngineDeadError → vLLM crash
```

The fix never full-removes here: an unpersisted victim is **skipped** (stays
resident in the memory-tier), a persisted victim is **demoted** to `BlockDevice`
(still a hit), and an all-unpersisted tier yields `AllocationFailed` → the caller
serves **uncached** — a survivable local miss, never a fatal one.

## Properties Verified

| ID        | Property                                                                                                          | Type   |
|-----------|------------------------------------------------------------------------------------------------------------------|--------|
| R-RESOLVE | Once a connector has Checked a key resident, a later Load of that key never observes `NotExist` — eviction may **demote** it but must never full-**remove** it. | Safety |
| R-REF     | The removal/demotion paths only act on unpinned victims (`read_refs == 0`).                                        | Safety |
| R-FIN     | At quiescence no read-refs are leaked and pool accounting is consistent (`pool_used` = live pool slots).           | Safety |

`R-RESOLVE` is the property that distinguishes the two builds: it **holds** in
the fixed build and is **violated** in the buggy build.

## System Abstraction

| Real component / operation                                            | Promela process / construct                       |
|-----------------------------------------------------------------------|---------------------------------------------------|
| Client `populate(key)` (reserve → DMA → commit → enqueue write-through)| `Populator` proctype (one per key), `do_populate` inline |
| vLLM connector: residency **Check** (no pin), then **Load** (`dm.lookup` pins) | `Connector` proctype (`N_CONN`)          |
| Background write-through worker (may fail with `IoError`)             | `BgWriter` proctype + `write_q` channel           |
| `evict_for_space` → `evict_one_clean` (demote / drop)                 | `Evictor` proctype, `evict_one_clean` inline      |
| Dispatch-map entry visibility / location                             | `dm_present[k]`, `dm_loc[k]` ∈ {`MT`, `BLOCK`}    |
| Write-through complete (`ssd_offset` set)                            | `persisted[k]`                                    |
| Dispatch-map read / write references                                 | `read_refs[k]`, `write_ref[k]`                    |
| Memory-tier pool occupancy                                           | `in_pool[k]`, `pool_used`, `POOL_CAP`             |
| `mt.peek` generation check on writer completion                      | `gen[k]`, job carries `(key, gen)`                |
| The pre-fix `dm.remove` drop fallback                                | `#ifdef BUGGY_DROP_FALLBACK` arms in `evict_one_clean` |

## Why the buggy state is reachable

Within eviction scope an entry is unpinned **and** unpersisted only if its
write-through has *failed*: the background writer holds the sole read-ref (from
`downgrade_reference`) until the write-through completes or fails, and only a
failure releases the ref while leaving `persisted == false`. `BgWriter` therefore
nondeterministically fails the write (`IoError`), producing exactly the
unpinned-and-unpersisted victim the drop fallback mishandled. (This is the same
reachability enabler used by the sibling
[`../populate-lookup-linearizability`](../populate-lookup-linearizability) model,
whose `P-LIN` *tolerates* the drop as a legitimate departure — `R-RESOLVE` here
is the stricter property that the fix restores and that forbids it.)

## Assumptions / Stubs

- **Check is unpinned, Load pins.** The connector's residency Check is a query
  distinct from the Load RPC and takes no dispatch-map read-ref; the Load's
  `dm.lookup` pins on a hit. The unpinned Check→Load window is the race window.
- **Linearization point.** `populate` becomes visible at
  `create_memory_tier_entry` (`lib.rs:3044`), modeled as one atomic step that
  sets `dm_present`, downgrades the write-ref to the writer's read-ref, and sets
  `pop_ok`.
- **Demotion keeps a key resolvable.** A demoted (`BlockDevice`) entry is still a
  dispatch-map hit (cold-path promote re-reads it), so `R-RESOLVE` treats demote
  as safe and only full-remove as the violation.
- **SSD-tier reclamation / explicit `remove` are out of scope.** Within this
  model nothing legitimately full-removes a present entry, so any transition of
  a Checked key to `NotExist` is the bug — never a legitimate departure.
- **Eviction fires under pressure.** The standalone `Evictor` models
  `evict_for_space` firing at any time under memory-tier pressure; it only ever
  acts on unpinned, pool-resident victims.
- **Bounded retries.** A populate performs at most `MAX_TRIES` eviction attempts
  before giving up with `AllocationFailed` (a graceful, uncached serve) — this is
  what the fixed build degrades to instead of dropping a live key.

## Running

Spin must be installed (see [`../write-before-evict/README.md`](../write-before-evict/README.md),
or `../install-spin.sh --prefix $HOME/.local`; add `$HOME/.local/bin` to `PATH`).

```bash
cd modelling/spin/check-pin-eviction-race

make            # FIXED dispatcher — exhaustive safety check, expect 0 errors
make buggy      # PRE-FIX dispatcher — expect an R-RESOLVE assertion violation + trail
make clean
```

Replay the buggy counterexample (the drop-then-miss interleaving):

```bash
spin -t -p -g -l -DBUGGY_DROP_FALLBACK check_pin_eviction_race.pml
```

## Results

**Fixed build** (`make safety`): **692,978 states**, depth 207, **0 errors**.

Coverage is 100% **except one deliberately-unreachable state** in `Connector`:
the `loaded_ok = false` (Load-miss) branch. That branch is *provably
unreachable* in the fixed build — which is precisely the guarantee: a key a
connector Checked resident can never miss its Load, so the fatal remote-forward
path is dead. (In the buggy build that same branch becomes reachable and is
where the assertion fires.)

**Buggy build** (`make buggy`): Spin halts at the first violation —
`assertion violated loaded_ok (at depth 172)` — after ~23,775 states, and writes
`check_pin_eviction_race.pml.trail`. The counterexample is exactly the race:

1. `Populator(0)` commits key 0 into the memory-tier, unpersisted; enqueues its
   write-through.
2. `Connector` **Checks** key 0 resident (`checked_present = true`) — no pin.
3. `BgWriter` **fails** key 0's write-through → releases the writer's read-ref;
   key 0 is now present, unpinned (`read_refs[0] == 0`), and unpersisted.
4. `Evictor` runs `evict_one_clean`, matches the `!persisted` victim arm, and —
   via the drop fallback — sets `dm_present[0] = 0` (full-remove).
5. `Connector` **Loads** key 0 → `NotExist` → `loaded_ok = false` →
   `assert(loaded_ok)` **fails**. In the real system this is the fatal
   remote-forward that crashes vLLM.

## Correspondence to Source Code

| Model location (`check_pin_eviction_race.pml`)                 | Source (`components/dispatcher/src/lib.rs`)                                   | Lines            |
|----------------------------------------------------------------|-------------------------------------------------------------------------------|------------------|
| `do_populate` phase 1 (reserve + evict loop)                   | `populate` → `reserve_memory` → `evict_and_insert` → `evict_for_space`        | 2860, 2907, 1015, 962 |
| `do_populate` phase 3 (commit + downgrade, atomic)            | `copy_gpu_to_memory_completed`: `create_memory_tier_entry`, `downgrade_reference` | 3025, 3044, 3088 |
| `Connector` Check (residency probe, no pin)                    | connector `Check` RPC / residency query (separate from Load)                  | —                |
| `Connector` Load (`dm.lookup` pins; miss → remote-forward)     | `batch_lookup` → `dm.lookup`; `KeyNotFound` → remote-lookup                    | 2160–2214, 3676  |
| `evict_one_clean` demote branch (persisted victim)             | `evict_one_clean` → `dm.try_evict_to_block` / `convert_to_storage`            | 920, 3933, 3730  |
| `evict_one_clean` **drop fallback** (`#ifdef BUGGY_DROP_FALLBACK`) | the `dm.remove(cand)` fallback **removed** by commit `1d55b9c2`           | 920 (pre-fix ~949–958) |
| `BgWriter` persist / fail + release ref                        | background write-through; `convert_to_storage` consumes the ref               | 3730             |

### The fix, in the source

Commit `1d55b9c2` deletes the fallback in `evict_one_clean`:

```rust
-            // Fallback: write-through incomplete so it can't be demoted. Drop it
-            // entirely — but only if unpinned. `dm.remove` returns an error for
-            // pinned entries, so a success means no in-flight load points at it;
-            // only then is it safe to free the DRAM slot.
-            if dm.remove(cand).is_ok() {
-                let _ = mt.remove(cand);
-                ...
-                return true;
-            }
+            // Write-through incomplete (no ssd_offset) so it can't be demoted.
+            // Do NOT full-remove it: `dm.remove` returns NotExist for the key,
+            // which is UNRECOVERABLE. Under the Check→Pin race a connector may
+            // have already Checked this key resident and be about to load it ...
+            // So skip this candidate and try the next one ...
```

The regression tests `evict_never_drops_unpersisted_unpinned_victim` and
`evict_skips_unpersisted_victims_and_demotes_persisted_one` (commit `787b8263`)
guard it at the Rust level; this model guards the concurrency argument exhaustively.
