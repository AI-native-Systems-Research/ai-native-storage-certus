# Certus ↔ vLLM OffloadingSpec API Mapping

## Overview

This document maps the vLLM `OffloadingManager` / `OffloadingConnector` interface
(the KV-cache offloading spec) to the Certus shmq `Dispatcher` ops (the
opcode-framed shared-memory wire in `lib/shmq-dispatcher/src/wire.rs`).

The Python connector package is `certus_shmq_connector` (dir
`certus-shmq-connector/`). vLLM `OffloadingConnector` is configured with
`spec_name=CertusShmqOffloadingSpec`, `spec_module_path=certus_shmq_connector.spec`,
and `shm_path=/dev/shm/certus-shmq` — the connector opens the `/dev/shm` mailbox
(shared via `--ipc=host`) instead of dialing a server address.

## Mapping Table

| OffloadingSpec Method | Direction | Certus shmq op | Notes |
|---|---|---|---|
| `prepare_store(keys)` | Scheduler → Store | `Reserve` | Allocates DRAM slots; entries invisible until committed |
| `transfer_async` (store) | Worker → Store | `CopyToStore` | DMA from GPU into reserved DRAM slot via IPC handle |
| `complete_store(keys, success=True)` | Scheduler → Store | `CommitStore` | Registers entry in dispatch-map; enqueues SSD write-through |
| `complete_store(keys, success=False)` | Scheduler → Store | `AbortStore` | Discards reserved slot; frees DRAM without registration |
| `lookup(keys)` | Scheduler | `Check` | Returns exists/not-exists per key (no data transfer) |
| `prepare_load(keys)` | Scheduler → Load | `Pin(promote=true)` | Single call: pins + promotes SSD→DRAM in one round-trip |
| `transfer_async` (load) | Worker → Load | `Lookup` | DMA from DRAM/SSD to GPU via IPC handle |
| `complete_load(keys)` | Scheduler → Load | `Unpin` | Decrements pin refcount; entry eligible for eviction at zero |
| `touch(keys)` | Scheduler | `Touch` | Updates eviction timestamp (LRU refresh) |
| `take_events()` | Scheduler | `TakeEvents` | Drains eviction notifications since last poll |

## Auxiliary ops (no direct OffloadingSpec equivalent)

| Certus shmq op | Purpose |
|---|---|
| `Populate` | Single-phase store (Reserve+Copy+Commit atomically) — used by benchmarks |
| `Remove` | Explicit entry deletion (not triggered by OffloadingSpec; used by connector shutdown) |
| `ClearMemoryTier` | Bulk-clear all DRAM cache entries (admin/recovery) |
| `FlushToSsd` | Force all pending write-through jobs to complete (graceful shutdown) |

## Lifecycle Sequence

```
vLLM Scheduler                    Certus Server
─────────────                    ─────────────
prepare_store(keys)        ──→   Reserve(keys, sizes)
                                   ↓ (invisible DRAM slots allocated)
[worker] transfer_async    ──→   CopyToStore(keys, IPC handles)
                                   ↓ (GPU→DRAM DMA complete)
complete_store(success)    ──→   CommitStore(keys)
                                   ↓ (entries visible, SSD write-through enqueued)
                                   
lookup(key)                ──→   Check(keys)
                                   ↓ (exists=true)
                                   
prepare_load(keys)         ──→   Pin(keys, promote=true)
                                   ↓ (entries protected from eviction, promoted if on SSD)
[worker] transfer_async    ──→   Lookup(keys, IPC handles)
                                   ↓ (DRAM→GPU DMA complete)
complete_load(keys)        ──→   Unpin(keys)
                                   ↓ (entries eligible for eviction again)
                                   
touch(keys)                ──→   Touch(keys)
                                   ↓ (LRU timestamp refreshed)
                                   
take_events()              ──→   TakeEvents(max_events=0)
                                   ↓ (returns evicted key list + reasons)
```

## Eviction Events

| `EvictionReason` | Meaning | Scheduler Action |
|---|---|---|
| `DEMOTED` | Entry moved DRAM→SSD, still accessible | Optional: no metadata invalidation needed |
| `REMOVED` | Entry data lost, key no longer accessible | Required: invalidate block hash from scheduler cache |

## Channel Semantics

- Bounded channel (16384 slots); overflow drops oldest events
- `dropped_count` in response indicates events lost since last drain
- vLLM calls `take_events()` once per scheduler step (~every 10–50ms)
- Non-blocking: returns immediately with available events (0 if none)

## Pin Semantics

- Pin is reference-counted: multiple `prepare_load` calls increment
- Pinned entries are skipped by the LRU evictor
- Pin on non-existent key returns `KeyNotFound`
- Unpin below zero returns error (refcount underflow protection)
