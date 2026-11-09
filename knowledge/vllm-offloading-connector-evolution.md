# vLLM OffloadingConnector Evolution (v0.11 → v0.27)

This document traces the evolution of vLLM's KV offloading connector interface
from its introduction in v0.11.0 through the latest v0.27.1 release. Every
claim is verified against the actual source at `/home/nara/certus/related/vllm`
(pulled 2026-08-12, latest tag v0.27.1, HEAD `fe889ac925`).

## Document Structure

1. [Design Context](#design-context) — why the offloading stack exists
2. [Era Overview](#era-overview) — four major API eras at a glance
3. [Version-by-Version Changelog](#version-by-version-changelog) — what changed per release
4. [Dimension Reference](#dimension-reference) — each API dimension explained
5. [Full Capability Matrix](#full-capability-matrix) — all dimensions × all versions

---

## Design Context

The offloading connector enables asynchronous KV cache offloading between GPU
HBM and a secondary medium (CPU DRAM, remote storage, or any pluggable backend).
It was introduced as a multi-PR RFC in 2025:

- **PR #19848**: Generic offloading component (scheduler-side `OffloadingManager`
  + worker-side async transfer management)
- **PR #20075**: LRU-based CPU offloading management with pluggable backend
- **PR #21448**: Worker-side CPU support using `swap_blocks`
- **PR #22595**: `OffloadingConnector` as the KV connector wrapper
- **PR #24251**: `CPUOffloadingSpec` registration — first complete CPU offload path

Key design decisions (from the [vLLM blog post](https://vllm.ai/blog/2026-01-08-kv-offloading-connector)):

1. **Asynchronous API** (v0.9.0+): offloading/loading runs in parallel with
   model computation
2. **Pluggable backend architecture**: implement a transfer function, get a
   working offload backend
3. **Contiguous memory layout** (v0.12.0, PR #27743): all layers packed into one
   physical block (32KB→2MB), enabling efficient DMA
4. **DMA over custom CUDA kernel**: avoids GPU core contention, yields up to 32%
   better throughput

---

## Era Overview

The connector interface can be divided into four eras:

| Era | Versions | Spec Constructor | Worker API | Lookup Return |
|-----|----------|-----------------|-----------|---------------|
| **Genesis** | v0.11–v0.13 | `spec_cls(vllm_config)` | `get_handlers(kv_caches, attn_backends)` → `OffloadingHandler` | `int` (prefix match count) |
| **Geometry-Aware** | v0.14–v0.19 | `spec_cls(vllm_config, kv_cache_config)` | `get_handlers(kv_caches)` (canonical from v0.19) → `OffloadingHandler` | `int \| None` |
| **Per-Key + Context** | v0.20–v0.24 | `spec_cls(vllm_config, kv_cache_config)` | `get_handlers(kv_caches)` → `OffloadingHandler` | `bool \| None` (per `OffloadKey`) |
| **Clean Backend** | v0.25–v0.27 | v0.25: `spec_cls(vllm_config, kv_cache_config)` / v0.26+: `spec_cls(offloading_config)` | `get_worker(kv_caches)` → `OffloadingWorker` | `LookupResult` enum |

---

## Major Design Decisions and Rationale

This section documents the significant architectural decisions in the offloading
connector's evolution — not just what changed, but why, and what problem each
shift solved.

### Batch Lookup → Per-Key Lookup (v0.20)

**The old model (v0.11–v0.19):**
`lookup(block_hashes: Iterable[BlockHash]) -> int` — "how many consecutive
blocks from this prefix are cached?" Returns a prefix-match count.

**The problem:** This assumed cache residency is always a contiguous prefix.
In practice, offload tier residency becomes sparse:

1. **Partial eviction** — under LRU/ARC pressure, middle blocks of a prefix get
   evicted while head and tail survive (blocks 0,2,4 cached; 1,3 evicted)
2. **Best-effort store** — when the memory tier is saturated, `Reserve` fails for
   some blocks in a batch but succeeds for others. The connector stores what fits
   and drops the rest (Certus does this explicitly). Result: gaps in coverage.
3. **Store gating** (`store_threshold`, v0.18+) — blocks seen only once are never
   stored. First occurrence of a prompt stores nothing; second occurrence finds
   only blocks that appeared in a prior request. Coverage depends on workload
   overlap, not prefix contiguity.
4. **Multi-group layouts (GQA/MQA)** — a single block hash can map to different
   physical blocks across cache groups. One group's block might be evicted while
   another group's survives. The flat `BlockHash` cannot distinguish them.

**The new model (v0.20+):**
`lookup(key: OffloadKey, req_context: ReqContext) -> bool | None`

Per-key lookup lets the scheduler know exactly which blocks are available,
regardless of contiguity. The scheduler can then:
- Load only the hits (skip recomputation for those tokens)
- Recompute only the misses (no wasted prefill on already-cached blocks)
- Route lookups per-request via `req_context` (session-scoped caching)

**Note on GPU layout:** Blocks remain physically contiguous in GPU HBM — the
DMA efficiency of the contiguous memory layout (v0.12, PR #27743) is preserved.
What became non-contiguous is the *offload index* (which blocks are resident in
the offload tier at any given moment).

**Implication for Certus:** The certus-server's `Check` RPC naturally returns
per-key existence. The batch→per-key shift aligned the vLLM API with what
external storage systems already provide. No batching efficiency is lost because
vLLM calls `lookup` once per block per scheduling step (the scheduler iterates
over blocks anyway), and Certus's `prepare_store` / `prepare_load` still use
batch RPCs for the actual data transfer.

---

### OffloadKey Replaces BlockHash (v0.20)

**The problem:** `BlockHash` was a flat content hash of the KV data. With
multi-group KV cache layouts (GQA: 8 KV heads vs 32 Q heads), the same token
sequence produces the same content hash across groups — but each group's physical
block occupies a different memory region with a different size. One hash cannot
address them independently.

**The solution:** `OffloadKey = NewType("OffloadKey", bytes)` packs
`(block_hash, group_idx)` into a single opaque key. Helper functions
`make_offload_key(block_hash, group_idx)` and `get_offload_group_idx(key)`
decompose it. Now each group's block is independently addressable: group 0's
block can be evicted while group 1's survives.

---

### Handler Router → Direct Worker (v0.25)

**The old model (v0.11–v0.24):**
```
OffloadingSpec.get_handlers(kv_caches) yields:
    (GPULoadStoreSpec, CPULoadStoreSpec, handler_gpu_to_cpu)
    (CPULoadStoreSpec, GPULoadStoreSpec, handler_cpu_to_gpu)

OffloadingWorker (concrete router):
    register_handler(src_cls, dst_cls, handler)
    transfer_async(job_id, spec=(src, dst))  # routes by medium pair
```

**The problem:** The indirection was unnecessary. Each backend owns exactly one
offloaded medium. The connector already knows whether it's storing (GPU→medium)
or loading (medium→GPU). Routing by `(src_medium, dst_medium)` tuple adds a
dispatch lookup, a registration step, and two separate handler instances that
share most of their state.

**The new model (v0.25+):**
```
OffloadingSpec.get_worker(kv_caches) -> OffloadingWorker

OffloadingWorker (ABC):
    submit_store(job_id, src: GPULoadStoreSpec, dst: LoadStoreSpec)
    submit_load(job_id, src: LoadStoreSpec, dst: GPULoadStoreSpec)
```

Direction is in the method name. One worker instance, no registration, no
routing. `TransferResult` drops `transfer_type` because it's redundant.

**Implication for Certus:** The `handler.py` CertusGrpcWorker implements both
APIs from one class — `transfer_async` (≤0.24) simply delegates to
`submit_store`/`submit_load` based on spec types. Zero code duplication.

---

### LookupResult Enum Replaces bool|None (v0.25)

**The problem:** `bool | None` overloaded three meanings:
- `True` = block is cached and ready
- `False` = block is not cached (miss)
- `None` = retry later (transient unavailability)

But a fourth state existed that couldn't be expressed: the block's store is
in-flight (data being copied to the offload tier right now). Treating it as a
miss forces recomputation; treating it as a hit fails the load (data isn't there
yet); treating it as retry delays the request indefinitely.

**The solution (PR #46363):**
```python
class LookupResult(Enum):
    HIT = auto()          # cached and ready to load
    HIT_PENDING = auto()  # store in-flight; will be ready soon
    MISS = auto()         # not cached
    RETRY = auto()        # transient; try again next scheduling step
```

`HIT_PENDING` lets the scheduler decide: wait for the store to complete (cheap
if nearly done), or proceed with recomputation (better if the store just started).

**Implication for Certus:** The certus-server's `Check` RPC returns a binary
exists/not-exists. The connector maps this to `HIT`/`MISS` — it never returns
`HIT_PENDING` because the server's `CommitStore` is atomic (a key either exists
fully or doesn't). This is correct behavior: unlike in-process CPU offloading
where the DMA might be mid-flight, Certus blocks are either committed or not.

---

### OffloadingConfig Boundary (v0.26)

**The problem:** Backend specs received raw `VllmConfig` + `KVCacheConfig` —
large, deeply-nested objects that expose vLLM's full internal model and cache
representation. Backend authors had to:
1. Navigate `vllm_config.model_config.hf_config.num_hidden_layers` to get layer count
2. Understand `kv_cache_config.kv_cache_groups[0].kv_cache_spec.page_size_bytes`
3. Compute `block_size_factor` from the relationship between GPU/offload block sizes
4. Handle packed vs. non-packed tensor layouts for byte-size derivation

Every internal refactor of `VllmConfig` or `KVCacheConfig` broke external backends.

**The solution (PR #48150):** The connector translates raw configs into a
normalized `OffloadingConfig` before constructing the backend spec:

```python
@dataclass
class OffloadingConfig:
    groups: list[OffloadingGroupConfig]
    model: OffloadingModelConfig
    cache: OffloadingCacheConfig
    parallel: OffloadingParallelConfig
    extra_config: dict[str, Any]
    worker_kv_bytes_per_block: int   # pre-computed!
    enable_kv_cache_events: bool
```

`worker_kv_bytes_per_block` is the key field — the connector does the
packed/non-packed tensor math once, and backends just read a single integer.
No more reconstructing byte sizes from page_size × num_layers × factor.

**Implication for Certus:** On v0.26+, `block_bytes_from_offloading_config`
simply reads `config.worker_kv_bytes_per_block`. The fragile per-group derivation
in `block_bytes_from_config` (used for ≤0.24) is only a fallback for older versions.

---

### Contiguous Memory Layout (v0.12)

**The problem (v0.11):** vLLM allocated GPU memory in per-layer blocks. A single
logical KV block (all layers for N tokens) was fragmented into `num_layers`
separate physical allocations (sometimes further split into K and V). Physical
block sizes were tiny (8–32 KB for typical models).

This fragmentation is irrelevant for model computation (attention reads one
layer at a time) but devastating for offloading. DMA (cudaMemcpyAsync) achieves
peak throughput only with large contiguous transfers. Thousands of 32 KB copies
saturate the DMA command queue and under-utilize the copy engine's bandwidth.

**The solution (PR #27743):** Pack all layers into one physical block:

| Model | Old block size | New block size |
|-------|---------------|----------------|
| Llama-3.1-8B | 32 KB | 2 MB |
| Llama-3.1-70B | 8 KB | 1.25 MB |
| DeepSeek-V2-Lite | 72 KB | 1.9 MB |

Result: ~10× throughput improvement in the offloading connector (from blog benchmarks).

**Implication for Certus:** One IPC handle per block, one DMA per block, one
Reserve slot per block. The large block size (0.5–2 MB) is ideal for NVMe
write/read granularity too — aligns with the server's 128 KB slab size default.

---

### Block Dependency and Eviction

**Do offloaded blocks preserve dependency information?**

No. Offloaded blocks are content-addressed by their `OffloadKey` (hash of token
content + group index). The offload tier has no concept of "block 3 depends on
blocks 0,1,2 existing" or "these 5 blocks form a conversation prefix."

The reason: from the offloading connector's perspective, blocks are independent
units. A block's KV data is self-contained — attention for tokens in block 3 reads
from blocks 0,1,2,3 in GPU memory at inference time, but the *stored* data for
block 3 doesn't reference blocks 0,1,2. You can evict block 1 from the offload
tier without corrupting block 3's stored data.

**What this means for eviction:**

The offload tier uses flat LRU/ARC policies (recency/frequency of `touch` and
`lookup` calls). It does NOT evict-together or keep-together based on prefix
relationships. This can lead to:

- Partial prefix residency (blocks 0,2,4 cached; 1,3 evicted) — loadable blocks
  are loaded, missing blocks are recomputed. Net benefit is proportional to the
  fraction of blocks that survived.
- Orphaned tail blocks — if block 0 (the prefix root) is evicted but blocks 1–4
  survive, those tail blocks are still individually valid and loadable if a request
  arrives that matches their content hash. They aren't "useless without block 0."

**Could dependency-aware eviction help?**

Potentially yes — a policy that evicts full prefixes together (atomic eviction
units) would avoid the "partial prefix" case where you load 3/5 blocks and
recompute 2/5. But the current design bet is:

1. Partial hits still reduce TTFT (loading 3/5 blocks saves 60% of prefill compute)
2. Prefix-aware eviction requires tracking which blocks form a prefix, which
   adds bookkeeping and prevents sharing blocks across requests with overlapping
   (but not identical) prefixes
3. The per-key lookup API (v0.20+) already lets the scheduler handle partial
   residency gracefully — load what's there, recompute what's not

For Certus specifically, the server's `Reserve`/eviction layer is flat (per-key
LRU on the memory tier, per-key write-through to SSD). Adding dependency-aware
eviction would be a server-side policy change — the connector API already
supports it via `OffloadingEvent` notifications (the scheduler knows which blocks
were evicted and can adjust accordingly).

---

## Version-by-Version Changelog

### v0.11.0 — Introduction

The offloading stack appears for the first time.

**Module layout:**
- `vllm.v1.kv_offload.abstract` — `LoadStoreSpec`, `OffloadingManager`,
  `PrepareStoreOutput`, `OffloadingEvent`
- `vllm.v1.kv_offload.spec` — `OffloadingSpec`
- `vllm.v1.kv_offload.mediums` — `GPULoadStoreSpec`, `CPULoadStoreSpec`
- `vllm.v1.kv_offload.worker.worker` — `OffloadingHandler` (ABC),
  `OffloadingWorker` (router), `TransferResult`, `TransferSpec`
- `vllm.v1.kv_offload.factory` — `OffloadingSpecFactory`
- `vllm.v1.kv_offload.lru_manager` — `LRUOffloadingManager`

**Key interfaces:**
```python
class OffloadingSpec(ABC):
    def __init__(self, vllm_config: VllmConfig): ...
    def get_manager(self) -> OffloadingManager: ...
    def get_handlers(self, kv_caches: dict[str, torch.Tensor])
        -> Iterator[tuple[type[LoadStoreSpec], type[LoadStoreSpec], OffloadingHandler]]: ...

class OffloadingManager(ABC):
    def lookup(self, block_hashes: Iterable[BlockHash]) -> int: ...
    def prepare_load(self, block_hashes) -> LoadStoreSpec: ...
    def prepare_store(self, block_hashes) -> PrepareStoreOutput | None: ...

class OffloadingHandler(ABC):
    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool: ...
    def get_finished(self) -> list[TransferResult]: ...

TransferSpec = tuple[LoadStoreSpec, LoadStoreSpec]
TransferResult = tuple[int, bool]  # (job_id, success)
```

**Registration:**
- `OffloadingSpecFactory.register_spec("CPUOffloadingSpec", ...)`
- `KVConnectorFactory.register_connector("OffloadingConnector", ...)`
- External specs loadable via `spec_module_path` in `kv_connector_extra_config`

---

### v0.12.0 — ARC + Connector Constructor Extension

**Changes:**
1. `KVConnectorBase_V1.__init__` gains optional `kv_cache_config: KVCacheConfig | None = None`
2. `OffloadingConnector.__init__` now: `(vllm_config, role, kv_cache_config=None)`
3. `OffloadingSpec.get_handlers` gains `attn_backends` parameter
4. `ARCOffloadingManager` added in new `arc_manager.py`
5. `OffloadingConnector.prefer_cross_layer_blocks: ClassVar[bool] = True`
6. `TransferResult` becomes a dataclass: `job_id, success, transfer_size, transfer_time, transfer_type`

**Spec constructor unchanged** — still `spec_cls(vllm_config)` only.

The connector factory adds backward-compat logic for old 2-arg connector constructors.

---

### v0.13.0 — Minor Cleanup

No changes to the kv_offload module itself. `SharedStorageConnector` removed,
`ExampleConnector` registered instead. `MooncakeConnector` added.

---

### v0.14.0 — Spec Receives KVCacheConfig

**Breaking changes:**
1. `OffloadingSpec.__init__(self, vllm_config, kv_cache_config)` — `kv_cache_config`
   is now a positional parameter
2. `OffloadingSpecFactory.create_spec(config, kv_cache_config)` — passes both
3. `OffloadingHandler.wait(job_ids: set[int])` — **new abstract method**
4. `OffloadingWorker.wait(job_ids)` — delegates to all handlers

**Motivation (PR #27887):** `VllmConfig` is immutable user/model config;
`KVCacheConfig` is runtime-computed worker state. Separating them lets backends
derive block geometry from runtime information.

---

### v0.15.0 — Lookup Returns None

**Change:** `OffloadingManager.lookup` return type: `int` → `int | None`.
Returning `None` means "retry later" — the scheduler delays that request.

---

### v0.16.0–v0.17.0 — Stable

No interface changes. Internal improvements only.

---

### v0.18.0 — Geometry from KVCacheConfig

**Changes:**
1. `kv_cache_config` parameter changes from `KVCacheConfig | None` to
   `KVCacheConfig` (required) in spec/factory
2. `OffloadingSpec` gains geometry attributes:
   - `gpu_block_size` becomes `tuple[int, ...]` (one per KV cache group)
   - New: `hash_block_size`, `block_size_factor`
   - Removed: `offloaded_block_size` (replaced by per-group sizes)
3. New `FilterReusedOffloadingManager` in `reuse_manager.py` — store gating
   via `store_threshold`

**Motivation:** Backends must derive per-block byte sizes from runtime cache
tensors, not from static config. Multi-group layouts (e.g., GQA) require per-group
size tracking.

---

### v0.19.0 — CanonicalKVCaches + CPU Package Restructure

**Breaking changes:**
1. `OffloadingSpec.get_handlers` signature:
   - Before: `get_handlers(kv_caches: dict[str, Tensor], attn_backends: dict[...])`
   - After: `get_handlers(kv_caches: CanonicalKVCaches)`
2. `GPULoadStoreSpec.__init__` gains `group_sizes` and `block_indices` (optional)
3. CPU offload restructured into `cpu/` package:
   - `cpu/spec.py`, `cpu/manager.py`
   - `cpu/policies/abstract.py`, `cpu/policies/lru.py`, `cpu/policies/arc.py`
4. `CanonicalKVCaches` dataclass introduced:
   ```python
   @dataclass
   class CanonicalKVCacheTensor:
       tensor: torch.Tensor
       page_size_bytes: int
   @dataclass
   class CanonicalKVCacheRef:
       tensor_idx: int
       page_size_bytes: int
   @dataclass
   class CanonicalKVCaches:
       tensors: list[CanonicalKVCacheTensor]
       group_data_refs: list[list[CanonicalKVCacheRef]]
   ```

---

### v0.20.0 — OffloadKey + Per-Key Lookup + ReqContext

**Major interface evolution.** This is the biggest API break in the stack's history.

**Changes:**
1. `OffloadKey = NewType("OffloadKey", bytes)` introduced — replaces `BlockHash`
   everywhere. Pack/unpack helpers: `make_offload_key(block_hash, group_idx)`
2. `ReqContext` dataclass: `kv_transfer_params: dict[str, Any] | None = None`
3. `OffloadingManager.lookup` signature:
   - Before: `lookup(block_hashes: Iterable[BlockHash]) -> int | None`
   - After: `lookup(key: OffloadKey, req_context: ReqContext) -> bool | None`
4. All manager methods now take `OffloadKey` instead of `BlockHash`, and most
   gain `req_context` parameter
5. `PrepareStoreOutput` field renames: `block_hashes_to_store` → `keys_to_store`,
   `block_hashes_evicted` → `evicted_keys`
6. `OffloadingEvent` field renames: `block_hashes` → `keys`, `block_size` removed
7. `GPULoadStoreSpec.block_indices` becomes required (was optional)
8. `OffloadingHandler` and `OffloadingWorker` gain `shutdown()` method

---

### v0.21.0 — Stable

Minimal changes. `FilterReusedOffloadingManager` moved to standalone
`reuse_manager.py` from inline.

---

### v0.22.0 — Module Consolidation + Tiering

**Changes:**
1. **Module consolidation**: `abstract.py`, `mediums.py`, `spec.py` merged into
   `vllm.v1.kv_offload.base` (the three old modules are removed)
2. `ReqContext` gains `req_id: str` field
3. `OffloadingManager` gains `reset_cache()` (optional, default no-op)
4. `OffloadingManager.shutdown()` present
5. Tiering subsystem introduced: `vllm.v1.kv_offload.tiering/`
6. `TieringOffloadingSpec` registered in factory

---

### v0.23.0 — Request Lifecycle + OffloadPolicy

**Changes:**
1. `OffloadPolicy` enum: `BLOCK_LEVEL`, `REQUEST_LEVEL`
2. `RequestOffloadingContext` dataclass
3. `OffloadingManager.on_new_request(req_context) -> RequestOffloadingContext` —
   **new abstract method** (backends must implement)
4. `OffloadingManager.on_request_finished(req_context)` — optional
5. `OffloadingManager.on_schedule_end()` — optional
6. `OffloadingSpec.offload_prompt_only: bool` attribute

---

### v0.24.0 — Metrics + Events

**Changes:**
1. `OffloadingManager.has_pending_work() -> bool` — new optional
2. `OffloadingManager.get_stats()` — new optional
3. `OffloadingSpec.build_metric_definitions(extra_config)` classmethod
4. `OffloadingKVEventsConfig` dataclass
5. `OffloadingEventsTracker` class
6. `OffloadingConnector.get_required_kvcache_layout()` returns `"HND"`
7. Metric metadata types: `OffloadingMetricMetadata`, `OffloadingCounterMetadata`,
   `OffloadingGaugeMetadata`, `OffloadingHistogramMetadata`

---

### v0.25.0 — Worker Rewrite (OffloadingHandler → OffloadingWorker)

**Major breaking change** (PR #45053). The direction-agnostic handler router is
replaced by an explicit submit API.

**Before (v0.24):**
```python
# worker/worker.py (DELETED in v0.25)
class OffloadingHandler(ABC):
    def transfer_async(self, job_id, spec: TransferSpec) -> bool: ...
    def get_finished(self) -> list[TransferResult]: ...
    def wait(self, job_ids): ...

class OffloadingWorker:  # concrete router
    def register_handler(self, src_cls, dst_cls, handler): ...
    def transfer_async(self, job_id, spec) -> bool: ...

# spec
class OffloadingSpec(ABC):
    def get_handlers(self, kv_caches) -> Iterator[tuple[src, dst, OffloadingHandler]]: ...
```

**After (v0.25+):**
```python
# base.py
class OffloadingWorker(ABC):  # replaces OffloadingHandler
    def submit_store(self, job_id, src: GPULoadStoreSpec, dst: LoadStoreSpec) -> bool: ...
    def submit_load(self, job_id, src: LoadStoreSpec, dst: GPULoadStoreSpec) -> bool: ...
    def get_finished(self) -> list[TransferResult]: ...
    def wait(self, job_ids): ...
    def shutdown(self): ...

class OffloadingSpec(ABC):
    def get_worker(self, kv_caches: CanonicalKVCaches) -> OffloadingWorker: ...
    # get_handlers() REMOVED
```

**Also in v0.25:**
- `TransferResult` drops `transfer_type` field (direction is now implicit)
- `TransferSpec` type alias removed
- `worker/worker.py` directory deleted
- `TransferResult` and `OffloadingWorker` move into `base.py`
- `LookupResult` enum introduced (PR #46363):
  ```python
  class LookupResult(Enum):
      HIT = auto()
      HIT_PENDING = auto()
      MISS = auto()
      RETRY = auto()
  ```
- `OffloadingManager.lookup` returns `LookupResult` instead of `bool | None`

---

### v0.26.0 — OffloadingConfig Backend Boundary

**Breaking change for spec constructors** (PR #48150).

**Changes:**
1. `OffloadingSpec.__init__` signature: `(self, config: OffloadingConfig)` —
   no longer receives raw vLLM/cache config
2. `OffloadingSpecFactory.create_spec(config: OffloadingConfig)` — takes
   `OffloadingConfig` instead of vllm_config + kv_cache_config
3. New `OffloadingConfig` data model (in `vllm/v1/kv_offload/config.py`):
   ```python
   @dataclass
   class OffloadingConfig:
       groups: list[OffloadingGroupConfig]
       model: OffloadingModelConfig
       cache: OffloadingCacheConfig
       parallel: OffloadingParallelConfig
       extra_config: dict[str, Any]
       engine_id: str
       worker_kv_bytes_per_block: int
       enable_kv_cache_events: bool
   ```
4. `build_offloading_config(vllm_config, kv_cache_config) -> OffloadingConfig`
   in connector config module
5. `LoadStoreSpec` downgraded from ABC to plain class — `medium()` removed
6. `OffloadingEvent` gains `locality: Locality | None = None`
7. New `Locality` enum: `LOCAL`, `REMOTE`

**Motivation (PR #48150):** The connector translates raw VllmConfig/KVCacheConfig
into a normalized boundary before constructing the backend spec. Backends no
longer need to understand vLLM internals.

---

### v0.27.0–v0.27.1 — Tiering + Canonical Layout

**Changes:**
1. `Medium` enum: `CPU = "CPU"`, `STORAGE = "STORAGE"` — replaces string medium
2. `OffloadingEvent.medium` type: `str` → `Medium`
3. `TierMatcher` / `TierFilter` for per-request tier selection
4. `ReqContext` gains `load_tier_filter: TierFilter = TierFilter.ALL` and
   typed `_state` dict
5. `CanonicalKVCacheRef` gains `mapping: CanonicalPageMapping | None`
6. `OffloadingConfig` gains `replicated_layout: bool = False`
7. `OffloadingConnectorWorker.__init__` gains `vllm_config` parameter
8. `CachePolicyFactory` in `cpu/policies/factory.py` — extensible policy selection
9. Experimental API warning removed from `OffloadingSpec.__init__`

---

## Dimension Reference

The following dimensions capture the axes of change in the offloading API. Each
is a distinct concern that evolved independently.

### D1: Spec Constructor Shape

**What it is:** How `OffloadingSpecFactory` constructs a custom backend spec.

**Why it changed:** Initially specs only needed model config. As runtime geometry
(cache tensor layouts, block sizes) became required for correct DMA sizing, the
constructor gained `kv_cache_config`. In v0.26, the raw configs were normalized
into `OffloadingConfig` so backends don't need to understand vLLM internals.

| Phase | Versions | Constructor |
|-------|----------|-------------|
| One-arg model config | v0.11–v0.13 | `spec_cls(vllm_config)` |
| Two-arg with cache config | v0.14–v0.25 | `spec_cls(vllm_config, kv_cache_config)` |
| Normalized config | v0.26+ | `spec_cls(offloading_config: OffloadingConfig)` |

### D2: Worker/Handler API

**What it is:** How the spec provides worker-side transfer execution to the connector.

**Why it changed:** The original multi-handler router (`OffloadingWorker` +
`register_handler` by medium pair) was unnecessary indirection — each backend
owns one medium and the connector already knows the direction. PR #45053 replaced
it with a single `OffloadingWorker` ABC with explicit `submit_store`/`submit_load`.

| Phase | Versions | API |
|-------|----------|-----|
| Handler router | v0.11–v0.24 | `get_handlers(kv_caches) -> Iterator[(src, dst, OffloadingHandler)]` |
| Direct worker | v0.25+ | `get_worker(kv_caches) -> OffloadingWorker` with `submit_store`/`submit_load` |

### D3: Lookup Return Type

**What it is:** What `OffloadingManager.lookup()` returns and how the scheduler
interprets it.

**Why it changed:** Initially lookup returned a prefix-match count for batch
lookups. The shift to per-key lookup needed a boolean hit/miss. `None` was
overloaded for "retry". PR #46363 introduced `LookupResult` to disambiguate
`HIT_PENDING` (data in flight) from `RETRY` (transient unavailability).

| Phase | Versions | Signature |
|-------|----------|-----------|
| Batch prefix count | v0.11–v0.14 | `lookup(block_hashes) -> int` |
| Batch with retry | v0.15–v0.19 | `lookup(block_hashes) -> int \| None` |
| Per-key bool | v0.20–v0.24 | `lookup(key: OffloadKey, req_context) -> bool \| None` |
| Typed enum | v0.25+ | `lookup(key: OffloadKey, req_context) -> LookupResult` |

### D4: Block Addressing

**What it is:** How individual cache blocks are identified in manager calls.

**Why it changed:** `BlockHash` was a flat hash with no group awareness. When
multi-group layouts (GQA, MQA) appeared, the same hash could map to different
physical blocks in different cache groups. `OffloadKey` packs `(block_hash, group_idx)`.

| Phase | Versions | Type |
|-------|----------|------|
| BlockHash (flat) | v0.11–v0.19 | `BlockHash = int` or `bytes` |
| OffloadKey (hash + group) | v0.20+ | `OffloadKey = NewType("OffloadKey", bytes)` |

### D5: Request Context

**What it is:** Per-request metadata passed to manager methods for routing,
session tracking, and policy decisions.

**Why it changed:** Originally manager methods had no per-request context.
`ReqContext` (v0.20) enabled per-request routing. `on_new_request` (v0.23)
enabled request-level offloading decisions. `TierFilter` (v0.27) enables
per-request tier selection.

| Phase | Versions | Context |
|-------|----------|---------|
| None | v0.11–v0.19 | No per-request state |
| ReqContext basic | v0.20–v0.21 | `ReqContext(kv_transfer_params)` |
| ReqContext + req_id | v0.22 | `ReqContext(kv_transfer_params, req_id)` |
| RequestOffloadingContext | v0.23–v0.26 | `on_new_request()` abstract; `OffloadPolicy` enum |
| TierFilter | v0.27+ | `ReqContext.load_tier_filter`, typed `_state` dict |

### D6: KV Cache Geometry Passing

**What it is:** How the worker-side receives KV cache tensor layout information.

**Why it changed:** Originally a raw `dict[str, Tensor]`. As cross-layer blocks
and multi-group layouts appeared, the connector needed structured page-size and
group-reference metadata.

| Phase | Versions | Type |
|-------|----------|------|
| Raw tensor dict | v0.11–v0.18 | `dict[str, torch.Tensor]` (+ `attn_backends` in v0.12–v0.18) |
| CanonicalKVCaches | v0.19+ | Structured `CanonicalKVCaches` dataclass |

### D7: GPULoadStoreSpec Shape

**What it is:** What the GPU-side load/store spec carries for addressing blocks.

**Why it changed:** Originally just block IDs. Multi-group layouts require
knowing how many blocks belong to each group and their indices within the
coalesced tensor.

| Phase | Versions | Constructor |
|-------|----------|-------------|
| Block IDs only | v0.11–v0.18 | `GPULoadStoreSpec(block_ids)` |
| With group sizes (optional indices) | v0.19 | `GPULoadStoreSpec(block_ids, group_sizes, block_indices=None)` |
| With required indices | v0.20+ | `GPULoadStoreSpec(block_ids, group_sizes, block_indices)` |

### D8: Eviction Policy

**What it is:** How the CPU-primary offload tier decides which blocks to evict.

**Why it changed:** LRU was the only option initially. ARC (adaptive) was added
for workloads that mix recency and frequency. Reuse gating prevents low-value
blocks from entering the cache. Factory pattern enables out-of-tree policies.

| Phase | Versions | Policies |
|-------|----------|----------|
| LRU only | v0.11 | `LRUOffloadingManager` |
| LRU + ARC | v0.12–v0.17 | Explicit manager alternatives |
| LRU + ARC + store_threshold | v0.18 | `FilterReusedOffloadingManager` wrapper |
| CachePolicy interface | v0.19–v0.26 | `CachePolicy` ABC, `lru.py`, `arc.py` |
| CachePolicyFactory | v0.27+ | Extensible factory, out-of-tree policies via module path |

### D9: Module Location

**What it is:** Where the core types live in the source tree.

**Why it changed:** The initial split (`abstract.py`, `mediums.py`, `spec.py`)
was consolidated in v0.22 when the file count grew unwieldy. Worker types moved
into `base.py` in v0.25 when the handler router was eliminated.

| Phase | Versions | Layout |
|-------|----------|--------|
| Split modules | v0.11–v0.21 | `abstract.py`, `spec.py`, `mediums.py`, `worker/worker.py` |
| Consolidated base | v0.22–v0.24 | `base.py` (types), `worker/worker.py` (handler) |
| Fully merged | v0.25+ | `base.py` (all types including `OffloadingWorker`, `TransferResult`) |

### D10: TransferResult Shape

**What it is:** What the async transfer completion reports back.

**Why it changed:** Originally a plain tuple. Became a dataclass with optional
timing metadata. `transfer_type` was removed when the handler router was
eliminated (direction is implicit in `submit_store`/`submit_load`).

| Phase | Versions | Fields |
|-------|----------|--------|
| Tuple | v0.11 | `tuple[int, bool]` (job_id, success) |
| Dataclass with transfer_type | v0.12–v0.24 | `job_id, success, transfer_size, transfer_time, transfer_type` |
| Dataclass without transfer_type | v0.25+ | `job_id, success, transfer_size, transfer_time` |

---

## Full Capability Matrix

Each cell shows the state at that version. Blank = same as previous version.

| Version | D1: Spec Ctor | D2: Worker API | D3: Lookup Return | D4: Block Addr | D5: Req Context | D6: Cache Geometry | D7: GPU Spec | D8: Eviction | D9: Modules | D10: TransferResult |
|---------|--------------|----------------|-------------------|---------------|-----------------|-------------------|-------------|-------------|-------------|-------------------|
| **v0.11** | `(vllm_config)` | `get_handlers(dict, -)` → Handler | `int` | BlockHash | None | `dict[str, Tensor]` | `(block_ids)` | LRU | Split | `tuple` |
| **v0.12** | — | `get_handlers(dict, attn)` → Handler | — | — | — | `dict + attn_backends` | — | LRU+ARC | — | dataclass+type |
| **v0.13** | — | — | — | — | — | — | — | — | — | — |
| **v0.14** | `(vllm_config, kv_cache_config)` | — | — | — | — | — | — | — | — | — |
| **v0.15** | — | — | `int \| None` | — | — | — | — | — | — | — |
| **v0.16** | — | — | — | — | — | — | — | — | — | — |
| **v0.17** | — | — | — | — | — | — | — | — | — | — |
| **v0.18** | `(vllm_config, KVCacheConfig)` req'd | — | — | — | — | — | — | +store_threshold | — | — |
| **v0.19** | — | `get_handlers(CanonicalKVCaches)` | — | — | — | CanonicalKVCaches | `(ids, groups, indices?)` | CachePolicy ABC | — | — |
| **v0.20** | — | — | `bool \| None` per-key | OffloadKey | ReqContext | — | `(ids, groups, indices)` req'd | — | — | +shutdown() |
| **v0.21** | — | — | — | — | — | — | — | — | — | — |
| **v0.22** | — | — | — | — | +req_id | — | — | — | Consolidated base | — |
| **v0.23** | — | — | — | — | +on_new_request abstract | — | — | — | — | — |
| **v0.24** | — | — | — | — | +metrics/events | — | — | — | — | — |
| **v0.25** | `(vllm_config, kv_cache_config)` | `get_worker()` → OffloadingWorker | `LookupResult` enum | — | — | — | — | — | Fully merged | -transfer_type |
| **v0.26** | `(OffloadingConfig)` | — | — | — | — | — | — | — | +config.py | — |
| **v0.27** | — | — | — | — | +TierFilter | +CanonicalPageMapping | — | CachePolicyFactory | — | — |

---

## PR Cross-Reference

| PR | Version | Change |
|----|---------|--------|
| #19848 | v0.11 | Generic offloading component |
| #20075 | v0.11 | LRU-based CPU offload management |
| #21448 | v0.11 | Worker-side CPU support |
| #22595 | v0.11 | OffloadingConnector wrapper |
| #24251 | v0.11 | CPUOffloadingSpec registration |
| #27743 | v0.12 | Contiguous memory layout (block size 32KB→2MB) |
| #27039 | v0.12 | ARC eviction policy |
| #27887 | v0.14 | KVCacheConfig as explicit spec argument |
| #35342 | v0.18 | Store gating via store_threshold |
| #37874 | v0.19 | CachePolicy abstraction + cpu/ restructure |
| #45053 | v0.25 | OffloadingHandler → OffloadingWorker rewrite |
| #46363 | v0.25 | LookupResult enum |
| #48150 | v0.26 | OffloadingConfig clean backend boundary |
| #48414 | v0.26 | Canonical CPU layout support |
| #49114 | v0.27 | CachePolicyFactory |
| #50992 | v0.27 | ARC batch eviction fix |

---

## Notes on Unreleased `main` (at `fe889ac925`)

Beyond v0.27.1, `main` adds:
- `canonical_layout: bool` flag on `OffloadingConfig`
- `prefer_cross_layer_blocks` changes based on canonical layout flag
- These are not yet in any release tag
