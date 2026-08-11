# SPDX-License-Identifier: Apache-2.0
"""vLLM version-compatibility shim — the single source of truth for the small,
version-varying surface the connector plugs into.

The connector talks to a separately-run certus-server over a stable protobuf
contract; the ONLY code that changes across vLLM versions is a thin adapter over
vLLM's evolving ``vllm.v1.kv_offload.*`` plugin API. That adapter lives here so
the rest of the package (``client.py``, ``gpu.py``, ``mediums.py``, the pb2
stubs, the RPC mapping in ``manager.py``) stays version-blind and single-source.

Three techniques, each handling a different kind of change:

1. **Version detection** (``VERSION``) + a **capability matrix** (``FEATURES`` ->
   frozen ``CAPS``). Code branches on named capabilities, never on raw version
   numbers scattered around. ``python -m certus_grpc_connector.compat`` prints the
   feature x version matrix so the mapping is inspectable / CI-assertable.
2. **Import ladder** — the ``vllm.v1.kv_offload.*`` imports are wrapped so a
   module *rename* in a future vLLM can be absorbed by adding an alternate path
   (a capability flag can't rescue an import whose path moved).
3. **Adapter functions** (``make_transfer_result``, ``extract_gpu_ptrs``,
   ``block_bytes_from_config``, ``gpu_block_ids``) the connector calls instead of
   touching vLLM's shapes directly. Each version branch lives in exactly one
   place.

Supported (built + smoke-tested) versions: 0.20, 0.22, 0.23, 0.24, 0.26. Values in
the matrix are seeded from the 0.20 baseline; each is confirmed or corrected as the
version walk builds and smoke-tests that release. Entries still awaiting empirical
confirmation on a given version are marked ``# TODO(verify @0.xx)``.
"""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, version as _dist_version

# Versions we support as first-class, runnable rows.
SUPPORTED_VERSIONS: tuple[tuple[int, int], ...] = (
    (0, 20),
    (0, 22),
    (0, 23),
    (0, 24),
    (0, 26),
)


# ── version detection ──────────────────────────────────────────────────────


def _parse(v: str) -> tuple[int, int] | None:
    """Parse a version string to a (major, minor) tuple, or None."""
    try:
        parts = v.strip().lstrip("v").split(".")
        return (int(parts[0]), int(parts[1]))
    except (ValueError, IndexError):
        return None


def _detect_version() -> tuple[int, int]:
    """Resolve the running vLLM (major, minor).

    Order: explicit override env (used by unit tests to exercise each matrix
    row without real vLLM) -> installed distribution metadata -> the imported
    module's ``__version__`` (fakes set this) -> newest supported as a last
    resort so a detection miss defaults to the current target rather than the
    oldest behavior.
    """
    env = os.environ.get("CERTUS_VLLM_VERSION")
    if env and (parsed := _parse(env)):
        return parsed
    try:
        if parsed := _parse(_dist_version("vllm")):
            return parsed
    except PackageNotFoundError:
        pass
    mod = sys.modules.get("vllm")
    if mod is not None and (parsed := _parse(getattr(mod, "__version__", ""))):
        return parsed
    return SUPPORTED_VERSIONS[-1]


VERSION: tuple[int, int] = _detect_version()


# ── capability matrix ──────────────────────────────────────────────────────
#
# name -> predicate(version_tuple) -> bool. This dict IS the visible
# feature->version mapping. Add a row when a version introduces a difference the
# adapters must branch on; keep the predicate expressed as a version threshold.

