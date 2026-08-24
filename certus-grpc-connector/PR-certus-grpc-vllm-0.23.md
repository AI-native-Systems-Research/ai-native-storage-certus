# feat(certus-grpc): vLLM 0.26 support + vLLM 0.23; guard the per-layer KV split

**Branch:** `feat/certus-grpc-vllm-0.23` → **base:** `unstable`

Two commits on top of `unstable` (which already carries the 0.20–0.24
capability-matrix shim from #370):

1. `feat(certus-grpc): support vLLM 0.26 offloading-API rewrite via capability shim`
2. `feat(certus-grpc): support vLLM 0.23; guard the per-layer KV split`

Both extend the same capability-matrix shim (`certus_grpc_connector.compat`): the
connector self-adapts at run time (version detection → named capabilities) and
branches on named caps, never on raw version compares. The certus-server side is
untouched.

## 1. vLLM 0.26 offloading-API rewrite

vLLM 0.26 rewrote the `vllm.v1.kv_offload.*` plugin surface (marked experimental
in-source). Absorbed with four `v >= (0, 26)` capabilities and lazy adapters, so
one package still serves 0.20 → 0.26:

| Concern | ≤0.24 | 0.26 |
|---|---|---|
| Worker base | `OffloadingHandler` (`worker.worker`); `transfer_async(job_id, spec)` | `OffloadingWorker` (`base`); `submit_store` / `submit_load` |
| Spec → worker | `get_handlers(kv_caches)` → tuples | `get_worker(kv_caches)` → single worker |
| Spec ctor | `(vllm_config, kv_cache_config)` | `(config: OffloadingConfig)` |
| `lookup` return | `bool \| None` | `LookupResult` enum |
| KV caches | tensor + stride | `CanonicalKVCaches` |
| `TransferResult` | 5 fields (`transfer_type`) | 4 fields (dropped) |

New caps: `worker_split_submit`, `spec_config_object`, `lookup_returns_enum`,
`canonical_kv_caches` (all `≥0.26`); `transfer_result_has_type` flipped to
`v < (0, 26)`. One `CertusGrpcWorker` implements both worker interfaces; the spec
defines both `get_worker` and `get_handlers`. Era-absent symbols
(`OffloadingWorker`, `LookupResult`, `OffloadingConfig`) are resolved by lazy
adapters, out of the mandatory import ladder.

## 2. vLLM 0.23 support + two off-by-one corrections

Adds vLLM **0.23** as a supported version and corrects two matrix thresholds that
are **currently wrong in `unstable`** — both found empirically by building the
0.23/0.24 base images and inspecting the real KV-cache handoff at engine init.

### `on_new_request` became abstract at **0.23**, not 0.24

`OffloadingManager.on_new_request(req_context) -> RequestOffloadingContext` is an
`@abstractmethod` as of vLLM **0.23** (`vllm/v1/kv_offload/base.py:229`); without
it the manager can't be instantiated. `unstable` gates this at `v >= (0, 24)`.

Fixed: `has_on_new_request` → `v >= (0, 23)`. The connector already defines the
method unconditionally (returns the default `RequestOffloadingContext()` =
`BLOCK_LEVEL`, matching its `prepare_store` Check filter), so the fix is
declarative — but the matrix is documentation-as-code and must be accurate.

### The per-layer KV split entered at **0.23** — and it was silent

`extract_gpu_ptrs` read `kv_caches.tensors[0]` with **no length check**. Measured:

| vLLM | `len(tensors)` | layout | per-block stride |
|------|---------------:|--------|-----------------:|
| 0.20 | 1 | one coalesced tensor (all layers) | 2 097 152 B (full block) |
| 0.22 | 1 | one coalesced tensor (all layers) | 2 097 152 B (full block) |
| **0.23** | **32** | per-layer tensors (Llama-3: `(num_blocks, 65536)` int8) | 65 536 B (**layer 0 only**) |
| **0.24** | **32** | per-layer tensors | 65 536 B (**layer 0 only**) |
| 0.26 | 32 | `CanonicalKVCaches`, per-layer | 65 536 B |

On 0.23/0.24 the old code silently offloaded **layer 0 only** (1 of 32 layers) —
the recorded 0.24 benchmark shows exactly this (`stride=65536` + the
config-derived-Reserve mismatch WARNING). This is live in `unstable` for 0.24.

Fixed: the `≤0.24` branch now refuses `len(tensors) != 1` with a
`NotImplementedError` **before any store**, mirroring the 0.26 `CanonicalKVCaches`
branch. On 0.20/0.22 (`len == 1`) it passes; on 0.23/0.24 (`len == 32`) it aborts
cleanly — tier stays empty, nothing corrupted, the failure names the cause.
Completing a 0.23+ run needs a block represented as N per-layer regions (per-layer
IPC handles, or a connector-side staging-buffer gather) — a separate change, the
same wall 0.26 hits.

## Tests

`cd certus-grpc-connector && python -m pytest tests/ -q` — **64 passed**,
parametrized across 0.20 / 0.22 / 0.23 / 0.24 / 0.26 (fake-vllm factory in
`conftest.py`; `test_compat.py` pins every threshold). Two focused
`extract_gpu_ptrs` tests: a single coalesced tensor returns `(ptr, stride)`; a
32-way per-layer split raises `NotImplementedError`.

`python -m certus_grpc_connector.compat` prints the full matrix, with
`has_on_new_request` = yes from 0.23:

```
feature                                0.20    0.22    0.23    0.24    0.26
has_on_new_request                     .       .       yes     yes     yes
```

## Out of scope

- Multi-region-per-key server contract (blocks a real 0.23/0.24/0.26 run; the
  server is version-independent).
- Slides / benchmark artifacts (not committed).
