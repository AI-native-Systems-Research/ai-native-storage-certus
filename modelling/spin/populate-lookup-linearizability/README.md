# Populate-Lookup Linearizability

**Scope:** Whole system (client → dispatcher → memory-tier / dispatch-map → background writer → block-device eviction)

This Spin/Promela model verifies the dispatcher's core visibility contract:

> After `populate(key)` returns `Ok`, a concurrent `lookup(key)` never observes
> `KeyNotFound`, **unless** an explicit `remove(key)` or a memory-tier eviction
> (the "drop" path) has removed the entry from the dispatch-map in between.

The interesting concurrency is that `populate` is a **multi-phase, non-atomic**
operation (reserve a memory-tier slot → async GPU→DRAM DMA → register in the
dispatch-map → enqueue write-through), and it races against concurrent lookups,
the background writer, an evictor under pool pressure, and explicit removes. The
model pins down the linearization point of `populate` (`create_memory_tier_entry`)
and checks that every lookup observation is consistent with it.

## Properties Verified

| ID     | Property                                                                                                  | Type   |
|--------|-----------------------------------------------------------------------------------------------------------|--------|
| P-LIN  | A lookup observing `KeyNotFound` implies that, *at the lookup instant*, the key was never populated in this generation or a remove/eviction-drop had departed it. | Safety |
| P-PIN  | While a lookup holds its read-ref, the entry stays committed — no remove/drop under an in-flight load.     | Safety |
| P-REF  | The removal paths (eviction-drop, explicit remove) only fire when `read_refs == 0`.                       | Safety |
| P-WJOB | A queued write-through job always finds its entry still committed, resident in the memory-tier, and pinned by the writer's read-ref. | Safety |
| P-FIN  | At quiescence, every not-committed key has departed or was never populated, and no read-refs are leaked.  | Safety |

## System Abstraction

| Real component / operation                                       | Promela process / construct              |
|------------------------------------------------------------------|------------------------------------------|
| Client `populate(key)` (dispatcher `reserve_memory` → `copy_gpu_to_memory_completed`) | `Populator` proctype (one per key), `do_populate` inline |
| Client `lookup(key)` (dispatcher `lookup_async` → `dm.lookup`)   | `Looker` proctype (`N_LOOK` concurrent)  |
| Background write-through worker                                   | `BgWriter` proctype + `write_q` channel  |
| Memory-tier eviction (`evict_for_space` / `evict_one_clean`)     | `Evictor` proctype, `evict_one_clean` inline |
| Explicit `dispatcher.remove(key)`                                | `Remover` proctype                       |
| Dispatch-map entry visibility                                    | `committed[k]`                           |
| Entry location (MemoryTier vs BlockDevice)                       | `location[k]` ∈ {`MT`, `SSD`}            |
| Write-through complete (`ssd_offset` set)                        | `persisted[k]`                           |
| Dispatch-map read/write references                               | `read_refs[k]`, `write_ref[k]`           |
| Memory-tier pool occupancy                                       | `in_pool[k]`, `pool_used`, `POOL_CAP`    |
| `mt.peek` generation check on writer completion                  | `gen[k]`, job carries `(key, gen)`       |

## Assumptions / Stubs