FEATURES: dict[str, "callable"] = {
    # vLLM's OffloadingConnector scheduler passes a trailing ``req_context`` to
    # every ``OffloadingManager`` method (touch/lookup/prepare_*/complete_*).
    # ADDED in 0.22 — 0.20's scheduler called these without it. The manager
    # absorbs the difference with a superset signature (``req_context=None``
    # default), matching vLLM 0.22's exact arg order, so ONE signature serves
    # both eras. This flag records where the arg actually starts being passed;
    # it is declarative (the superset signature needs no runtime branch).
    "req_context_arg": lambda v: v >= (0, 22),  # verified @0.20 (absent), @0.22 (present)
    # ``TransferResult`` carries the 5th ``transfer_type`` field. Present on
    # 0.20/0.22/0.24; 0.26 DROPPED it (worker direction is explicit via
    # submit_store/submit_load, so the medium pair is no longer carried on the
    # result). ``make_transfer_result`` builds the 4-field shape when this is off.
    "transfer_result_has_type": lambda v: v < (0, 26),  # verified @0.20,0.22,0.24 (present), @0.26 (dropped)
    # ``get_handlers``/``get_worker`` receives an object exposing ``.tensors[i]``
    # (each ``.tensor`` + ``.page_size_bytes``). The ATTRIBUTE is present across the
    # whole range — but its length is a runtime KV-allocation property that changed:
    # measured 0.20/0.22 coalesce all layers into ONE tensor (stride = full block),
    # while 0.23+ split it per-layer (Llama-3: 32 tensors). ``extract_gpu_ptrs``
    # maps EACH tensor to its own IPC region (multi-region offload), so both the
    # single-tensor (N==1) and per-layer-split (N==32) layouts are handled — this
    # flag only gates the access pattern, not the layout. Verified @0.20,0.22
    # (len==1), @0.23,0.24,0.26 (len==32).
    "kv_caches_tensors_attr": lambda v: v >= (0, 20),
    # ``KVCacheConfig.kv_cache_groups[i]`` exposes ``.layer_names`` and
    # ``.kv_cache_spec.page_size_bytes``. True on 0.20; re-confirm per hop.
    "kv_cache_group_attrs": lambda v: v >= (0, 20),  # TODO(verify @0.22/0.24/0.26)
    # v0.26's OffloadingConnector requires the hybrid KV-cache manager to be
    # disabled on the engine (see run driver).
    "needs_disable_hybrid_kv_cache_manager": lambda v: v >= (0, 26),
    # 0.22+ auto-enables async scheduling by default for generation models, with
    # NO check for a kv_transfer/offloading connector (vllm/config/vllm.py). But
    # the OffloadingConnector's scheduler asserts strict per-request transfer
    # serialization (`assert not req_status.transfer_jobs` — "a load can only be
    # issued when no other jobs are pending"). Under async scheduling a request
    # is re-scheduled for a load while its store is still in flight, tripping
    # that assert and killing the engine (EngineDeadError). The run driver must
    # pass ``async_scheduling=False`` to opt out. 0.20 did not default it on, so
    # the flag (and the kwarg itself) only applies from 0.22.
    "needs_disable_async_scheduling": lambda v: v >= (0, 22),  # verified @0.22 (assert crash w/o it)
    # 0.23 added a new ABSTRACT method OffloadingManager.on_new_request(req_context)
    # -> RequestOffloadingContext(policy=...), called once when the scheduler first
    # sees a request. Without an implementation the manager is abstract and vLLM
    # can't instantiate it (TypeError at engine init). We return the default
    # BLOCK_LEVEL context (offload newly-computed blocks, skip prefix hits — which
    # matches our prepare_store Check filter). The return type first appears in the
    # consolidated ``base`` module at 0.23, so the manager builds it lazily via
    # ``new_request_offloading_context``; older bases neither declare nor call the
    # method. Declarative flag (the method is defined unconditionally; it is simply
    # never invoked before 0.23). Measured @0.23 (abstract in base.py) — the
    # abstract method landed at 0.23, NOT 0.24, so the threshold is (0, 23).
    "has_on_new_request": lambda v: v >= (0, 23),  # verified @0.23 (abstract; instantiation fails without it)
    # ── 0.26 offloading-API rewrite (four related breaks, all ≥0.26) ──
    # vLLM 0.26 rewrote vllm.v1.kv_offload.* ("experimental, subject to change").
    # The worker interface changed from a per-direction OffloadingHandler with
    # ``transfer_async(job_id, spec)`` (dispatched by (src_medium, dst_medium)
    # tuples from ``get_handlers``) to a SINGLE ``OffloadingWorker`` with explicit
    # ``submit_store(job_id, src, dst)`` / ``submit_load(job_id, src, dst)``,
    # returned by ``get_worker``. Verified @0.26 (ImportError: worker.worker gone).
    "worker_split_submit": lambda v: v >= (0, 26),
    # Reserved for a future/experimental shape where ``OffloadingSpec.__init__``
    # takes a single OffloadingConfig object. The local 0.26 tree still constructs
    # specs as ``spec_cls(vllm_config, kv_cache_config)`` and its base still exposes
    # ``gpu_block_size`` / ``block_size_factor``, so this stays false through 0.26.
    "spec_config_object": lambda v: False,
    # ``OffloadingManager.lookup`` returns a ``LookupResult`` enum
    # (MISS/HIT/HIT_PENDING/RETRY) rather than ``bool | None``.
    "lookup_returns_enum": lambda v: v >= (0, 26),
    # ``get_worker`` receives ``CanonicalKVCaches`` (a list of per-block
    # ``(num_blocks, page_size_bytes)`` int8 tensors + per-group refs) rather than
    # the raw kv_caches object ``get_handlers`` used to get.
    "canonical_kv_caches": lambda v: v >= (0, 26),
}


