# certus-connector

vLLM **OffloadingSpec** plugin for the Certus storage system. Implements vLLM's `OffloadingSpec` ABC so that `OffloadingConnectorScheduler` can offload KV cache blocks to tiered DRAM + raw NVMe storage via SPDK.

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
  ├─ get_manager() → OffloadingManager     ← allocation / eviction decisions
  │     ├─ NativeCertusOffloadingManager       (production, backed by Rust)
  │     └─ CertusOffloadingManager             (mock, pure Python for testing)
  │
  └─ get_handlers() → OffloadingHandler    ← actual GPU ↔ storage DMA
        ├─ GpuToCertusHandler                  (store: GPU → DRAM staging → NVMe)
        └─ CertusToGpuHandler                  (load:  NVMe/DRAM → GPU)
```

This is the same plugin contract that llm-d's `SharedStorageOffloadingSpec` uses. The difference: llm-d uses POSIX files on shared storage, we use raw NVMe via SPDK with no filesystem.

## Rust engine (certus_native)

The Python handlers delegate to a Rust PyO3 extension module (`certus_native`) which assembles and wires the Certus component stack:

```
certus_native.CertusEngine                 ← PyO3 class (assembler, not a component)
  │
  │  instantiates & connects:
  │
  ├─ dispatcher        components/dispatcher/v0/       orchestrates cache ops
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
| `complete_store(keys, success)` | `()` | If `success=True`: mark blocks as ready (now loadable) and unpin. If `success=False`: remove the blocks entirely (rollback allocation). |
| `prepare_load(keys)` | `LoadStoreSpec` | Pin blocks for reading (protected from eviction). Returns location info for the handler to perform DMA. Assumes all given keys are already stored and ready. |
| `complete_load(keys)` | `()` | Unpin blocks (allow eviction again). Must be called after load DMA completes. |
| `touch(keys)` | `()` | Update LRU ordering — marks blocks as recently used. May trigger promotion to faster tier. Called even for GPU-cached blocks that don't need loading. |
| `take_events()` | `Iterable[OffloadingEvent]` | Yield new events (stored/evicted) since last call. Consumed by vLLM for accounting. |
| `shutdown()` | `()` | Release all resources. |

### Key invariants

1. **Eviction only from `prepare_store`** — the only trigger for freeing capacity.
2. **Pinning protects from eviction** — blocks between `prepare_*` and `complete_*` cannot be evicted.
3. **Blocks not loadable until `complete_store(success=True)`** — prevents reading partially-written data.
4. **`None` return from `prepare_store` = hard rejection** — vLLM will not retry automatically.
5. **`None` return from `lookup` = soft delay** — vLLM scheduler retries the request later.
6. **Consecutive prefix semantics** — `lookup` returns the longest prefix of hits, not total hit count.

### Native Rust API mapping

