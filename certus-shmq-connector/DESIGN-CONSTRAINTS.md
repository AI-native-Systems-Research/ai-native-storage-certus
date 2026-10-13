# certus-shmq-connector: design constraints

This file gives the rules of the vLLM KV-offload connector that vLLM and the Certus server depend on.
If a change breaks one of these rules, vLLM can stop, or vLLM can give incorrect output without an error.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../knowledge/design-constraints.md).

This package has no spec. The module docstrings and comments are the contract. Paths below are relative to `certus_shmq_connector/`.

## CONN-1. A load must not fail for a key that `prepare_load` pinned

- **Rule:** Make sure that the server can serve each key that `lookup` reports as present, until `complete_load`. Do not add a failure on the load path that the connector can avoid.
- **Reason:** `_do_load` returns `False` if any key is not served. vLLM asserts that each transfer succeeds, so a `False` stops the engine. See DC-6 in the main document.
- **Applies to:** `handler.py`, `manager.py`, and the server Pin, Lookup and eviction code.
- **Status:** Established by code.
- **Evidence:** `handler.py:325-347` (comment at line 333: "a load must not fail (vLLM asserts)"); the order rule "pin-before-load → vLLM EngineDeadError" in the workbench model `targets/evolve-coldload/experiments/s20261009-211818/models/v001/temporal.yaml:5-37`.

## CONN-2. The store path always reports success

- **Rule:** Return `True` from `_do_store` in all cases. If a copy fails, abort the reservation of each failed key (all keys if the transport fails) before the commit.
- **Reason:** vLLM stops on a failed transfer, but a store that fails only means that the block is not cached. Because of the abort, `CommitStore` cannot make an empty slot visible.
- **Applies to:** `handler.py` store body, `manager.py` `complete_store`.
- **Status:** Established by code.
- **Evidence:** `handler.py:284-323` (comment at line 299: "the store path must NEVER report success=False").

## CONN-3. Pin before Lookup, without promotion, and Unpin after the load

- **Rule:** Keep the lifecycle: `prepare_load` → `Pin(promote=False)`, the worker → `Lookup`, `complete_load` → `Unpin`. Do not set `promote=True` on Pin.
- **Reason:** The pin keeps the entry from eviction during the load. Lookup already promotes cold entries. Two promotions of the same key fail with `AlreadyExists`, the load fails, and vLLM stops.
- **Applies to:** `manager.py`, `handler.py`, `shmq-dispatcher` Pin.
- **Status:** Established by code. Note: the comment records that dispatcher spec FR-040 calls the promotion a detached background task, but the server runs it inline.
- **Evidence:** `manager.py:6-17`; `manager.py:398-423`; `manager.py:437-441`.

## CONN-4. Count only `RESIDENT` keys as a load hit

- **Rule:** For a load decision (`lookup`, `touch`), count a key as present only if its Check state is `RESIDENT`. For store deduplication, count `RESIDENT` and `PENDING`.
- **Reason:** A `PENDING` key has no dispatch-map entry yet. If vLLM counts it as a hit, the load fails and vLLM stops.
- **Applies to:** `manager.py` `_check_all_present`.
- **Status:** Established by code.
- **Evidence:** `manager.py:135-168`.

## CONN-5. Store and load must use the same key fold

- **Rule:** Calculate the server key with `_key_to_u64` and then `ns_key(key, rank, world_size)` on both the store path and the load path. Use the real tensor-parallel rank of vLLM.
- **Reason:** With tensor parallelism, each rank stores a different shard under the same block hash. A different fold on the two paths loads the shard of a different rank, and vLLM gives incorrect output without an error. The keys are also stored on the SSD, so a change to the fold makes stored data unreachable.
- **Applies to:** `mediums.py`, `manager.py`, `handler.py`, `spec.py`.
- **Status:** Established by code.
- **Evidence:** `mediums.py:16-46`; `manager.py:38-53`; `manager.py:128-133`; `handler.py:277`, `handler.py:327`.

## CONN-6. Reserve at least the size of the copy to DRAM

- **Rule:** Calculate the Reserve size for each block from the configuration. Then the scheduler and the worker get the same value. Do not use a value smaller than the GPU-to-DRAM copy.
- **Reason:** vLLM makes two instances of the connector. The scheduler instance cannot see the KV tensor. If the slot is too small, each copy fails its bounds check, and the cache gets no blocks. No error shows, because the store path always reports success (CONN-2).
- **Applies to:** `spec.py`.
- **Status:** Established by code.
- **Evidence:** `spec.py:140-150`.