@dataclass(frozen=True)
class Caps:
    req_context_arg: bool
    transfer_result_has_type: bool
    kv_caches_tensors_attr: bool
    kv_cache_group_attrs: bool
    needs_disable_hybrid_kv_cache_manager: bool
    needs_disable_async_scheduling: bool
    has_on_new_request: bool
    worker_split_submit: bool
    spec_config_object: bool
    lookup_returns_enum: bool
    canonical_kv_caches: bool


def caps_for(v: tuple[int, int]) -> Caps:
    """Resolve the capability set for a given (major, minor) vLLM version."""
    return Caps(**{name: bool(pred(v)) for name, pred in FEATURES.items()})


CAPS: Caps = caps_for(VERSION)


# ── import ladder (lazy) ─────────────────────────────────────────────────────
#
# The vLLM symbols the connector needs, imported on first access and re-exported.
# Each block is a try/except ladder: today only the 0.20-era path is known, so
# the except arm raises a clear error naming the running version. When a future
# vLLM relocates a module, add its path as an earlier ``try`` arm — nothing else
# in the package changes.
#
# The import is LAZY (deferred until a symbol is first accessed) rather than run
# at module load, so the pure matrix machinery above (VERSION / FEATURES / CAPS /
# render_matrix) is usable without a functioning vLLM — e.g.
# ``python -m certus_grpc_connector.compat`` prints the matrix on a plain laptop,
# and CI can assert it. The loud import-failure behavior is preserved: it fires
# on the first real symbol use (``from .compat import OffloadingSpec`` etc.),
# which is exactly when a moved API path must be surfaced.

_VLLM_SYMBOLS: dict[str, object] = {}


def _import_error(what: str, exc: Exception) -> ImportError:
    return ImportError(
        f"certus-grpc-connector: could not import {what} for vLLM {VERSION[0]}.{VERSION[1]} "
        f"({exc}). The vllm.v1.kv_offload API path may have moved in this version — "
        f"add the new path to the import ladder in compat.py."
    )


# The import ladder, made data. Each symbol maps to an ordered tuple of candidate
# module paths; the resolver tries them in order and uses the first that provides
# the symbol. A module *rename* across a vLLM version is absorbed by prepending
# the new path here — nothing else in the package changes. This IS the visible
# record of where each symbol lives per version.
#
# Verified relocations:
#   0.20  — LoadStoreSpec/OffloadingEvent/OffloadingManager/OffloadKey/
#           PrepareStoreOutput in ``vllm.v1.kv_offload.abstract``; GPULoadStoreSpec
#           in ``.mediums``; OffloadingSpec in ``.spec``.
#   0.22  — all of the above consolidated into ``vllm.v1.kv_offload.base``
#           (``abstract``/``mediums``/``spec`` modules removed). ``worker.worker``
#           unchanged. Confirmed empirically against the v0.22.0 image.
#   0.26  — offloading-API rewrite. ``TransferResult`` moved into ``base`` too;
#           the worker base class became ``OffloadingWorker`` (in ``base``) and
#           ``worker.worker`` was removed, so ``OffloadingHandler`` no longer
#           exists — it is resolved via ``worker_base_class()`` (which also serves
#           the ≤0.24 ``OffloadingHandler``) rather than the mandatory ladder,
#           because a symbol that is absent on some supported version cannot live
#           here (``_load_vllm_symbols`` resolves the whole dict at once).
#           ``TransferSpec`` (a ≤0.24 type-only alias) is likewise not resolved
#           here; it is only used as a string annotation.
_SYMBOL_PATHS: dict[str, tuple[str, ...]] = {
    "LoadStoreSpec": ("vllm.v1.kv_offload.base", "vllm.v1.kv_offload.abstract"),
    "OffloadingEvent": ("vllm.v1.kv_offload.base", "vllm.v1.kv_offload.abstract"),
    "OffloadingManager": ("vllm.v1.kv_offload.base", "vllm.v1.kv_offload.abstract"),
    "OffloadKey": ("vllm.v1.kv_offload.base", "vllm.v1.kv_offload.abstract"),
    "PrepareStoreOutput": ("vllm.v1.kv_offload.base", "vllm.v1.kv_offload.abstract"),
    "GPULoadStoreSpec": ("vllm.v1.kv_offload.base", "vllm.v1.kv_offload.mediums"),
    "OffloadingSpec": ("vllm.v1.kv_offload.base", "vllm.v1.kv_offload.spec"),
    "TransferResult": (
        "vllm.v1.kv_offload.base",  # 0.26
        "vllm.v1.kv_offload.worker.worker",  # <=0.24
    ),
}