There are three layers. Only the bottom one (Rust components) needs new work:

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Layer 1: Python shim (native_manager.py)                               │
│  Converts OffloadKey bytes → u64, constructs PrepareStoreOutput.        │
│  NO logic here — pure adapter. Stays as-is.                             │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ calls via PyO3
┌───────────────────────────────────▼─────────────────────────────────────┐
│  Layer 2: CertusEngine (src/engine.rs)                                  │
│  Wires components, translates between PyO3 types and Rust traits.       │
│  Orchestrates calls to dispatcher + dispatch-map.                       │
│  Needs updating once dispatch-map exposes eviction/ref-count APIs.      │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ calls via component interfaces
┌───────────────────────────────────▼─────────────────────────────────────┐
│  Layer 3: Rust components (Daniel's work)                               │
│  dispatch-map: threshold LRU, ref-counting, evict_lru(n, protected)     │
│  dispatcher: integrate eviction into prepare_store path                 │
└─────────────────────────────────────────────────────────────────────────┘
```

Per-method breakdown:

| `native_manager.py` calls | `CertusEngine` method | Rust component work | Status |
|---|---|---|---|
| `lookup(keys)` | `batch_check(keys)` | `dispatcher.check()` per key | **Done** |
| `prepare_store(keys)` | `prepare_store(keys)` | Dispatch-map: `evict_lru(n, protected)` when full; dispatcher: remove evicted, allocate new | **Daniel: in progress** |
| `complete_store(keys, ok)` | `complete_store(keys, ok)` | On failure: `dispatcher.remove()` per key. On success: mark ready in dispatch-map. | **Partially done** (remove works, readiness gating TBD) |
| `touch(keys)` | `touch(keys)` | Dispatch-map: update threshold LRU ordering | **Daniel: in progress** |
| `prepare_load(keys)` | (not wired yet) | Dispatch-map: increment `ref_cnt` (eviction protection only, no physical pin) | **Needs implementing** |
| `complete_load(keys)` | (no-op) | Dispatch-map: decrement `ref_cnt` | **Needs implementing** |
| `shutdown()` | `shutdown()` | `dispatcher.shutdown()` + `gpu.shutdown()` | **Done** |

### What Daniel needs to add to dispatch-map

```rust
// New methods on IDispatchMap (or a new IEvictionPolicy trait):

/// Update LRU ordering for key (threshold-based).
fn touch(&self, key: CacheKey);

/// Increment ref_cnt — block protected from eviction while ref > 0.
fn pin(&self, key: CacheKey) -> Result<(), Error>;

/// Decrement ref_cnt.
fn unpin(&self, key: CacheKey) -> Result<(), Error>;

/// Evict up to `count` LRU blocks, skipping pinned (ref_cnt > 0) and protected set.
/// Returns evicted keys, or None if cannot satisfy `count` evictions (atomic).
fn evict_lru(&self, count: usize, protected: &HashSet<CacheKey>) -> Option<Vec<CacheKey>>;

/// Mark block as ready (loadable). Called after successful store.
fn mark_ready(&self, key: CacheKey);
```

Once these exist, `engine.rs` orchestrates them in `prepare_store`:
```
1. Filter already-cached keys
2. Check capacity: need = to_store.len() - free_space
3. If need > 0: call dispatch_map.evict_lru(need, protected_set)
   - If None → return None (cannot store)
   - Else → dispatcher.remove() each evicted key
4. Allocate via dispatcher.populate() for each new key
5. Return (keys_to_store, evicted_keys)
```

### Eviction and tier management

**Eviction** (block removed entirely, capacity freed) is triggered **only** by `prepare_store`.
This matches vLLM's own CPU offloading manager — there is no background eviction, timer-based
eviction, or memory-pressure eviction in the contract. It is purely demand-driven.

There are three distinct space-management operations:

| Operation | Trigger | Effect | Block still accessible? |
|-----------|---------|--------|------------------------|
| **Eviction** | `prepare_store` (NVMe full) | NVMe slab freed, DRAM slot freed, key removed from index | No — gone entirely |
| **Demotion** | `touch` → promotion needs a DRAM slot | Coldest DRAM slot freed, data remains on NVMe | Yes — loadable from NVMe |
| **Idle demotion** | Background timer (optional) | Idle DRAM slots freed after timeout | Yes — loadable from NVMe |

Only **eviction** is required by the vLLM contract. Demotion is an internal optimization
for managing the DRAM tier and is invisible to vLLM.

### What the native Rust path must support

| # | Requirement | Status | Notes |
|---|-------------|--------|-------|
| 1 | **Eviction in `prepare_store`** | In progress (Daniel) | On-demand only: when extent manager is full, query dispatch-map for LRU victims with `ref_cnt == 0`, call `dispatcher.remove()`, retry allocation. No background eviction thread — `prepare_store` is the sole trigger. |
| 2 | **LRU ordering in `touch`** | In progress (Daniel) | Threshold LRU — dispatch-map tracks access order so eviction picks the coldest block. Updated on `touch`, scanned on `prepare_store`. No background sweep needed. |
| 3 | **Ref-counting (`prepare_load` / `complete_load`)** | Not yet implemented | Pinned blocks (`ref_cnt > 0`) must be skipped during eviction. Currently `complete_load` is a no-op. |
| 4 | **Readiness gating** | Partially implemented | Blocks must not be returned by `lookup` or `prepare_load` until `complete_store(success=True)`. Dispatcher's `check()` may already handle this if dispatch-map tracks readiness. |
| 5 | **Atomic eviction** | Not yet implemented | If N evictions are requested but fewer than N unpinned blocks exist, evict nothing and return `None`. Must be all-or-nothing. |
| 6 | **Protected set in eviction** | Not yet implemented | Keys in the current `prepare_store` input must not be evicted (they might already be cached and must remain). |
| 7 | **Demotion (optional, v1)** | Deferred | DRAM tier management. Dispatcher already stages in DRAM and migrates to NVMe in background, but no explicit slot reclamation under DRAM pressure yet. Not required by vLLM contract. |

### Native path differences from mock

The mock Python manager models a generic cache. The native Rust path has hardware-specific
nuances that simplify some operations:

| Aspect | Mock (Python) | Native (Rust + SPDK) |
|--------|---------------|----------------------|
| **Host memory** | Allocated/freed per block | Pre-allocated SPDK DMA buffer pool — all pinned at init |
| **Pin/unpin on load** | Conceptually pins memory for DMA | No-op physically — memory is always pinned. `ref_cnt` only prevents eviction. |
| **GPU DMA registration** | Would need `cudaHostRegister` per buffer | DMA buffers are pre-registered. `dma_copy_to_host`/`dma_copy_to_device` use them directly. |
| **Capacity** | Configurable slot counts | Fixed at init — extent manager knows total slabs from NVMe device size, DRAM pool from config. |
| **Staging** | Explicit DRAM tier with promotion/demotion | Dispatcher stages ALL writes in DRAM first, background thread migrates to NVMe. DRAM is a write-through cache, not a separate tier to manage. |

**Key implication for `prepare_load`/`complete_load`**: these are purely logical ref-count
operations in the native path. No memory is allocated, pinned, or registered — only the
eviction-protection semantics matter.

**Key implication for capacity**: `prepare_store` returning `None` means the NVMe extent
manager is full AND there are not enough unpinned blocks to evict. The DRAM staging pool
cannot overflow because the dispatcher controls admission.

### gRPC handler equivalence

If implementing a gRPC service fronting the Rust components directly (bypassing Python),
the handlers must preserve these same semantics — particularly:

- Eviction only from `prepare_store` (no background/timer eviction)
- Pinning bracket: blocks between `prepare_*` and `complete_*` cannot be evicted
- Atomic eviction: either free enough space or reject entirely (`None`)
- Protected set: don't evict keys that are in the current store request
- Readiness: blocks not loadable until `complete_store(success=True)`
