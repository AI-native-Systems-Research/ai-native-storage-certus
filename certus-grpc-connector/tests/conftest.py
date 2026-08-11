# SPDX-License-Identifier: Apache-2.0
"""Install minimal fake ``vllm`` modules so the connector imports without vLLM.

The connector's manager/handler/spec/mediums import a handful of vLLM classes at
module load. vLLM is a heavy GPU dependency not present in unit-test CI, so we
register just-enough stand-ins here. These stubs mirror only the attributes the
connector actually touches; they are not a functional vLLM.

The fakes are produced by a per-version *factory* (``build_fake_vllm``) so a
future vLLM whose plugin surface diverges gets its shape encoded in ONE place,
alongside the ``compat.FEATURES`` entry that gates the connector's adaptation.
The factory models the shape changes the connector branches on, including the
0.26 consolidated ``base`` module and worker interface. It intentionally keeps
0.26's spec constructor as ``(vllm_config, kv_cache_config)`` because the local
vLLM factory still calls plugins that way.
"""

from __future__ import annotations

import sys
import types
from dataclasses import dataclass, make_dataclass
from typing import Any

# The even versions we produce fakes for. Mirrors compat.SUPPORTED_VERSIONS, but
# defined locally: compat imports vllm at module load, so it cannot be imported
# until AFTER a fake vllm is installed. Keep the two lists in sync (test_compat
# asserts they match).
SUPPORTED_VERSIONS: tuple[tuple[int, int], ...] = (
    (0, 20),
    (0, 22),
    (0, 23),
    (0, 24),
    (0, 26),
)


def _transfer_result_has_type(version: tuple[int, int]) -> bool:
    """Whether ``TransferResult`` carries the 5th ``transfer_type`` field.

    Mirrors ``compat.FEATURES['transfer_result_has_type']``. Duplicated here (not
    imported) for the load-order reason above; the fake's field set MUST track
    the real one so the ``make_transfer_result`` adapter is tested against the
    shape it will actually meet. 0.20/0.22/0.24 carry it; the 0.26 API rewrite
    dropped it (direction is explicit via submit_store/submit_load).
    """
    return version < (0, 26)


def _is_0_26(version: tuple[int, int]) -> bool:
    """Whether this version uses the 0.26 consolidated-``base`` rewrite (single
    OffloadingWorker, LookupResult enum, CanonicalKVCaches)."""
    return version >= (0, 26)


def _module(name: str) -> types.ModuleType:
    mod = types.ModuleType(name)
    sys.modules[name] = mod
    return mod


def _purge_fake_vllm() -> None:
    """Remove any previously-installed fake vllm modules from sys.modules so a
    fresh factory build (a different version) is picked up on the next import."""
    for name in [n for n in sys.modules if n == "vllm" or n.startswith("vllm.")]:
        del sys.modules[name]


