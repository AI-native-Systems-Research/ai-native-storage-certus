# Cold-path Promotion Atomicity under Concurrent Lookups

**Scope:** Whole system (concurrent `batch_lookup` threads → dispatch-map + memory-tier → background evictor)

This Spin/Promela model verifies that when several concurrent lookups race to
promote the **same** key from SSD (`BlockDevice`) into the memory-tier, the
promotion is atomic and the losers recover correctly:

> Exactly one lookup wins the `mt.insert` and allocates the slot; every loser
> observes `AlreadyExists` and re-serves the block warm via the recovery pass
> (`serve_concurrently_promoted`). The dispatch-map entry flips to `MemoryTier`
> only **after** the winner's SSD→DRAM read completes, so any lookup that
> observes `MemoryTier` is guaranteed resident data.

It models the **current** in-place promotion protocol
(`promote_block_to_memory_tier`, refs preserved — lib.rs:3894 / 2486) plus the
concurrent-promotion recovery pass (lib.rs:2689, `serve_concurrently_promoted`
lib.rs:1181), and adds a distinct warm-serving lookup role that races with
promotion. It is a companion to [`../promotion-atomicity`](../promotion-atomicity),
which models an older remove+recreate protocol without the warm-lookup/recovery
role.

## Properties Verified

| ID | Property                                                                                              | Type   |
|----|-------------------------------------------------------------------------------------------------------|--------|
| A1 | At most one lookup wins `mt.insert` per promotion lifecycle — no double-allocation of the slot.        | Safety |
| A2 | Whenever a lookup observes `MemoryTier`, the slot is allocated **and** resident (SSD read completed).  | Safety |
| A3 | Eviction never fires on a pinned entry (`read_refs > 0`); a warm serve holds its pin throughout.      | Safety |
| A4 | `mt_pool_used` always equals the number of live slots — no losing promoter leaks a slot.              | Safety |
| A5 | At quiescence: no leaked read-refs, and dispatch-map / memory-tier state are consistent.              | Safety |

## System Abstraction

| Real component / operation                                                     | Promela process / construct        |
|--------------------------------------------------------------------------------|-------------------------------------|
| Concurrent `batch_lookup` threads (classify → promote / recover / warm-serve)  | `LookupThread` proctype (`N_THREADS`) |
| `dm.lookup` (pins on hit), `release_read`                                       | `atomic` classify block, `read_refs[]` |
| `mt.insert(key)` — the keyed, atomic allocation                                 | `atomic` insert block (`won` / `already` / `poolfull`) |
| `dm.promote_block_to_memory_tier` — in-place flip                               | `atomic { dm_loc[k] = MT }`         |
| `serve_concurrently_promoted` (loser recovery loop)                             | `already` branch + outer retry loop |
| `evict_for_space` / `try_evict_to_block`                                         | `Evictor` proctype                  |
| Pipelined SSD→DRAM read (+ fused SSD→GPU serve for N==1)                         | winner `skip` + `resident[k] = true` |
| Dispatch-map location / memory-tier slot / residency                            | `dm_loc[]`, `mt_alloc[]`, `resident[]` |
| Promotion-lifecycle winner counter                                              | `win_count[]` (reset on eviction)   |

## Assumptions / Stubs

- **Entries are never fully removed.** Within promotion-atomicity scope nothing
  calls `dm.remove` on a `BlockDevice` entry (SSD-tier reclamation is a separate
  subsystem), so a key's dispatch-map entry always exists as `BlockDevice` or
  `MemoryTier`; the `NotExist` recovery branch (lib.rs:1216) is out of scope.
- **Winner serve is fused with the read.** The single-region cold path fuses the
  SSD→GPU copy into the pipelined read (`gpu_dst` = the region, lib.rs:2357), so
  the winner has no post-flip serve window and holds no dispatch-map pin after
  promotion. The multi-region scatter's post-flip slot lifetime is a
  write-before-evict / pin-safety concern verified in
  [`../write-before-evict`](../write-before-evict), not here.
- **SSD read is a nondeterministic schedulable step** (`skip`); its data movement
  is irrelevant to the atomicity of the dispatch-map/memory-tier transition. The
  read is always modeled as succeeding (the failure-undo path is covered by the
  sibling `../promotion-atomicity` model).
- **Bounded retries.** A `LookupThread` performs at most `MAX_ATTEMPTS`
  classify/promote/recover iterations, modeling the bounded retry + timeout of
  `serve_concurrently_promoted` (lib.rs:1204). Exhausting attempts models the
  timeout → `KeyNotFound` outcome and is a legitimate termination.
- **Memory-tier eviction is DM-driven.** The evictor only demotes `MemoryTier`
  entries (matching `try_evict_to_block`, which rejects non-`MemoryTier` and
  pinned entries); a winner mid-read is invisible to it because the entry is
  still `BlockDevice`.

## Running

Spin must be installed (see [`../write-before-evict/README.md`](../write-before-evict/README.md),
or `../install-spin.sh --prefix $HOME/.local`).

```bash
cd modelling/spin/promotion-lookup-race
make            # spin -a, compile pan, run exhaustive safety verification
make clean      # remove generated pan.* and trails
```

Replay a counterexample if one is ever produced:

```bash
spin -t -p -g -l promotion_lookup_race.pml
```

## Tuning the Model

| Parameter      | Default | Meaning / effect                                                                    |
|----------------|---------|-------------------------------------------------------------------------------------|
| `N_KEYS`       | 2       | Distinct keys. `> POOL_CAP` is what makes the `PoolFull` + eviction path reachable. |
| `POOL_CAP`     | 1       | Memory-tier pool slots. `< N_KEYS` forces cross-key pool contention.                |
| `N_THREADS`    | 3       | Lookup threads, assigned `tid % N_KEYS`. `≥ 2` on one key gives the same-key race.  |
| `MAX_ATTEMPTS` | 3       | Bounded classify/promote/recover retries per thread (models the recovery timeout).  |

With `N_THREADS=3` / `N_KEYS=2`, threads 0 and 2 both target key 0 (the same-key
promotion race) while thread 1 targets key 1 (cross-key pool contention). Current
defaults verify exhaustively in **~0.01 s**: **39,626 states**, depth 107,
**0 errors**, and **0 unreached states** in every proctype (100% coverage).

## Correspondence to Source Code

| Model location (`promotion_lookup_race.pml`)           | Source (`components/dispatcher/src/lib.rs`)                          | Lines            |
|--------------------------------------------------------|----------------------------------------------------------------------|------------------|
| Classify `atomic` (pin on hit)                         | `batch_lookup` classification loop, `dm.lookup`                      | 2160–2214, 3676  |
| Cold `release_read` before promote                     | `batch_lookup` BlockDevice arm `dm.release_read`                     | 2205             |
| `mt.insert` `atomic` (`won`/`already`/`poolfull`)      | `evict_and_insert` → `mt.insert` (Ok / `AlreadyExists` / `PoolFull`) | 2347–2383, 1015  |
| Winner read + `resident = true` + flip to MT           | fused SSD→GPU read + `promote_block_to_memory_tier`                  | 2357, 2486, 3894 |
| `already` branch (loop → re-lookup → warm serve)       | `serve_concurrently_promoted` recovery loop                         | 2689–2698, 1181  |
| Warm-hit serve (pin → serve → `release_read`)          | `batch_lookup` `MemoryTier` arm + `serve_memory_tier_to_gpu`         | 2175–2203        |
| `Evictor` demote + free slot                           | `evict_for_space` → `evict_one_clean` → `try_evict_to_block`         | 962, 920, 3933   |
