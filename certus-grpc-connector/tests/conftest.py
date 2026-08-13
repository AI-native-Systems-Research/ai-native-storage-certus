# SPDX-License-Identifier: Apache-2.0
"""Install minimal fake ``vllm`` modules so the connector imports without vLLM.

The connector's manager/handler/spec/mediums import a handful of vLLM classes at
module load. vLLM is a heavy GPU dependency not present in unit-test CI, so we
register just-enough stand-ins here. These stubs mirror only the attributes the
connector actually touches; they are not a functional vLLM.

The fakes are produced by a per-version *factory* (``build_fake_vllm``) so a
future vLLM whose plugin surface diverges gets its shape encoded in ONE place,
alongside the ``compat.FEATURES`` entry that gates the connector's adaptation.
The plugin surface has THREE distinct module layouts across the supported range,
and the factory reproduces each so the connector's import ladder is exercised
against the shape it will really meet (verified against the release images):

  * **0.20/0.21 — split modules.** ``OffloadKey``/``LoadStoreSpec``/
    ``OffloadingEvent``/``PrepareStoreOutput``/``OffloadingManager`` live in
    ``vllm.v1.kv_offload.abstract``, ``GPULoadStoreSpec`` in ``.mediums``,
    ``OffloadingSpec`` in ``.spec``; ``OffloadingHandler``/``TransferResult`` in
    ``.worker.worker``.
  * **0.22/0.23/0.24 — consolidated ``base``.** ``abstract``/``mediums``/``spec``
    were removed and their classes moved into ``vllm.v1.kv_offload.base``;
    ``.worker.worker`` is retained (still holds ``OffloadingHandler``/
    ``TransferResult``). 0.23 added ``RequestOffloadingContext`` to ``base``.
  * **0.26 — offloading-API rewrite.** ``TransferResult`` moved into ``base``,
    the worker base became ``OffloadingWorker`` (in ``base``, no more
    ``.worker.worker``), and ``OffloadingConfig`` lives in a new ``.config``.

Keying the layout off the same version thresholds the connector's ``compat``
branches on keeps the fakes and the adapters from silently drifting apart.
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
    OffloadingWorker, OffloadingConfig ctor, LookupResult enum, CanonicalKVCaches).
    Mirrors the ``worker_split_submit`` / ``spec_config_object`` predicates."""
    return version >= (0, 26)


