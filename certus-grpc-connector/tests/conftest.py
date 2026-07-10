# SPDX-License-Identifier: Apache-2.0
"""Install minimal fake ``vllm`` modules so the connector imports without vLLM.

The connector's manager/handler/spec/mediums import a handful of vLLM classes at
module load. vLLM is a heavy GPU dependency not present in unit-test CI, so we
register just-enough stand-ins here. These stubs mirror only the attributes the
connector actually touches; they are not a functional vLLM.
"""

from __future__ import annotations

import sys
import types
from dataclasses import dataclass, field
from typing import Any


def _module(name: str) -> types.ModuleType:
    mod = types.ModuleType(name)
    sys.modules[name] = mod
    return mod


def _install_fake_vllm() -> None:
    if "vllm" in sys.modules:
        return

    _module("vllm")
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

    class GPULoadStoreSpec(LoadStoreSpec):
        def __init__(self, block_ids):
            self.block_ids = block_ids

    mediums.GPULoadStoreSpec = GPULoadStoreSpec

    class OffloadingSpec:
        def __init__(self, *a, **k):
            pass

    sys.modules["vllm.v1.kv_offload.spec"].OffloadingSpec = OffloadingSpec

    class OffloadingHandler:
        pass

    @dataclass
    class TransferResult:
        job_id: int
        success: bool
        transfer_size: int
        transfer_time: float
        transfer_type: Any

    worker.OffloadingHandler = OffloadingHandler
    worker.TransferResult = TransferResult
    worker.TransferSpec = tuple
    worker.TransferType = tuple


_install_fake_vllm()
