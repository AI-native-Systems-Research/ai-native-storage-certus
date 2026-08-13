# Certus gRPC Connector — vLLM Compatibility Assessment

**Assessment date:** 2026-08-12
**Connector source:** `certus-grpc-connector/certus_grpc_connector/`
**vLLM versions checked:** upstream release tags v0.11.0 through v0.27.1

---

## What We Support Today

The connector supports vLLM **0.20, 0.22, 0.23, 0.24, 0.26**.

All five versions pass the unit test suite (64 tests) and the code paths are
verified against upstream release tags.

| Version | Spec Constructor | Worker API | Lookup Return | Status |
|---------|-----------------|-----------|---------------|--------|
| **v0.20** | 2-arg `(vllm_config, kv_cache_config)` | `get_handlers` → OffloadingHandler | `bool \| None` | **Working** |
| **v0.22** | 2-arg | `get_handlers` → OffloadingHandler | `bool \| None` | **Working** |
| **v0.23** | 2-arg | `get_handlers` → OffloadingHandler | `bool \| None` | **Working** |
| **v0.24** | 2-arg | `get_handlers` → OffloadingHandler | `bool \| None` | **Working** |
| **v0.26** | 1-arg `(OffloadingConfig)` | `get_worker` → OffloadingWorker | `LookupResult` enum | **Working** |

### How The Code Handles Each Version

**v0.20–v0.24 (get_handlers era):**
- `spec.py:80` — `super().__init__(vllm_config, kv_cache_config)`
- `spec.py:183` — `get_handlers()` yields the same worker for both medium pairs
- `handler.py:216` — `transfer_async(job_id, spec)` routes by `isinstance` check
- `manager.py:107` — `lookup()` returns `bool` via `lookup_result(exists)`
- `compat.py:438` — `extract_gpu_ptrs` reads `.tensors[i]` (single tensor on 0.20/0.22, per-layer on 0.23+)
- `manager.py:91` — `on_new_request()` returns `RequestOffloadingContext()` (required from 0.23)

**v0.26 (OffloadingConfig era):**
- `spec.py:74` — `super().__init__(config)` with single `OffloadingConfig`
- `spec.py:78` — `block_bytes_from_offloading_config(config)` reads `worker_kv_bytes_per_block`
- `spec.py:174` — `get_worker()` returns `CertusGrpcWorker` (subclasses `OffloadingWorker`)
- `handler.py:200-212` — `submit_store` / `submit_load` (explicit direction)
- `manager.py:111-116` — `lookup()` returns `LookupResult.HIT`/`.MISS`
- `compat.py:419` — `extract_gpu_ptrs` uses `CanonicalKVCaches.tensors` path

---

## What We Need to Support Next

### v0.27 — Required Changes

v0.27 is released and introduces breaking changes for the connector:

| Change | Impact | Fix |
|--------|--------|-----|
| `OffloadingEvent.medium` changed from `str` to `Medium` enum | `manager.py:286-290` yields events with `medium=CertusLoadStoreSpec.medium()` (returns `"Certus"` string). vLLM will compare this against `Medium` enum values. | Return `Medium` value or use `.value` comparison. Add capability flag `has_medium_enum` at ≥0.27. |
| `TierFilter` / `TierMatcher` on `ReqContext` | `ReqContext.load_tier_filter` controls which tiers a load can hit. Connector ignores it today. | Likely no change needed — Certus is a single-tier backend from vLLM's perspective. The default `TierFilter.ALL` passes through. Verify. |
| `OffloadingConnectorWorker.__init__` gains `vllm_config` | vLLM passes an extra arg when constructing the connector worker. | Connector doesn't override this (vLLM builds the `OffloadingConnectorWorker` itself). No fix needed unless we override it. |
| `OffloadingConfig` gains `replicated_layout: bool` | New field on config. | Connector doesn't read this field. No fix needed unless we want to optimize for replicated layouts. |
| `CachePolicyFactory` | Extensible eviction policy selection. | Irrelevant — Certus doesn't use vLLM's CPU eviction policies. |

**Estimated effort:** Low. The only mandatory fix is the `Medium` enum handling in `take_events`.

### v0.28+ / `main` — Watch Items

- `canonical_layout` flag on `OffloadingConfig` — changes `prefer_cross_layer_blocks` behavior
- Potential further `OffloadingEvent` field changes

---

## Known Limitations in Current Support

### Reserve Size Formula (v0.20–v0.24)

The ≤0.24 path computes per-block bytes as:
```
page_size_bytes × num_layers × block_size_factor
```

This is correct for standard non-packed layouts (Llama, Qwen, Mistral). For
packed KV layouts (DeepSeek MLA), it may under-reserve.

**Mitigation:** `spec.py:149-157` cross-checks the config-derived size against
the actual tensor stride at `get_worker`/`get_handlers` time and logs a warning
if they differ. This catches the mismatch at runtime.

**On v0.26:** Not an issue — `config.worker_kv_bytes_per_block` is pre-computed
correctly by vLLM regardless of layout.

### No v0.25 Support

v0.25 was a transitional release (~6 weeks) that combined the new worker API
with the old spec constructor. The connector skips it. Running against v0.25
would crash (import of `OffloadingHandler` from deleted `worker/worker.py`).

### Test Coverage Gap

Unit tests run against **fake vLLM modules** shaped by the same assumptions as
`compat.py`. This is circular: if a capability flag is wrong, the fake replicates
the wrong shape, and tests still pass.

Full confidence requires running against a real vLLM installation with torch and
GPU. The Dockerfile supports this via `VLLM_VERSION` build arg, but it requires
GPU hardware.

---

## Connector Architecture Summary

```
vLLM OffloadingConnector
    │
    ├── scheduler role: CertusGrpcOffloadingSpec → get_manager()
    │       │
    │       └── GrpcCertusOffloadingManager
    │               lookup()      → Check RPC
    │               prepare_store → Check + Reserve RPCs
    │               complete_store→ CommitStore / AbortStore RPC
    │               prepare_load  → Pin(promote=false) RPC
    │               complete_load → Unpin RPC
    │               take_events   → TakeEvents RPC
    │
    └── worker role: CertusGrpcOffloadingSpec → get_worker() / get_handlers()
            │
            └── CertusGrpcWorker
                    submit_store / transfer_async → CopyToStore RPC
                    submit_load  / transfer_async → Lookup RPC
                    (IPC handles with per-block offsets)
```

Key design choices:
- One class serves both API eras (dual interface: `submit_store`/`submit_load` + `transfer_async`)
- Base class resolved lazily via `worker_base_class()` — never imports a symbol that doesn't exist on the running version
- Process-level singleton gRPC channel shared across manager + worker
- ThreadPoolExecutor (4 threads) for async RPC submission
- Multi-region IPC handles for per-layer tensor splits (v0.23+)
