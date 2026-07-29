# SPDX-License-Identifier: Apache-2.0
"""Install minimal fake ``vllm`` modules so the connector imports without vLLM.

The connector's manager/handler/spec/mediums import a handful of vLLM classes at
module load. vLLM is a heavy GPU dependency not present in unit-test CI, so we
register just-enough stand-ins here. These stubs mirror only the attributes the
connector actually touches; they are not a functional vLLM.

The fakes are produced by a per-version *factory* (``build_fake_vllm``) so a
future vLLM whose plugin surface diverges gets its shape encoded in ONE place,
alongside the ``compat.FEATURES`` entry that gates the connector's adaptation.
Today the four supported even versions (0.20/0.22/0.24/0.26) share the 0.20
shape for everything except engine flags (``needs_disable_hybrid_kv_cache_manager``,
which is a run-driver concern, not an import-surface one), so the factory
currently emits the same modules for every version — but the seam is here, keyed
off the same predicates the connector branches on, so the fakes and the adapters
cannot silently drift apart.
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
SUPPORTED_VERSIONS: tuple[tuple[int, int], ...] = ((0, 20), (0, 22), (0, 24), (0, 26))


def _transfer_result_has_type(version: tuple[int, int]) -> bool:
    """Whether ``TransferResult`` carries the 5th ``transfer_type`` field.

    Mirrors ``compat.FEATURES['transfer_result_has_type']``. Duplicated here (not
    imported) for the load-order reason above; the fake's field set MUST track
    the real one so the ``make_transfer_result`` adapter is tested against the
    shape it will actually meet.
    """
    return version >= (0, 20)


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
    _module("vllm.config")
    _module("vllm.v1")
    _module("vllm.v1.kv_cache_interface")
    _module("vllm.v1.kv_offload")
    abstract = _module("vllm.v1.kv_offload.abstract")
    mediums = _module("vllm.v1.kv_offload.mediums")
    _module("vllm.v1.kv_offload.spec")
    _module("vllm.v1.kv_offload.worker")
    worker = _module("vllm.v1.kv_offload.worker.worker")

    sys.modules["vllm.config"].VllmConfig = object
    sys.modules["vllm.v1.kv_cache_interface"].KVCacheConfig = object

    OffloadKey = Any
    abstract.OffloadKey = OffloadKey

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

    abstract.LoadStoreSpec = LoadStoreSpec
    abstract.OffloadingEvent = OffloadingEvent
    abstract.PrepareStoreOutput = PrepareStoreOutput
    abstract.OffloadingManager = OffloadingManager

    import numpy as np

    class GPULoadStoreSpec(LoadStoreSpec):
        # Mirror vLLM 0.20.0: block_ids stored as an np.int64 array.
        def __init__(self, block_ids, group_sizes=None, block_indices=None):
            self.block_ids = np.array(block_ids, dtype=np.int64)

    mediums.GPULoadStoreSpec = GPULoadStoreSpec

    class OffloadingSpec:
        def __init__(self, *a, **k):
            pass

    sys.modules["vllm.v1.kv_offload.spec"].OffloadingSpec = OffloadingSpec

    class OffloadingHandler:
        pass

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

    worker.OffloadingHandler = OffloadingHandler
    worker.TransferResult = TransferResult
    worker.TransferSpec = tuple
    worker.TransferType = tuple


# Auto-install the baseline (0.20) fakes at collection time so the version-blind
# tests (test_manager, test_handler) import cleanly. Parametrized tests that need
# a specific version rebuild via build_fake_vllm + importlib.reload(compat).
if "vllm" not in sys.modules:
    build_fake_vllm((0, 20))
