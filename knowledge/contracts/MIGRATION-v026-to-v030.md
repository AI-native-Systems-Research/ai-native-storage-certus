# vLLM KV-Offloading Interface: v0.26/0.28 → v0.30 Migration

Structured diff of the `vllm.v1.kv_offload.*` plugin surface between the
`v026` contract (verified against **v0.28.1rc0-197-gdc9114b201**) and the `v030`
contract (verified against **v0.30.1rc0-171-g2bc902eb0f**, commit `2bc902eb0f`).

Every symbol below was read from real v0.30 source under
`/home/b/vllm-0.30/vllm/vllm/v1/kv_offload/`. Line references are for that tree.

> **Scope note.** v0.26 already introduced the "consolidated `base`" rewrite
> (single `OffloadingWorker`, `OffloadingConfig` ctor, `LookupResult` enum,
> `CanonicalKVCaches`). The `v026` contract file documents that era. This
> migration therefore focuses on what is **genuinely new/changed at 0.30** on top
> of the 0.26 rewrite. Because only 0.30 source (not 0.27/0.28/0.29) was available,
> deltas confirmed only in 0.30 source are conservatively attributed to 0.30.

---

## 1. Top-level `OffloadingSpec` / `OffloadingManager` / `OffloadingWorker` ABCs

**No breaking change to the three core ABC method signatures.** A connector that
satisfied the 0.26 rewrite (single-config ctor, `get_worker(kv_caches)`,
`submit_store/submit_load/get_finished/wait`, `lookup → LookupResult`,
`on_new_request → RequestOffloadingContext`) still satisfies 0.30. The certus
connector's `OffloadingSpec`/`OffloadingManager`/`OffloadingWorker` subclasses are
signature-compatible with 0.30 as-is.

The deltas below are **additive/optional** on the paths the connector touches, and
**material** on the tiering path (which the connector does not implement).

---

## 2. Changed / renamed types (`vllm/v1/kv_offload/base.py`)

| Symbol | v026 (≤0.28) | v030 | Kind |
|---|---|---|---|
| `Medium` enum | `CPU`, `STORAGE`, **`REMOTE`** | `CPU`, `STORAGE` only | **REMOVED value** |
| `TierFilter` | enum `ALL / LOCAL / REMOTE` | `@dataclass(frozen=True)` with `matchers: tuple[TierMatcher, ...]`, ClassVar `ALL`, `allows()` | **type reshape** |
| `TierMatcher` | — | new `NamedTuple(medium, locality)` + `matches()` | **added** |
| `CanonicalKVCacheRef.page_mapping` | `page_mapping: CanonicalPageMapping \| None` | **renamed** `mapping: CanonicalPageMapping \| None = None` | **renamed field** |
| `BlockIDsLoadStoreSpec.block_ids` | `np.ndarray` (doc said int64) | `np.array(block_ids, dtype=np.int32)` (base.py:419) | **dtype narrowed** |

---

## 3. Added types (`base.py`)

- **`BlockIDsLoadStoreSpec(LoadStoreSpec, ABC)`** — new shared base for
  block-id-carrying specs; `GPULoadStoreSpec` now extends it (base.py:411/425).
