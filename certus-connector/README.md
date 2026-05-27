# certus-connector

vLLM **OffloadingSpec** plugin for the Certus storage system. Implements vLLM's `OffloadingSpec` ABC so that `OffloadingConnectorScheduler` can offload KV cache blocks to tiered DRAM + raw NVMe storage via SPDK. Works with llm-d (which uses vLLM as its runtime).

Single installable package providing both the native Rust engine (PyO3) and the Python vLLM adapter.

## How it fits into vLLM

```
vLLM OffloadingConnectorScheduler          ← vLLM's internal scheduler
  │                                           (we do NOT implement this)
  │  loads via kv_connector_extra_config:
  │    spec_module_path = "certus_connector.spec"
  │    spec_name = "CertusOffloadingSpec"
  │
  ▼
CertusOffloadingSpec (OffloadingSpec)       ← OUR plugin entry point
  │
  │  creates ONE shared CertusEngine instance:
  │
  ├─ get_manager() → OffloadingManager     ← allocation / eviction decisions
  │     ├─ NativeCertusOffloadingManager       (production, wraps CertusEngine)
  │     └─ CertusOffloadingManager             (mock, pure Python for testing)
  │
  └─ get_handlers() → OffloadingHandler    ← actual GPU ↔ storage DMA
        ├─ GpuToCertusHandler(engine)          (store: GPU → DRAM staging → NVMe)
        └─ CertusToGpuHandler(engine)          (load:  NVMe/DRAM → GPU)
                                  ↑
                    same CertusEngine instance as manager
```

This is the same plugin contract that llm-d's `SharedStorageOffloadingSpec` uses. The difference: llm-d uses POSIX files on shared storage, we use raw NVMe via SPDK with no filesystem.

## Rust engine (certus_native)

A single `CertusEngine` instance is shared between the manager (index/allocation/eviction) and the handlers (GPU DMA transfers). This ensures the handler can find data that the manager stored. The engine is a Rust PyO3 extension module (`certus_native`) which assembles and wires the Certus component stack:

```
certus_native.CertusEngine                 ← PyO3 class (assembler, not a component)
  │
  │  instantiates & connects:
  │
  ├─ dispatcher        components/dispatcher/v1/       orchestrates cache ops
  ├─ dispatch-map      components/dispatch-map/v0/     key → location index
  ├─ gpu-services      components/gpu-services/v0/     CUDA DMA transfers
  └─ spdk-env          components/spdk-env/            SPDK environment init
```

These are reusable Rust components (defined with `define_component!`) that live in the repo under `components/`. The `CertusEngine` is the application-level assembler — it creates each component, connects their receptacles (typed dependency slots), and exposes the combined API to Python.

The dispatcher internally creates NVMe block devices and extent managers during `initialize()` based on the PCI addresses in config.

## Package contents

| Path | What |
|------|------|
| `src/lib.rs` | PyO3 module definition — `CertusEngine` class |
| `src/engine.rs` | Wires the Rust component stack (creates, connects, initializes) |
| `src/keys.rs` | OffloadKey (u64) to CacheKey mapping |
| `certus_connector/spec.py` | `CertusOffloadingSpec` — vLLM OffloadingSpec implementation |
| `certus_connector/manager.py` | Mock manager (pure Python, for testing without hardware) |
| `certus_connector/native_manager.py` | Production manager (thin proxy to `certus_native.CertusEngine`) |
| `certus_connector/handler.py` | Transfer handlers (GPU ↔ Certus I/O) |
| `certus_connector/mediums.py` | `CertusLoadStoreSpec` medium definition |

## Build

Requires SPDK and CUDA for full native build. Without hardware, the mock manager path works for development/testing.

```bash
# Python tests (no hardware needed)
python3 -m pytest tests/ -v

# Full build (requires SPDK + CUDA)
pip install -e .

# Rust type-check only (will fail at spdk-sys link without SPDK libs)
cargo check -p certus-connector
```

## vLLM configuration

```json
{
    "spec_name": "CertusOffloadingSpec",
    "spec_module_path": "certus_connector.spec",
    "data_pci_addrs": ["0000:02:00.0"],
    "metadata_pci_addr": "0000:01:00.0",
    "slab_size_bytes": 131072,
    "dram_cache_bytes": 8589934592,
    "io_queue_depth": 128
}
```

Set `"use_native": false` to force the mock manager (for testing without hardware).

## OffloadingManager semantics (native path contract)

The native Rust path must implement these semantics. This is the contract that
vLLM's `OffloadingConnectorScheduler` calls on the manager returned by
`CertusOffloadingSpec.get_manager()`.

### Method reference

