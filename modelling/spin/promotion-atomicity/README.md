# Cold-path Promotion Atomicity

## Scope

Whole system (LookupThread → DispatchMap → MemoryTier → Evictor)

## Description

Verifies that at most one thread succeeds in promoting a given key from SSD
(BlockDevice state in the dispatch-map) to the DRAM memory-tier. When multiple
concurrent `batch_lookup` or single `lookup` calls discover the same key is cold
(on SSD), they race to promote it. The model verifies that the dispatch-map's
reference counting, entry-state transitions, and the memory-tier's
`AlreadyExists` guard ensure exactly-once promotion with no leaked slots.

Includes a background evictor that can demote promoted keys back to BlockDevice
state, creating the scenario where a key that was previously promoted becomes
promotable again. The model verifies that even with eviction-induced re-promotion,
at most one thread holds a live memory-tier allocation at any instant.

## Properties Verified

| ID | Property | Type |
|----|----------|------|
| P1 | At most one thread holds a live MT allocation per key at any instant (no concurrent double-allocate) | Safety |
| P2 | No double-allocation: `mt.insert(key)` succeeds at most once per key lifecycle | Safety |
| P3 | After all threads complete, `write_ref = 0` and `read_ref = 0` (no dangling locks) | Safety |
| P4 | A failed promoter cleans up its MT allocation (no dangling slot without dm entry) | Safety |
| P5 | Pool usage counter is always consistent with actual allocations | Safety |

## System Abstraction

| Real component | Promela process |
|----------------|-----------------|
| `batch_lookup` cold-path threads / single `lookup` | `LookupThread(id)` × N_THREADS |
| Background evictor (`evict_for_space` / `BackgroundEvictor`) | `Evictor()` |
| `DispatchMapComponent::lookup()` (wait + read_ref++) | atomic: check state + inc read_ref |
| `DispatchMapComponent::release_read()` | atomic: dec read_ref |
| `DispatchMapComponent::remove()` (ref-checked) | atomic: check refs + set DM_EMPTY |
| `DispatchMapComponent::create_memory_tier_entry()` | atomic: check empty + create entry |
| `DispatchMapComponent::release_write()` | atomic: set write_ref = 0 |
| `DispatchMapComponent::convert_memory_tier_to_block()` | Evictor: set DM_BLOCK_DEVICE |
| Memory-tier `insert(key)` | atomic: check !allocated + pool < cap |
| Memory-tier `remove(key)` / `evict_lru()` | Evictor: clear mt_allocated, dec pool |
| SSD read (pipelined NVMe + CUDA DMA) | Nondeterministic success/failure |

## Assumptions / Stubs

- **SSD read** — nondeterministic success/failure. The pipelined DMA is irrelevant to promotion atomicity.
- **GPU DMA** — not modeled (irrelevant to the atomicity property).
- **Multiple drives** — abstracted; the model focuses on per-key serialization.
- **Dispatch-map condvar** — wait-for-write_ref==0 modeled as atomic guard (skip if can't proceed, modeling timeout path).
- **Pool capacity** — bounded at POOL_CAP=2 to force eviction and test re-promotion.
- **Generation counter** — tracks eviction cycles; prevents conflating separate promotion lifecycles.

## Running

```bash
cd modelling/spin/promotion-atomicity

# Safety verification (assertions + invalid end-states)
make

# Or step by step:
spin -a promotion_atomicity.pml
cc -O2 -DSAFETY -o pan pan.c
./pan -m200000

# Liveness/deadlock check
make liveness

# Clean generated files
make clean
```

## Tuning the Model

| Parameter  | Value | Rationale                                           |
|------------|-------|-----------------------------------------------------|
| N_THREADS  | 3     | Thread 0 and 2 race on key 0; thread 1 on key 1    |
| N_KEYS     | 2     | Two keys verify per-key isolation                   |
| POOL_CAP   | 2     | Forces eviction under contention                    |

Why these are sufficient:
1. The promotion protocol is per-key — if safe for 2 competing threads on one key, it's safe for N.
2. Two keys verify that per-key state doesn't leak across keys.
3. Three threads + evictor create 4-way interleavings covering double-race, evict-then-retry, and uncontested patterns.
4. POOL_CAP = N_KEYS means the evictor must fire to allow re-promotion.

To explore larger state spaces:
```bash
spin -DN_THREADS=4 -DN_KEYS=3 -a promotion_atomicity.pml
cc -O2 -DSAFETY -DMEMLIM=8192 -o pan pan.c
./pan -m500000
```

## Correspondence to Source Code

| Model location | Source file | Line range |
|----------------|-------------|------------|
| `LookupThread` / dm.lookup | `components/dispatch-map/src/lib.rs` | 143–183 |
| `LookupThread` / release_read before promote | `components/dispatcher/src/lib.rs` | 1349 |
| `LookupThread` / mt.insert | `components/dispatcher/src/lib.rs` | 1489 |
| `LookupThread` / dm.remove | `components/dispatcher/src/lib.rs` | 1542 |
| `LookupThread` / create_memory_tier_entry | `components/dispatcher/src/lib.rs` | 1543–1553 |
| `LookupThread` / release_write | `components/dispatcher/src/lib.rs` | 1559 |
| `Evictor` / evict_for_space | `components/dispatcher/src/lib.rs` | 483–542 |
| `Evictor` / convert_memory_tier_to_block | `components/interfaces/src/idispatch_map.rs` | 155 |
| dm.remove (ref check) | `components/dispatch-map/src/lib.rs` | 336–354 |
| dm.create_memory_tier_entry (AlreadyExists) | `components/dispatch-map/src/lib.rs` | 377–389 |
