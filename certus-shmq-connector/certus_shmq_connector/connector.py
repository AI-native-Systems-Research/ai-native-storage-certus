# SPDX-License-Identifier: Apache-2.0
"""vLLM's OffloadingConnector, with an opt-in KV cache layout request.

vLLM resolves one KV cache layout for the whole model from the connector
*class*'s ``get_required_kvcache_layout()`` (vllm/v1/attention/backends/utils.py
``resolve_kv_cache_layout``); an ``OffloadingSpec`` such as
``CertusShmqOffloadingSpec`` has no say. By default this class makes the stock
``OffloadingConnector``'s request: ``LBHNC`` (layer-outermost), so every block
reaches Certus as one region per layer — 32 pieces of 64 KiB for Llama-3-8B —
and the layout matches ``NixlConnector``'s, so the two can share a
``MultiConnector``. Certus copies a block's pieces as batched GPU copies; on
vLLM 0.30 with Llama-3-8B, LongDocQA's query round took 3.06 s with ``LBHNC``
and 3.27 s with ``BLHNC`` (one contiguous piece per block).

``CERTUS_KV_CACHE_LAYOUT=<name>`` (e.g. ``BLHNC``) makes the class request that
layout instead. It is a request, not a setting: vLLM uses it only when every
attention backend in the model supports it, and otherwise logs a warning and
falls back to the backends' default, so a model on an unsupported backend still
starts. ``VLLM_KV_CACHE_LAYOUT``, when set, overrides it.

Use it in place of ``OffloadingConnector``:

    --kv-transfer-config '{"kv_connector": "CertusConnector",
      "kv_connector_module_path": "certus_shmq_connector.connector",
      "kv_role": "kv_both",
      "kv_connector_extra_config": {"spec_name": "CertusShmqOffloadingSpec",
        "spec_module_path": "certus_shmq_connector.spec", ...}}'
"""

from __future__ import annotations

import os

try:
    from vllm.distributed.kv_transfer.kv_connector.v1.offloading_connector import (
        OffloadingConnector as _OffloadingConnector,
    )
except ImportError:  # vLLM not installed (unit tests, benchmark shim)
    _OffloadingConnector = object


def requested_layout(stock: str | None) -> str | None:
    """Layout to request, given the stock OffloadingConnector's request: the
    stock request, unless ``CERTUS_KV_CACHE_LAYOUT`` names another one (empty:
    the stock request)."""
    override = os.environ.get("CERTUS_KV_CACHE_LAYOUT")
    if override is not None:
        return override.strip() or stock
    return stock


class CertusConnector(_OffloadingConnector):
    """``OffloadingConnector`` whose layout request is ``requested_layout``."""

    @classmethod
    def get_required_kvcache_layout(cls, vllm_config) -> str | None:
        stock = None
        if _OffloadingConnector is not object:
            stock = _OffloadingConnector.get_required_kvcache_layout(vllm_config)
        return requested_layout(stock)