def build_fake_vllm(version: tuple[int, int] = (0, 20)) -> None:
    """Install fake ``vllm`` modules shaped for the given (major, minor) version.

    Idempotent per interpreter state: purges any prior fake first, so callers can
    rebuild for a different version between imports. Sets ``vllm.__version__`` so
    ``compat._detect_version`` resolves to ``version`` when reloaded.
    """
    _purge_fake_vllm()

    vllm = _module("vllm")
    vllm.__version__ = f"{version[0]}.{version[1]}.0"
    _module("vllm.config").VllmConfig = object
    _module("vllm.v1")
    _module("vllm.v1.kv_cache_interface").KVCacheConfig = object
    _module("vllm.v1.kv_offload")

    import numpy as np

    # ── classes shared by every version (only their MODULE LOCATION moves) ──

    OffloadKey = Any

    class LoadStoreSpec:  # noqa: D401 - stub
        pass

    @dataclass
    class OffloadingEvent:
        keys: list
        medium: str
        removed: bool

    @dataclass
    class PrepareStoreOutput:
        keys_to_store: list
        store_spec: Any
        evicted_keys: list

    class OffloadingManager:
        pass

    class GPULoadStoreSpec(LoadStoreSpec):
        # Mirror vLLM: block_ids stored as an np.int64 array. 0.26 added required
        # group_sizes/block_indices; the connector only reads .block_ids.
        def __init__(self, block_ids, group_sizes=None, block_indices=None):
            self.block_ids = np.array(block_ids, dtype=np.int64)
            self.group_sizes = group_sizes
            self.block_indices = block_indices

    # TransferResult's field set is version-varying: the ``transfer_type`` field
    # is present iff the matching FEATURES predicate says so, so the fake matches
    # exactly the shape make_transfer_result will construct on this version.
    tr_fields = [
        ("job_id", int),
        ("success", bool),
        ("transfer_size", int),
        ("transfer_time", float),
    ]
    if _transfer_result_has_type(version):
        tr_fields.append(("transfer_type", Any))
    TransferResult = make_dataclass("TransferResult", tr_fields)

    def _init_offloading_spec(self, vllm_config, kv_cache_config) -> None:
        self.vllm_config = vllm_config
        self.kv_cache_config = kv_cache_config
        self.extra_config = (
            vllm_config.kv_transfer_config.kv_connector_extra_config
        )
        parallel_config = getattr(
            vllm_config,
            "parallel_config",
            types.SimpleNamespace(
                decode_context_parallel_size=1,
                prefill_context_parallel_size=1,
            ),
        )
        context_parallel_factor = (
            parallel_config.decode_context_parallel_size
            * parallel_config.prefill_context_parallel_size
        )
        self.gpu_block_size = tuple(
            group.kv_cache_spec.block_size * context_parallel_factor
            for group in kv_cache_config.kv_cache_groups
        )
        self.block_size_factor = 1
        offloaded_block_size = self.extra_config.get("block_size")
        if offloaded_block_size is not None:
            gpu_block_sizes = set(self.gpu_block_size)
            assert len(gpu_block_sizes) == 1
            gpu_block_size = gpu_block_sizes.pop()
            offloaded_block_size = int(offloaded_block_size)
            assert offloaded_block_size % gpu_block_size == 0
            self.block_size_factor = offloaded_block_size // gpu_block_size

    if _is_0_26(version):
        # ── 0.26 rewrite: everything consolidated into vllm.v1.kv_offload.base;
        # abstract/mediums/spec/worker.worker are GONE. ──
        base = _module("vllm.v1.kv_offload.base")
        config_mod = _module("vllm.v1.kv_offload.config")

        from enum import Enum, auto

        class LookupResult(Enum):
            MISS = auto()
            HIT = auto()
            HIT_PENDING = auto()
            RETRY = auto()

        @dataclass
        class RequestOffloadingContext:
            policy: Any = None

        # Local 0.26 still constructs plugin specs as
        # spec_cls(vllm_config, kv_cache_config), while the worker/manager API has
        # moved to the consolidated base module.
        class OffloadingSpec:
            def __init__(self, vllm_config, kv_cache_config):
                _init_offloading_spec(self, vllm_config, kv_cache_config)

        # OffloadingWorker: the single-worker ABC. Not marked abstract in the
        # fake (the connector subclass provides every method anyway).
        class OffloadingWorker:
            def submit_store(self, job_id, src_spec, dst_spec):  # noqa: D401
                raise NotImplementedError

            def submit_load(self, job_id, src_spec, dst_spec):
                raise NotImplementedError

            def get_finished(self):
                raise NotImplementedError

            def wait(self, job_ids):
                raise NotImplementedError

        @dataclass
        class CanonicalKVCacheTensor:
            tensor: Any
            page_size_bytes: int

        @dataclass
        class CanonicalKVCaches:
            tensors: list
            group_data_refs: list

        @dataclass
        class OffloadingConfig:
            worker_kv_bytes_per_block: int
            extra_config: Any

        for name, obj in {
            "OffloadKey": OffloadKey,
            "LoadStoreSpec": LoadStoreSpec,
            "OffloadingEvent": OffloadingEvent,
            "PrepareStoreOutput": PrepareStoreOutput,
            "OffloadingManager": OffloadingManager,
            "GPULoadStoreSpec": GPULoadStoreSpec,
            "OffloadingSpec": OffloadingSpec,
            "TransferResult": TransferResult,
            "OffloadingWorker": OffloadingWorker,
            "LookupResult": LookupResult,
            "RequestOffloadingContext": RequestOffloadingContext,
            "CanonicalKVCacheTensor": CanonicalKVCacheTensor,
            "CanonicalKVCaches": CanonicalKVCaches,
        }.items():
            setattr(base, name, obj)
        config_mod.OffloadingConfig = OffloadingConfig
    else:
        # ── ≤0.24 layout: split modules (abstract/mediums/spec/worker.worker).
        # 0.23 added an abstract on_new_request whose return type
        # RequestOffloadingContext lives in the consolidated ``base`` module, so
        # a partial base is provided from 0.23 for the lazy resolver to find. ──
        abstract = _module("vllm.v1.kv_offload.abstract")
        mediums = _module("vllm.v1.kv_offload.mediums")
        spec = _module("vllm.v1.kv_offload.spec")
        _module("vllm.v1.kv_offload.worker")
        worker = _module("vllm.v1.kv_offload.worker.worker")

        abstract.OffloadKey = OffloadKey
        abstract.LoadStoreSpec = LoadStoreSpec
        abstract.OffloadingEvent = OffloadingEvent
        abstract.PrepareStoreOutput = PrepareStoreOutput
        abstract.OffloadingManager = OffloadingManager
        mediums.GPULoadStoreSpec = GPULoadStoreSpec

        class OffloadingSpec:
            def __init__(self, vllm_config, kv_cache_config):
                _init_offloading_spec(self, vllm_config, kv_cache_config)

        spec.OffloadingSpec = OffloadingSpec

        class OffloadingHandler:
            pass

        worker.OffloadingHandler = OffloadingHandler
        worker.TransferResult = TransferResult
        worker.TransferSpec = tuple
        worker.TransferType = tuple

        if version >= (0, 24):
            base = _module("vllm.v1.kv_offload.base")

            @dataclass
            class RequestOffloadingContext:
                policy: Any = None

            base.RequestOffloadingContext = RequestOffloadingContext


# Auto-install the baseline (0.20) fakes at collection time so the version-blind
# tests (test_manager, test_handler) import cleanly. Parametrized tests that need
# a specific version rebuild via build_fake_vllm + importlib.reload(compat).
if "vllm" not in sys.modules:
    build_fake_vllm((0, 20))