# Symbols whose module path has been stable across every supported version.
_CONFIG_PATHS: dict[str, str] = {
    "VllmConfig": "vllm.config",
    "KVCacheConfig": "vllm.v1.kv_cache_interface",
}


def _load_vllm_symbols() -> dict[str, object]:
    """Resolve and cache the vLLM plugin symbols. Idempotent; raises a clear
    ImportError (naming the running version) if a symbol's module path has moved
    beyond every candidate in ``_SYMBOL_PATHS``."""
    if _VLLM_SYMBOLS:
        return _VLLM_SYMBOLS

    import importlib

    resolved: dict[str, object] = {}

    for sym, modpath in _CONFIG_PATHS.items():
        try:
            resolved[sym] = getattr(importlib.import_module(modpath), sym)
        except Exception as _e:  # noqa: BLE001
            raise _import_error(f"{sym} (from {modpath})", _e)

    for sym, candidates in _SYMBOL_PATHS.items():
        last_exc: Exception | None = None
        for modpath in candidates:
            try:
                resolved[sym] = getattr(importlib.import_module(modpath), sym)
                break
            except (ImportError, AttributeError) as _e:
                last_exc = _e
        else:
            raise _import_error(
                f"{sym} (tried: {', '.join(candidates)})",
                last_exc or ImportError(sym),
            )

    _VLLM_SYMBOLS.update(resolved)
    return _VLLM_SYMBOLS


# PEP 562: resolve the re-exported vLLM symbols on first attribute access, so
# ``from .compat import OffloadingSpec`` and ``compat.TransferResult`` work while
# the pure matrix stays importable without vLLM.
def __getattr__(name: str):
    if name in _VLLM_LAZY_NAMES:
        return _load_vllm_symbols()[name]
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


_VLLM_LAZY_NAMES = frozenset(
    [
        "VllmConfig",
        "KVCacheConfig",
        "LoadStoreSpec",
        "OffloadingEvent",
        "OffloadingManager",
        "OffloadKey",
        "PrepareStoreOutput",
        "GPULoadStoreSpec",
        "OffloadingSpec",
        "TransferResult",
    ]
)


__all__ = [
    "VERSION",
    "CAPS",
    "FEATURES",
    "SUPPORTED_VERSIONS",
    "caps_for",
    "VllmConfig",
    "KVCacheConfig",
    "LoadStoreSpec",
    "OffloadingEvent",
    "OffloadingManager",
    "OffloadKey",
    "PrepareStoreOutput",
    "GPULoadStoreSpec",
    "OffloadingSpec",
    "TransferResult",
    "make_transfer_result",
    "extract_gpu_ptrs",
    "block_bytes_from_config",
    "block_bytes_from_offloading_config",
    "gpu_block_ids",
    "new_request_offloading_context",
    "lookup_result",
    "worker_base_class",
]


# ── adapters ───────────────────────────────────────────────────────────────


def make_transfer_result(
    job_id: int,
    success: bool,
    transfer_size: int,
    transfer_time: float,
    transfer_type,
) -> "TransferResult":
    """Build a vLLM ``TransferResult``, absorbing field add/remove across versions.

    Constructed by keyword so field *reordering* never breaks; ``CAPS`` gates the
    presence of the ``transfer_type`` field for versions that drop/rename it.
    """
    TransferResult = _load_vllm_symbols()["TransferResult"]
    if CAPS.transfer_result_has_type:
        return TransferResult(
            job_id=job_id,
            success=success,
            transfer_size=transfer_size,
            transfer_time=transfer_time,
            transfer_type=transfer_type,
        )
    return TransferResult(
        job_id=job_id,
        success=success,
        transfer_size=transfer_size,
        transfer_time=transfer_time,
    )