- **`CanonicalPageMapping`** (base.py:488) — `canonical_page_size_bytes`,
  `local_page_size_bytes`, `runs: tuple[CopyRun, ...]`, `num_writers`,
  `writer_index`, `parallelism_agnostic`, `is_writer()` (RFC #42082 canonical layouts).
- **`CopyRun`** — element of `CanonicalPageMapping.runs`.
- **`OffloadKey` helpers**: `make_offload_key(block_hash, group_idx)`,
  `get_offload_block_hash(key)`, `get_offload_group_idx(key)` (base.py:28/33/38).
- **`OffloadingKVEventsConfig`** (`enable_kv_cache_events`, `self_describing_kv_events`)
  — now built by `OffloadingSpec.__init__` and stored as `self.kv_events_config`.
- Metric-metadata types: `OffloadingMetricMetadata`, `OffloadingCounterMetadata`,
  `OffloadingGaugeMetadata`, `OffloadingHistogramMetadata(buckets)`.

---

## 4. Changed fields on existing dataclasses (`base.py`)

### `ReqContext`
- **Added** `kv_hints: KvHintsEnvelope | None = None` (new import
  `from vllm.v1.kv_hints import KvHintsEnvelope`, base.py:19/93).
- **Added** `_offload_key_positions: dict[OffloadKey, int]` (init=False) plus
  `set_offload_key_position` / `get_offload_key_position` methods.
- `load_tier_filter` default is still `TierFilter.ALL` — but `TierFilter` is now a
  dataclass (see §2), so the *value* shape changed even though the field name did not.

### `OffloadingEvent`
- `keys`, `medium`, `removed` unchanged (all present since the 0.26 rewrite).
- **Added** `locality: Locality | None = None`, `ownership: str | None = None`
  (secondary-tier id; `None` = primary), `removal_expected: bool = False`.
- All new fields have defaults → keyword construction (as the connector does) is
  unaffected.

### `RequestOffloadingContext`
- `policy: OffloadPolicy = OffloadPolicy.CHUNK_LEVEL`. **Correction vs stale docs:**
  the default policy is `CHUNK_LEVEL` (`"chunk_level"`), NOT "BLOCK_LEVEL".
  `OffloadPolicy` values are `CHUNK_LEVEL` / `REQUEST_LEVEL` only.

### `TransferResult`
- Unchanged from the 0.26 rewrite: `job_id`, `success`, `transfer_size=None`,
  `transfer_time=None`. Still no `transfer_type` field.

---

## 5. `OffloadingConfig` (`vllm/v1/kv_offload/config.py`) — added fields

| Field | v026 | v030 | Notes |
|---|---|---|---|
| `canonical_layout: bool = False` | — | added | Set from `extra_config["canonical_layout"]` (offloading/config.py:178). |
| `kv_cache_layout: str \| None = None` | — | added | Resolved `KVCacheLayout` name, from `vllm_config.cache_config.kv_cache_layout` (offloading/config.py:287). |

`OffloadingParallelConfig` exposes the full parallel topology
(`rank`, `world_size`, `tp_size`, `pp_size`, `pcp_size`, `dcp_size`,
`data_parallel_*`, `is_parallelism_agnostic`). **No new *required* capability** is
imposed on a connector — there is **no DCP/context-parallel declaration a spec must
emit**; canonical-layout certification happens internally in the offloading
connector's config builder, not via a spec-side flag (confirmed: no such
requirement in `offloading_connector.py` / `offloading/config.py`).

---

## 6. New enum: `KVCacheLayout` (`vllm/v1/kv_cache_layout.py`)

New at 0.30. Members `LBHNC`, `LBNHC`, `LHBNC`, `BLHNC`, `BLNHC`, `BHLNC` (stride
permutations of `L, B, H, N, C`) with properties `stride_order`,
`layer_view_order`, `is_layer_compact`, `is_block_contiguous`, `is_block_compact`,
`is_block_outermost`. Its resolved *name* surfaces on
`OffloadingConfig.kv_cache_layout`.

---

## 7. Tiering surface (`vllm/v1/kv_offload/tiering/*`) — MATERIAL changes

The certus connector is a **top-level `OffloadingSpec`, not a
`SecondaryTierManager`**, so these do not change the connector — but the tiering
surface changed materially, which is why a `v030` tiering sibling contract was
created.

### `SecondaryTierManager` (`tiering/base.py`)
- **Ctor CHANGED**: new 4th param
  `backpressure_detector: BackpressureDetector | None = None` (base.py:145-150).