| Method | Returns | Semantics |
|--------|---------|-----------|
| `lookup(keys)` | `int \| None` | Count of **consecutive** keys (from start) that are cached and ready. Stops at first miss. Return `None` to signal "retry later" (delays vLLM scheduler). |
| `prepare_store(keys)` | `PrepareStoreOutput \| None` | Reserve space for new keys. Evict LRU if capacity exceeded. Returns which keys need storing, their locations, and which keys were evicted. Returns `None` if storage is impossible (cannot free enough space). Allocated blocks are **pinned** (protected from eviction) until `complete_store`. |
| `complete_store(keys, success)` | `()` | If `success=True`: no-op (blocks already readable after `populate`). If `success=False`: remove the blocks entirely (rollback allocation). |
| `prepare_load(keys)` | `LoadStoreSpec` | Pin blocks for reading (protected from eviction). Returns location info for the handler to perform DMA. Assumes all given keys are already stored and ready. |
| `complete_load(keys)` | `()` | Unpin blocks (allow eviction again). Must be called after load DMA completes. |
| `touch(keys)` | `()` | Update LRU ordering — marks blocks as recently used. Called even for GPU-cached blocks that don't need loading. |
| `take_events()` | `Iterable[OffloadingEvent]` | Yield new events (stored/evicted) since last call. Consumed by vLLM for accounting. |
| `shutdown()` | `()` | Release all resources. |

### Key invariants

1. **Eviction only from `prepare_store`** — the only trigger for freeing capacity.
2. **Pinning protects from eviction** — blocks between `prepare_*` and `complete_*` cannot be evicted.
3. **Blocks readable immediately after `populate()`** — GPU→DRAM copy completes atomically during store, no gating needed.
4. **`None` return from `prepare_store` = hard rejection** — vLLM will not retry automatically.
5. **`None` return from `lookup` = soft delay** — vLLM scheduler retries the request later.
6. **Consecutive prefix semantics** — `lookup` returns the longest prefix of hits, not total hit count.

### Native Rust API mapping