def _lazy_base_attr(name: str):
    """Resolve a symbol from ``vllm.v1.kv_offload.base`` on demand.

    Used for 0.26-only types (``OffloadingWorker``, ``LookupResult``,
    ``RequestOffloadingContext``) that must NOT sit in the mandatory
    ``_SYMBOL_PATHS`` ladder — that resolves the whole dict at once, so a symbol
    absent on an older supported version would break resolution there. These are
    only ever touched on the versions that have them.
    """
    import importlib

    try:
        return getattr(importlib.import_module("vllm.v1.kv_offload.base"), name)
    except (ImportError, AttributeError) as _e:
        raise _import_error(f"{name} (from vllm.v1.kv_offload.base)", _e)


def extract_gpu_ptrs(kv_caches) -> list[tuple[int, int]]:
    """(base pointer, per-block stride bytes) for EACH KV-cache tensor.

    The per-block stride is ``stride(0)`` of each block tensor, in bytes. This is
    the single highest-risk silent-break point across vLLM versions — a new
    attention/cache backend can change the shape — so it is isolated here.

    Measured 2026-07-30: vLLM 0.20/0.22 present ONE coalesced tensor (all layers,
    stride = full 2 MiB block); 0.23+ split the KV cache into N per-layer tensors
    (Llama-3-8B: 32 x ``(num_blocks, 65536)`` int8), each a SEPARATE allocation
    with its own ``data_ptr``. We return one ``(ptr, stride)`` per tensor: the
    caller opens one IPC handle per region and the block is stored/loaded as N
    colocated regions in one slot. A single-tensor layout is simply ``N == 1`` —
    there is no version branch here, only a per-tensor loop.

    Two object shapes expose the same per-tensor layout:

    * **0.26+ ``CanonicalKVCaches``** (``CAPS.canonical_kv_caches``): a list of
      unique block tensors, each ``(num_blocks, page_size_bytes)`` int8, plus
      per-group refs.
    * **≤0.24** (``CAPS.kv_caches_tensors_attr``): the object ``get_handlers``
      receives exposes the same ``.tensors[i].tensor`` layout.
    """
    if CAPS.canonical_kv_caches:
        tensors = kv_caches.tensors
        shapes = [
            (tuple(t.tensor.shape), int(t.page_size_bytes)) for t in tensors
        ]
        print(
            f"[certus-grpc] CanonicalKVCaches: {len(tensors)} tensor(s); "
            f"(shape, page_size_bytes)={shapes}; "
            f"groups={len(kv_caches.group_data_refs)}",
            flush=True,
        )
        # One IPC region per canonical tensor; the server lands the N regions
        # colocated in a single N*page slot. A single-tensor layout is N == 1.
        regions: list[tuple[int, int]] = []
        for t in tensors:
            tensor = t.tensor
            stride_bytes = tensor.stride(0) * tensor.element_size()
            regions.append((tensor.data_ptr(), stride_bytes))
        return regions
    if CAPS.kv_caches_tensors_attr:
        # ≤0.24 shares the CanonicalKVCaches-style ``.tensors`` layout. Measured
        # 2026-07-30: 0.20/0.22 present ONE coalesced tensor (all layers, stride =
        # full 2 MiB block); 0.23/0.24 split the KV cache into per-layer tensors
        # (Llama-3: 32 x (num_blocks, 65536) int8). We export one IPC region per
        # tensor and the server lands the N regions colocated in a single N*page
        # slot — no truncation to layer 0, no version branch (N == 1 for 0.20/0.22).
        tensors = kv_caches.tensors
        shapes = [
            (tuple(t.tensor.shape), int(t.page_size_bytes)) for t in tensors
        ]
        print(
            f"[certus-grpc] KV tensors: {len(tensors)} tensor(s); "
            f"(shape, page_size_bytes)={shapes}; "
            f"groups={len(kv_caches.group_data_refs)}",
            flush=True,
        )
        regions = []
        for t in tensors:
            tensor = t.tensor
            stride_bytes = tensor.stride(0) * tensor.element_size()
            regions.append((tensor.data_ptr(), stride_bytes))
        return regions
    raise NotImplementedError(
        f"certus-grpc-connector: KV-cache tensor layout for vLLM {VERSION[0]}.{VERSION[1]} "
        f"is not yet mapped in compat.extract_gpu_ptrs. Inspect the object passed to "
        f"OffloadingSpec.get_handlers on this version and add a branch."
    )