- **Added** `medium: ClassVar[Medium | None] = None` (base.py:143).
- **Added** property `bp_detector -> BackpressureDetector | None` (base.py:175).
- **`serve_external_requests` CHANGED**: now `(parent: ParentManager) -> None`
  (base.py:312) — v026 took no `parent` argument.

### New `ParentManager(ABC)` (`tiering/base.py:93`)
Callback interface a secondary tier uses: `on_new_request`, `lookup`,
`create_store_job(keys, req_context) -> TransferJob`, `on_request_finished`.

### `JobResult` (`tiering/base.py:79`)
- **Added** `successful_keys: Collection[OffloadKey] | None = None` — partial-failure
  subset (`None` = all keys share `success`).

### New backpressure subsystem (`tiering/backpressure.py`)
- Policies: `BackpressurePolicy`, `DropAccountingPolicy`, `DropStorePolicy`,
  `ThrottledDropPolicy`.
- Detectors: `BackpressureDetector (ABC)`, `EMABackpressureDetector`
  (LOCAL vs NETWORK watermarks; obj tier classified NETWORK).
- Wired by `SecondaryTierFactory.create_secondary_tier()`, which pops a
  `"backpressure"` config block and injects the detector into the tier ctor.

### New built-in tier: `kvcr` (`tiering/factory.py:170`)
`SecondaryTierFactory` now registers **`kvcr` → KVCRSecondaryTierManager** alongside
`example` / `fs` / `p2p` / `obj`.

### `TieringOffloadingMetrics` (`tiering/base.py:39`)
New class of metric-name constants (includes backpressure metrics).

---

## 8. Capability flags derived for the connector's `compat.py`

Following the connector convention — *add a version-gated flag only where an
adapter must branch* — the diff yields exactly **one** connector-relevant new flag:

| Flag | Predicate | Why |
|---|---|---|
| `kvcache_layout_field` | `v >= (0, 30)` | `OffloadingConfig` gained `kv_cache_layout` (resolved `KVCacheLayout` name) + `canonical_layout`. Adapter `kv_cache_layout_name(config)` reads it (returns `None` on older versions). |

**Candidate flags evaluated and REJECTED** (per "derive the real ones, don't blindly add"):

- `secondary_tier_manager` — the tiering deltas (backpressure ctor param,
  `serve_external_requests(parent)`, `ParentManager`, `kvcr`, partial-failure
  `JobResult`) are real, but the certus connector is a **top-level spec, not a
  secondary tier**, and never subclasses `SecondaryTierManager`. No adapter branch
  exists to gate → documented in the tiering contract, **not** added to `compat.py`.
- `kvcache_layout_enum` — folded into `kvcache_layout_field` (the connector reads the
  resolved *name* off the config; it does not need the enum object at runtime).
- `out_of_tree_module_path` — `spec_module_path` loading has been stable since v026;
  **not a 0.30 delta**, so not gated.
- `dcp_declaration_required` — **no evidence.** There is no DCP/context-parallel
  declaration a spec must emit; canonical certification is internal to vLLM's config
  builder. Rejected.

Everything else that changed at 0.30 on the connector's path is additive/optional and
handled by the existing `>= (0, 26)` flags (`worker_split_submit`,
`spec_config_object`, `lookup_returns_enum`, `canonical_kv_caches`), which evaluate
`True` for 0.29/0.30 automatically once those rows are added to `SUPPORTED_VERSIONS`.

- `OffloadingEvent` new fields → have defaults; connector constructs by keyword. No branch.
- `CanonicalKVCacheRef.page_mapping → mapping` rename → connector reads
  `tensors[i].tensor` + `len(group_data_refs)`, never the ref's mapping field. No branch.
- `block_ids` int64→int32 → `gpu_block_ids()` already coerces via `int(b)`. No branch.
- `ReqContext.kv_hints` → connector treats `req_context` opaquely; never constructs one. No branch.
- `Medium` losing `REMOTE` → connector uses `CertusLoadStoreSpec.medium()` (CPU/STORAGE). No branch.
