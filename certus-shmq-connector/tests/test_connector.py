# SPDX-License-Identifier: Apache-2.0
"""CertusConnector's KV cache layout request."""

import importlib

import pytest

from certus_shmq_connector import compat


@pytest.fixture
def connector(monkeypatch):
    monkeypatch.delenv("CERTUS_KV_CACHE_LAYOUT", raising=False)
    mod = importlib.import_module("certus_shmq_connector.connector")
    return importlib.reload(mod)


@pytest.mark.parametrize(
    "version,expected",
    [((0, 26), False), ((0, 29), True), ((0, 30), True)],
)
def test_kvcache_layout_enum_from_0_29(version, expected):
    # 0.29 (PR #51718) introduced the model-wide KVCacheLayout names.
    assert compat.caps_for(version).kvcache_layout_enum is expected


def test_requests_the_stock_layout_by_default(connector):
    # The stock request on every vLLM: LBHNC on 0.29+ (NixlConnector's too),
    # BLHNC under hisparse, legacy names (HND/NHD) or None before 0.29.
    for stock in ("LBHNC", "BLHNC", "LBNHC", "HND", None):
        assert connector.requested_layout(stock) == stock


def test_env_opts_into_another_layout(monkeypatch, connector):
    monkeypatch.setenv("CERTUS_KV_CACHE_LAYOUT", "BLHNC")
    assert connector.requested_layout("LBHNC") == "BLHNC"
    monkeypatch.setenv("CERTUS_KV_CACHE_LAYOUT", " BLHNC ")
    assert connector.requested_layout("LBHNC") == "BLHNC"
    monkeypatch.setenv("CERTUS_KV_CACHE_LAYOUT", "")
    assert connector.requested_layout("LBHNC") == "LBHNC"   # empty: stock request


@pytest.mark.parametrize("env,expected", [(None, "LBHNC"), ("BLHNC", "BLHNC")])
def test_classmethod_wraps_stock(monkeypatch, connector, env, expected):
    if env is not None:
        monkeypatch.setenv("CERTUS_KV_CACHE_LAYOUT", env)

    class Stock:
        @classmethod
        def get_required_kvcache_layout(cls, vllm_config):
            return "LBHNC"

    monkeypatch.setattr(connector, "_OffloadingConnector", Stock)
    assert connector.CertusConnector.get_required_kvcache_layout(None) == expected