def _is_consolidated_base(version: tuple[int, int]) -> bool:
    """Whether the classes live in the consolidated ``vllm.v1.kv_offload.base``
    module (0.22–0.24) rather than the 0.20-era split abstract/mediums/spec.

    0.22 removed abstract/mediums/spec and moved their classes into ``base``,
    while KEEPING ``.worker.worker``; the 0.26 rewrite then absorbed
    ``.worker.worker`` too (handled by ``_is_0_26``). Confirmed empirically
    against the release images."""
    return (0, 22) <= version < (0, 26)


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
        # Mirror vLLM (all versions v0.20+): ``block_ids``, ``group_sizes`` and
        # ``block_indices`` are ALL required positional args, tied together by
        # two asserts. block_ids is stored as an np.int64 array. The connector
        # only reads ``.block_ids``, but the fake keeps the real signature so a
        # test that constructs one the way vLLM's scheduler does is validated
        # against the shape real vLLM would accept (bare ``GPULoadStoreSpec(ids)``
        # is a TypeError against real vLLM and must be here too).
        def __init__(self, block_ids, group_sizes, block_indices):
            assert sum(group_sizes) == len(block_ids), (
                "group_sizes must sum to len(block_ids)"
            )
            assert len(block_indices) == len(group_sizes), (
                "block_indices must be one per group"
            )
            self.block_ids = np.array(block_ids, dtype=np.int64)
            self.group_sizes = group_sizes
            self.block_indices = block_indices

    # TransferResult's field set is version-varying: the ``transfer_type`` field
    # is present iff the matching FEATURES predicate says so, so the fake matches
    # exactly the shape make_transfer_result will construct on this version.
    # transfer_size/transfer_time/transfer_type default to None, as in real vLLM
    # (``TransferResult(job_id=1, success=True)`` is valid there and must be here).
    tr_fields = [
        ("job_id", int),
        ("success", bool),
        ("transfer_size", "int | None", None),
        ("transfer_time", "float | None", None),
    ]
    if _transfer_result_has_type(version):
        tr_fields.append(("transfer_type", "Any | None", None))
    TransferResult = make_dataclass("TransferResult", tr_fields)

    # ≤0.24 base ``OffloadingSpec.__init__(vllm_config, kv_cache_config)``. Real
    # vLLM's base sets six attributes on ``self``; the connector's spec.py reads
    # ``gpu_block_size`` / ``block_size_factor`` / ``extra_config`` off ``self``
    # AFTER ``super().__init__()``. Mirror that here (derived from the passed
    # configs, with fallbacks so a minimal stub config still works) so the fake
    # actually validates that the base produces those attributes — the earlier
    # ``def __init__(self, *a, **k): pass`` set nothing and hid that dependency.
    class LegacyOffloadingSpec:
        def __init__(self, vllm_config, kv_cache_config):
            self.vllm_config = vllm_config
            self.kv_cache_config = kv_cache_config
            kv_transfer = getattr(vllm_config, "kv_transfer_config", None)
            self.extra_config = (
                getattr(kv_transfer, "kv_connector_extra_config", None) or {}
            )
            self.hash_block_size = getattr(vllm_config, "hash_block_size", 0)
            # Real base exposes gpu_block_size as a tuple (one entry per KV-cache
            # group); the connector asserts len == 1.
            self.gpu_block_size = getattr(vllm_config, "gpu_block_size", (16,))
            self.block_size_factor = getattr(vllm_config, "block_size_factor", 1)

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

        # OffloadingSpec base takes a single OffloadingConfig and exposes
        # extra_config (the connector reads server/slab_size_bytes from it).
        class OffloadingSpec:
            def __init__(self, config):
                self.config = config
                self.extra_config = getattr(config, "extra_config", {})

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

        # Full 0.26 OffloadingConfig field set (config.py). The connector reads
        # ``worker_kv_bytes_per_block`` and ``extra_config``; the other six fields
        # (groups/enable_kv_cache_events/engine_id/model/cache/parallel) are real
        # too, so the fake carries them — otherwise a connector change that starts
        # reading one would pass here but fail against real vLLM. All are given
        # defaults so a test can build a minimal config, but every real attribute
        # name is present.
        @dataclass
        class OffloadingConfig:
            worker_kv_bytes_per_block: int = 0
            extra_config: Any = None
            groups: Any = ()
            enable_kv_cache_events: bool = False
            engine_id: str = ""
            model: Any = None
            cache: Any = None
            parallel: Any = None

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
    elif _is_consolidated_base(version):
        # ── 0.22–0.24: abstract/mediums/spec removed; their classes live in the
        # consolidated ``base`` module. ``.worker.worker`` is retained (still
        # holds OffloadingHandler + TransferResult). 0.23 added
        # RequestOffloadingContext to ``base``. ──
        base = _module("vllm.v1.kv_offload.base")
        _module("vllm.v1.kv_offload.worker")
        worker = _module("vllm.v1.kv_offload.worker.worker")

        base.OffloadKey = OffloadKey
        base.LoadStoreSpec = LoadStoreSpec
        base.OffloadingEvent = OffloadingEvent
        base.PrepareStoreOutput = PrepareStoreOutput
        base.OffloadingManager = OffloadingManager
        base.GPULoadStoreSpec = GPULoadStoreSpec
        base.OffloadingSpec = LegacyOffloadingSpec

        # 0.23 added the abstract on_new_request whose return type
        # RequestOffloadingContext lives in ``base``; the connector's compat
        # gates on has_on_new_request (>= 0.23) and resolves it lazily from base.
        if version >= (0, 23):

            @dataclass
            class RequestOffloadingContext:
                policy: Any = None

            base.RequestOffloadingContext = RequestOffloadingContext

        class OffloadingHandler:
            pass

        worker.OffloadingHandler = OffloadingHandler
        worker.TransferResult = TransferResult
        worker.TransferSpec = tuple
        worker.TransferType = tuple
    else:
        # ── 0.20/0.21: split modules (abstract/mediums/spec/worker.worker). ──
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
        spec.OffloadingSpec = LegacyOffloadingSpec

        class OffloadingHandler:
            pass

        worker.OffloadingHandler = OffloadingHandler
        worker.TransferResult = TransferResult
        worker.TransferSpec = tuple
        worker.TransferType = tuple


# Auto-install the baseline (0.20) fakes at collection time so the version-blind
# tests (test_manager, test_handler) import cleanly. Parametrized tests that need
# a specific version rebuild via build_fake_vllm + importlib.reload(compat).
if "vllm" not in sys.modules:
    build_fake_vllm((0, 20))