- **Linearization point.** `populate` becomes visible at `create_memory_tier_entry`
  (`lib.rs:3044`), modeled as an atomic step that sets `committed[k]` together with
  `downgrade_reference` (write-ref → writer's read-ref) and `pop_ok[k]`. The GPU
  DMA (phase 2) is modeled as a bare schedulable point — its data movement is
  irrelevant to visibility. Splitting `committed` and `pop_ok`, or committing
  *after* returning Ok, would break P-LIN.
- **BlockDevice (cold-path) hits.** A lookup that finds a demoted (`SSD`) entry is
  modeled as a direct hit (pin → serve → release). The real `promote_and_serve`
  re-reads from NVMe and may re-insert into the memory-tier; that promotion does
  not change dispatch-map visibility, so it is abstracted away here. (Promotion
  *atomicity* is a separate property — see `../promotion-atomicity`.)
- **SSD-tier eviction is out of scope.** Reclaiming space on the block device
  (removing `SSD` entries) belongs to a different subsystem; `evict_one_clean`
  here only touches pool-resident (`in_pool`) entries, matching the real code.
- **Write-through may fail.** `BgWriter` nondeterministically completes or fails
  the write (`IoError`). A failed write releases the read-ref without setting
  `persisted`, producing the unpinned-and-unpersisted entry that the eviction
  **drop** path (`dm.remove` inside `evict_one_clean`) must handle.
- **No blind LRU.** The real `evict_one_clean` refuses to blind-free a pinned
  slot; the model's evictor likewise only reclaims unpinned victims.
- **Bounded channel.** `write_q` is a bounded channel (`N_KEYS` deep); the real
  writer queue is effectively unbounded but the bound is never hit here.

## Running

Spin must be installed (see `../write-before-evict/README.md`, or
`../install-spin.sh --prefix $HOME/.local`).

```bash
cd modelling/spin/populate-lookup-linearizability
make            # spin -a, compile pan, run exhaustive safety verification
make clean      # remove generated pan.* and trails
```

To replay a counterexample if one is ever produced:

```bash
spin -t -p -g -l populate_lookup_linearizability.pml
```

## Tuning the Model

| Parameter   | Default | Meaning / effect                                                            |
|-------------|---------|-----------------------------------------------------------------------------|
| `N_KEYS`    | 2       | Number of distinct keys / Populators. More keys ⇒ larger state space.       |
| `POOL_CAP`  | 1       | Memory-tier pool slots. `< N_KEYS` forces eviction (demote/drop) contention. |
| `N_LOOK`    | 2       | Concurrent Looker processes (exercises P-PIN across overlapping pins).       |
| `LOOKUPS`   | 1       | Lookups per Looker. Raising to 2 keeps full coverage but grows to ~22M states. |
| `MAX_TRIES` | 4       | `max_eviction_attempts` before `populate` gives up with `AllocationFailed`. |

Current defaults verify exhaustively in **~4.4 s**: **7,354,797 states**, depth 221,
**0 errors**, and **0 unreached states** in every proctype (100% coverage).

## Correspondence to Source Code

| Model location (`populate_lookup_linearizability.pml`)   | Source (`components/dispatcher/src/lib.rs`)                     | Lines        |
|----------------------------------------------------------|----------------------------------------------------------------|--------------|
| `do_populate` phase 1 (reserve + evict loop)             | `populate` → `reserve_memory` → `evict_and_insert` → `evict_for_space` | 2860, 2907, 1015, 962 |
| `do_populate` phase 3 (commit + downgrade, atomic)       | `copy_gpu_to_memory_completed`: `create_memory_tier_entry`, `downgrade_reference` | 3025, 3044, 3088 |
| `write_q ! k, gen` (enqueue write-through)               | `copy_gpu_to_memory_completed`: `writer.enqueue`               | 3095–3102    |
| `Looker` atomic `dm.lookup` snapshot + pin               | `lookup_async` → `dm.lookup` (read-ref on hit)                 | 2711, 3676   |
| `Looker` hit-serve + `release_read`                      | `lookup_async` MemoryTier / BlockDevice serve + `release_read` | 2753–2785    |
| `evict_one_clean` demote branch                          | `evict_one_clean` → `dm.try_evict_to_block` / `convert_to_storage` | 920, 3730 |
| `evict_one_clean` drop branch                            | `evict_one_clean` → `dm.remove`                                | 920, 3810    |
| `BgWriter` persist + release ref (and failure branch)    | background write-through; `convert_to_storage` consumes the ref | 3730         |
| `Remover`                                                | `dispatcher.remove` → `dm.remove` (rejects `read_refs > 0`)    | 2813, 3810   |