def _block_bytes_from_kv_cache_tensors(
    kv_cache_config,
    block_size_factor: int,
) -> int | None:
    """Per-offloaded-block bytes from KVCacheConfig.kv_cache_tensors.

    This mirrors vLLM's own CPU offload sizing path: for unpacked layouts, sum all
    tensor byte sizes; for packed layouts, use the first packed tensor's byte size.
    Divide by the number of GPU blocks to get bytes per GPU block, then scale by
    ``block_size_factor`` if the offloaded block spans multiple GPU blocks.
    """
    tensors = getattr(kv_cache_config, "kv_cache_tensors", None)
    num_blocks = int(getattr(kv_cache_config, "num_blocks", 0) or 0)
    if not tensors or num_blocks <= 0:
        return None

    is_packed = any(int(getattr(t, "block_stride", 0) or 0) for t in tensors)
    if is_packed and not all(int(getattr(t, "block_stride", 0) or 0) for t in tensors):
        raise ValueError("mixed packed/unpacked kv_cache_tensors")

    total_gpu_kv_bytes = (
        int(tensors[0].size) if is_packed else sum(int(t.size) for t in tensors)
    )
    if total_gpu_kv_bytes <= 0:
        return None

    per_gpu_block, rem = divmod(total_gpu_kv_bytes, num_blocks)
    if rem:
        # Round up rather than under-reserving. This should not normally happen;
        # sizes produced by vLLM are expected to be block-aligned.
        per_gpu_block += 1
        print(
            f"[certus-grpc] WARNING: kv_cache_tensors total bytes "
            f"{total_gpu_kv_bytes} not divisible by num_blocks={num_blocks}; "
            f"rounding per-GPU-block Reserve size up to {per_gpu_block} bytes",
            flush=True,
        )

    block_bytes = per_gpu_block * block_size_factor
    layout = "packed" if is_packed else "unpacked"
    print(
        f"[certus-grpc] per-block Reserve size from kv_cache_tensors "
        f"({layout}): total_gpu_kv_bytes={total_gpu_kv_bytes} / "
        f"num_blocks={num_blocks} * block_size_factor={block_size_factor} = "
        f"{block_bytes} bytes",
        flush=True,
    )
    return block_bytes


def block_bytes_from_config(kv_cache_config, block_size_factor: int) -> int | None:
    """True offloaded per-block size in bytes, derived from the KV-cache config
    (available to both the scheduler and worker roles, unlike the GPU tensor).

    Prefer ``kv_cache_tensors`` because it captures split and packed tensor
    layouts directly. Fall back to the older group-derived
    ``page_size_bytes * num_layers * block_size_factor`` calculation only when the
    tensor metadata is unavailable. Returns None if neither path can be read
    (caller falls back to ``slab_size_bytes``).
    """
    if not CAPS.kv_cache_group_attrs:
        raise NotImplementedError(
            f"certus-grpc-connector: KVCacheConfig group layout for vLLM "
            f"{VERSION[0]}.{VERSION[1]} is not yet mapped in "
            f"compat.block_bytes_from_config. Add a branch for this version."
        )
    try:
        if block_bytes := _block_bytes_from_kv_cache_tensors(
            kv_cache_config, block_size_factor
        ):
            return block_bytes
    except Exception as e:  # noqa: BLE001 - fall through to group fallback
        print(
            f"[certus-grpc] WARNING: could not derive per-block size from "
            f"kv_cache_tensors ({e}); falling back to KV-cache groups.",
            flush=True,
        )

    try:
        groups = kv_cache_config.kv_cache_groups
        if len(groups) != 1:
            return None
        num_layers = len(groups[0].layer_names)
        page = int(groups[0].kv_cache_spec.page_size_bytes)
        block_bytes = page * num_layers * block_size_factor
        print(
            f"[certus-grpc] per-block Reserve size from KV-cache config: "
            f"page_size_bytes={page} * num_layers={num_layers} * "
            f"block_size_factor={block_size_factor} = {block_bytes} bytes",
            flush=True,
        )
        return block_bytes
    except Exception as e:  # noqa: BLE001 - fall back to slab_size_bytes
        print(
            f"[certus-grpc] WARNING: could not derive per-block size from "
            f"KV-cache config ({e}); falling back to slab_size_bytes. If it is "
            f"smaller than the real block, stores will fail their D2H bounds check.",
            flush=True,
        )
        return None


