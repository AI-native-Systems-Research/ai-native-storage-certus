# Cold-path Promotion Atomicity

**Scope**: Whole system (LookupThread → DispatchMap → MemoryTier)

Verifies that at most one thread succeeds in promoting a given key from SSD
(BlockDevice state in the dispatch-map) to the DRAM memory-tier. When multiple
concurrent `batch_lookup` or single `lookup` calls discover the same key is cold
(on SSD), they race to promote it. The model verifies that the dispatch-map's
reference counting and entry-state transitions ensure exactly-once promotion
with no leaked memory-tier slots.

## Properties Verified

| ID | Property | Type |
|----|----------|------|
| P1 | At most one thread successfully completes promotion for a given key (exactly one `create_memory_tier_entry` succeeds) | Safety |
| P2 | No double-allocation: `mt.insert(key)` succeeds at most once per key lifecycle (no leaked memory-tier slots) | Safety |
| P3 | After all promotions complete, the key is in MemoryTier state with `write_ref = 0` (no dangling locks) | Safety |
| P4 | A failed promoter cleans up its MT allocation (no dangling memory-tier slot without a matching dispatch-map entry) | Safety |

## System Abstraction

| Real component | Promela process |
|----------------|-----------------|
| `batch_lookup` cold-path threads / single `lookup` calling `promote_and_serve` | `LookupThread(id)` |
| `DispatchMapComponent::lookup()` (wait + read_ref++) | atomic block: check state + inc read_ref |
| `DispatchMapComponent::release_read()` | atomic dec read_ref |
| `DispatchMapComponent::remove()` (ref-checked) | atomic block: check refs + remove entry |
| `DispatchMapComponent::create_memory_tier_entry()` | atomic block: check empty + create entry |
| `DispatchMapComponent::release_write()` | atomic set write_ref = 0 |
| Memory-tier `insert(key)` | atomic check + set `mt_allocated[key]` |
| SSD read (pipelined_ssd_to_gpu) | Nondeterministic success/failure |

## Assumptions / Stubs

- **SSD read** — modeled as nondeterministic success/failure. The actual pipelined DMA transfer is not relevant to promotion atomicity; only whether it succeeds or fails matters.
- **Eviction** — not modeled (evict_for_space is called before mt.insert but doesn't affect the promotion race).
- **GPU DMA** — not modeled (irrelevant to the atomicity property).
- **Multiple drives** — abstracted away; the model focuses on per-key serialization regardless of which drive the data lives on.
- **Dispatch-map condvar** — the wait-for-write_ref==0 is modeled as an atomic guard (threads that can't proceed simply skip, modeling the timeout path).

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

Parameters: `N_THREADS=3`, `N_KEYS=2`

- `N_THREADS=3` with `N_KEYS=2` means thread 0 and thread 2 both target key 0, while thread 1 targets key 1. This creates the interesting double-promotion race on key 0.
- The nondeterministic SSD read exercises both the success and failure cleanup paths.
- All thread interleavings between the race window (after `release_read`, before `dm.remove`) are explored.

Why these are sufficient:
1. The promotion protocol is per-key — if safe for 2 competing threads on one key, it's safe for N.
2. Two keys verify that per-key state doesn't leak across keys.
3. Three threads ensure the model covers both "two racers" and "one uncontested" patterns.

To explore larger state spaces:
```bash
spin -DN_THREADS=4 -DN_KEYS=2 -a promotion_atomicity.pml
cc -O2 -DSAFETY -DMEMLIM=8192 -o pan pan.c
./pan -m200000
```

## Correspondence to Source Code

| Model location | Source file | Line range |
|----------------|-------------|------------|
| `LookupThread` / dm.lookup | `components/dispatch-map/src/lib.rs` | 143–183 |
| `LookupThread` / release_read before promote | `components/dispatcher/src/lib.rs` | 1324 |
| `LookupThread` / mt.insert | `components/dispatcher/src/lib.rs` | 1151–1156 (batch) / 208–211 (single) |
| `LookupThread` / dm.remove | `components/dispatcher/src/lib.rs` | 1177 (batch) / 280 (single) |
| `LookupThread` / create_memory_tier_entry | `components/dispatcher/src/lib.rs` | 1178–1184 (batch) / 281 (single) |
| `LookupThread` / release_write | `components/dispatcher/src/lib.rs` | 1187 (batch) / 285 (single) |
| dm.remove (ref check) | `components/dispatch-map/src/lib.rs` | 336–354 |
| dm.create_memory_tier_entry (AlreadyExists) | `components/dispatch-map/src/lib.rs` | 377–389 |
