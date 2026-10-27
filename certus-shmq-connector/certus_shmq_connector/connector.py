# SPDX-License-Identifier: Apache-2.0
"""vLLM's OffloadingConnector, asking for one contiguous KV piece per block.

vLLM resolves one KV cache layout for the whole model from the connector
*class*'s ``get_required_kvcache_layout()`` (vllm/v1/attention/backends/utils.py
``resolve_kv_cache_layout``); an ``OffloadingSpec`` such as
``CertusShmqOffloadingSpec`` has no say. The stock ``OffloadingConnector``
requests ``LBHNC`` (layer-outermost), so every block reaches Certus as one
region per layer — 32 pieces of 64 KiB for Llama-3-8B — and the server copies
each piece separately. ``BLHNC`` (block-outermost) makes a block one contiguous
piece across all layers; on vLLM 0.30 with Llama-3-8B that halved
LongDocQA's query round (7.6 s to 3.9 s).

This class changes only that request. It is a request, not a setting: vLLM uses
it only when every attention backend in the model supports ``BLHNC``, and
otherwise logs a warning and falls back to the backends' default, so a model on
an unsupported backend still starts. ``VLLM_KV_CACHE_LAYOUT``, when set,
overrides it. Before vLLM 0.29 the hook took legacy layout names and no
block-outermost layout existed, so there it defers to the stock connector.

Use it in place of ``OffloadingConnector``:

    --kv-transfer-config '{"kv_connector": "CertusConnector",
      "kv_connector_module_path": "certus_shmq_connector.connector",
      "kv_role": "kv_both",
      "kv_connector_extra_config": {"spec_name": "CertusShmqOffloadingSpec",
        "spec_module_path": "certus_shmq_connector.spec", ...}}'

``CERTUS_KV_CACHE_LAYOUT`` overrides the requested layout (e.g. ``LBHNC`` to
compare against the stock request); an empty value defers to the stock
connector.
"""

from __future__ import annotations

import os

from .compat import CAPS

# Requested when nothing overrides it: one contiguous all-layer piece per block.
DEFAULT_LAYOUT = "BLHNC"

try:
    from vllm.distributed.kv_transfer.kv_connector.v1.offloading_connector import (
        OffloadingConnector as _OffloadingConnector,
    )
except ImportError:  # vLLM not installed (unit tests, benchmark shim)
    _OffloadingConnector = object


def requested_layout(stock: str | None) -> str | None:
    """Layout to request, given the stock OffloadingConnector's request.

    The stock request is kept where it carries meaning Certus must not override
    (hisparse asks for BLHNC already; any non-default stock choice is respected
    by only replacing the plain LBHNC default).
    """
    override = os.environ.get("CERTUS_KV_CACHE_LAYOUT")
    if override is not None:
        return override.strip() or stock
    if not CAPS.kvcache_layout_enum:
        return stock
    if stock in (None, "LBHNC"):
        return DEFAULT_LAYOUT
    return stock


class CertusConnector(_OffloadingConnector):
    """``OffloadingConnector`` whose layout request is ``requested_layout``."""

    @classmethod
    def get_required_kvcache_layout(cls, vllm_config) -> str | None:
        stock = None
        if _OffloadingConnector is not object:
            stock = _OffloadingConnector.get_required_kvcache_layout(vllm_config)
        return requested_layout(stock)