def gpu_block_ids(load_store_spec) -> list[int]:
    """Ordered GPU block ids from a ``GPULoadStoreSpec``. Isolated so a change to
    how vLLM stores block ids (np array vs list, attribute name) lives in one place."""
    return [int(b) for b in load_store_spec.block_ids]


def new_request_offloading_context():
    """Default per-request offloading context for ``OffloadingManager.on_new_request``
    (0.24+). Returns ``RequestOffloadingContext()`` — the BLOCK_LEVEL policy: offload
    newly-computed blocks and skip prefix hits, which matches the connector's Check
    filter in ``prepare_store``.

    Resolved lazily (not via the mandatory ``_SYMBOL_PATHS`` ladder) because the type
    only exists on 0.24+; loading it there would break symbol resolution on 0.20/0.22.
    Only ever called on ``CAPS.has_on_new_request`` versions.
    """
    return _lazy_base_attr("RequestOffloadingContext")()


def lookup_result(exists: bool):
    """Return the shape ``OffloadingManager.lookup`` must yield for this version.

    * **0.26+** (``CAPS.lookup_returns_enum``): a ``LookupResult`` enum —
      ``HIT`` when the key is present, ``MISS`` otherwise. (The richer
      ``HIT_PENDING`` / ``RETRY`` states model in-flight/backpressure cases the
      connector's synchronous certus-server lookup never produces.)
    * **≤0.24**: a plain ``bool``.
    """
    if CAPS.lookup_returns_enum:
        LookupResult = _lazy_base_attr("LookupResult")
        return LookupResult.HIT if exists else LookupResult.MISS
    return bool(exists)


def block_bytes_from_offloading_config(config) -> int:
    """Per-block offload size in bytes on 0.26, read straight off the
    ``OffloadingConfig``.

    0.26 exposes ``worker_kv_bytes_per_block`` directly (the full per-block bytes
    across all layers in the group), so the connector no longer has to reconstruct
    it from ``KVCacheConfig`` groups the way ``block_bytes_from_config`` does for
    ≤0.24. Used to size the certus-server Reserve and cross-checked against the
    canonical tensor's ``stride(0)`` at ``get_worker`` time.
    """
    block_bytes = int(config.worker_kv_bytes_per_block)
    print(
        f"[certus-grpc] per-block Reserve size from OffloadingConfig: "
        f"worker_kv_bytes_per_block = {block_bytes} bytes",
        flush=True,
    )
    return block_bytes


def worker_base_class():
    """The base class the connector's worker must subclass on this version.

    * **0.26+** (``CAPS.worker_split_submit``): ``OffloadingWorker`` (in
      ``vllm.v1.kv_offload.base``) — one instance, explicit
      ``submit_store`` / ``submit_load``.
    * **≤0.24**: ``OffloadingHandler`` (in ``vllm.v1.kv_offload.worker.worker``)
      — per-direction ``transfer_async(job_id, spec)``.

    Resolved lazily and referenced ONLY from the worker factory, so the base
    absent on the other era is never imported. It cannot live in the mandatory
    ``_SYMBOL_PATHS`` ladder for the same reason (that resolves the whole dict at
    once).
    """
    if CAPS.worker_split_submit:
        return _lazy_base_attr("OffloadingWorker")
    import importlib

    try:
        mod = importlib.import_module("vllm.v1.kv_offload.worker.worker")
        return mod.OffloadingHandler
    except (ImportError, AttributeError) as _e:
        raise _import_error(
            "OffloadingHandler (from vllm.v1.kv_offload.worker.worker)", _e
        )


# ── matrix CLI ─────────────────────────────────────────────────────────────


def render_matrix() -> str:
    """Render the feature x supported-version matrix as text."""
    names = list(FEATURES)
    col = max(len(n) for n in names) + 2
    header = "feature".ljust(col) + "".join(
        f"{v[0]}.{v[1]:<6}" for v in SUPPORTED_VERSIONS
    )
    lines = [f"vLLM compat matrix (detected running version: {VERSION[0]}.{VERSION[1]})", header, "-" * len(header)]
    for name in names:
        row = name.ljust(col)
        for v in SUPPORTED_VERSIONS:
            row += f"{'yes' if FEATURES[name](v) else '.':<8}"
        lines.append(row)
    return "\n".join(lines)


if __name__ == "__main__":
    print(render_matrix())