Three layers:

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Layer 1: Python shim (native_manager.py)                               │
│  Converts OffloadKey bytes → u64, constructs PrepareStoreOutput.        │
│  Pure adapter — no logic.                                               │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ calls via PyO3
┌───────────────────────────────────▼─────────────────────────────────────┐
│  Layer 2: CertusEngine (src/engine.rs)                                  │
│  Wires components, translates between PyO3 types and Rust traits.       │
│  Orchestrates eviction, ref-counting, entry tracking.                   │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ calls via component interfaces
┌───────────────────────────────────▼─────────────────────────────────────┐
│  Layer 3: Rust components                                               │
│  dispatch-map: LRU ordering, ref-counting, oldest_keys()                │
│  dispatcher: populate, lookup, remove, touch, evict_for_space           │
└─────────────────────────────────────────────────────────────────────────┘
```

Per-method breakdown:

| `native_manager.py` calls | `CertusEngine` method | Rust component work | Status |
|---|---|---|---|
| `lookup(keys)` | `batch_check(keys)` | `dispatcher.check()` per key | **Done** |
| `prepare_store(keys)` | `prepare_store(keys)` | Filters cached keys, evicts LRU via `dispatcher.remove()` when over watermark, returns `None` if can't free enough | **Done** |
| `complete_store(keys, ok)` | `complete_store(keys, ok)` | On failure: `dispatcher.remove()` per key. On success: no-op (entry already readable after `populate`). | **Done** |
| `touch(keys)` | `touch(keys)` | Dispatch-map: update threshold LRU ordering | **Done** |
| `prepare_load(keys)` | `prepare_load(keys)` | Dispatch-map: `lookup()` (increments `read_ref`, blocks eviction, returns storage offset) | **Done** |
| `complete_load(keys)` | `complete_load(keys)` | Dispatch-map: `release_read()` (decrements `read_ref`) | **Done** |
| `shutdown()` | `shutdown()` | `dispatcher.shutdown()` + `gpu.shutdown()` | **Done** |

### How eviction works

The engine tracks total cached entries via an atomic `entry_count`:
- Incremented after successful `dispatcher.populate()` (in `store_async`) and `dispatcher.commit_store()` (in `store_host_bytes`)
- Decremented after successful `dispatcher.remove()` (in `prepare_store` eviction and `complete_store` rollback)

When `prepare_store` is called and `entry_count + to_store.len() > eviction_watermark`:
1. Scan `dispatch_map.oldest_keys(MAX)` for LRU-ordered candidates
2. Skip keys in the protected set (keys in the current store request)
3. Call `dispatcher.remove(candidate)` — fails silently for pinned entries (active `read_ref > 0`)
4. If enough entries freed → return `(to_store, evicted)`. If not → return `None`

`eviction_watermark = max_cache_entries * eviction_threshold` (both from config).

Returning `None` is **required** because vLLM's worker asserts `transfer_result.success` (worker.py:348) — store failures crash the process. The only safe capacity signal is rejecting at `prepare_store` before the handler is called.

**Why llm-d FS backend doesn't need this:** its handler writes to a POSIX shared filesystem (Lustre, CephFS) — no fixed DRAM pool, no allocation failure. `write()` doesn't fail due to capacity. **Why we do:** our `dispatcher.populate()` allocates from a fixed-size DRAM staging pool. When full and nothing is evictable (all entries mid-background-write), `populate()` returns `AllocationFailed`, `store_async` returns `false`, and the worker assert crashes the process. Proactive eviction prevents this by ensuring capacity exists before the handler is called.

### Notes on `prepare_load` ref-counting

Manager and handler share one `CertusEngine` instance (one dispatch-map). The flow:
`prepare_load` → `dm.lookup()` (ref=1), `load_async` → `dispatcher.lookup()` → `dm.lookup()` (ref=2) → DMA → `dm.release_read()` (ref=1), `complete_load` → `dm.release_read()` (ref=0). The transient double-ref during DMA is harmless — it gives extra eviction protection while the transfer is in flight. One redundant atomic op per block per load, but correct.

### Eviction and tier management

**Eviction** (block removed entirely, capacity freed) is triggered **only** by `prepare_store`.
This matches vLLM's own CPU offloading manager — there is no background eviction, timer-based
eviction, or memory-pressure eviction in the contract. It is purely demand-driven.

There are two distinct space-management operations in the native path:

| Operation | Trigger | Effect | Block still accessible? |
|-----------|---------|--------|------------------------|
| **Eviction** | `prepare_store` (entry count > watermark) | Entry removed from dispatch-map, memory-tier, and NVMe extent freed | No — gone entirely |
| **DRAM demotion** | `dispatcher.populate()` when DRAM staging is full | Coldest DRAM slot freed via `evict_for_space()`, data remains on NVMe | Yes — loadable from NVMe |

Eviction is the vLLM contract requirement. DRAM demotion is internal to the dispatcher — it happens automatically during `populate()` to make room for incoming data, invisible to vLLM.

### What the native Rust path must support

| # | Requirement | Status | Notes |
|---|-------------|--------|-------|
| 1 | **Eviction in `prepare_store`** | **Done** | Engine tracks `entry_count` atomically, evicts LRU via `dispatcher.remove()` when over watermark, returns `None` if can't free enough. |
| 2 | **LRU ordering in `touch`** | **Done** | Engine calls `dispatcher.touch()` per key. |
| 3 | **Ref-counting (`prepare_load` / `complete_load`)** | **Done** | `prepare_load` calls `dispatch_map.lookup()` (increments `read_ref` + returns location), `complete_load` calls `release_read()`. Manager and handler share one engine instance — transient double-ref during DMA is balanced (dispatcher releases its ref after DMA completes). Blocks with `read_ref > 0` are skipped during eviction (`remove` fails with `ActiveReferences`). |
| 4 | **Readiness gating** | **N/A** | Non-issue: `populate()` writes data to DRAM and registers in dispatch-map immediately — block is readable before `complete_store`. `commit_store()` only persists to NVMe. |
| 5 | **Atomic eviction** | **Done** | If N evictions are requested but fewer than N unpinned blocks exist, returns `None`. All-or-nothing semantics. |
| 6 | **Protected set in eviction** | **Done** | Keys in the current `prepare_store` input are in a `protected` HashSet and skipped during eviction. |
| 7 | **DRAM demotion** | **Done** | Handled internally by dispatcher's `evict_for_space()` during `populate()` — evicts LRU DRAM slots and transitions entries to BlockDevice when DRAM is full. Not part of vLLM contract. |

### Native path characteristics

- **Host memory**: pre-allocated SPDK DMA buffer pool — all pinned at init. No per-block allocation.
- **`prepare_load`/`complete_load`**: purely logical ref-count operations. No memory allocated or registered — only eviction-protection semantics matter.
- **Capacity**: fixed at init — extent manager knows total slabs from NVMe device size, DRAM pool from config.
- **Staging**: dispatcher stages ALL writes in DRAM first, background thread migrates to NVMe. DRAM is a write-through cache.

### gRPC handler equivalence

If implementing a gRPC service fronting the Rust components directly (bypassing Python),
the handlers must preserve these same semantics — particularly:

- Eviction only from `prepare_store` (no background/timer eviction)
- Pinning bracket: blocks between `prepare_*` and `complete_*` cannot be evicted
- Atomic eviction: either free enough space or reject entirely (`None`)
- Protected set: don't evict keys that are in the current store request

