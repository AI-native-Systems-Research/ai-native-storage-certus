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


def _caps(monkeypatch, connector, version):
    monkeypatch.setattr(connector, "CAPS", compat.caps_for(version))


@pytest.mark.parametrize(
    "version,expected",
    [((0, 26), False), ((0, 29), True), ((0, 30), True)],
)
def test_kvcache_layout_enum_from_0_29(version, expected):
    # 0.29 (PR #51718) introduced the model-wide KVCacheLayout names.
    assert compat.caps_for(version).kvcache_layout_enum is expected


def test_requests_blhnc_over_stock_lbhnc(monkeypatch, connector):
    _caps(monkeypatch, connector, (0, 30))
    assert connector.requested_layout("LBHNC") == "BLHNC"
    assert connector.requested_layout(None) == "BLHNC"


def test_keeps_other_stock_requests(monkeypatch, connector):
    # hisparse makes the stock connector ask for BLHNC itself; any other
    # deliberate stock choice is not overridden.
    _caps(monkeypatch, connector, (0, 30))
    assert connector.requested_layout("BLHNC") == "BLHNC"
    assert connector.requested_layout("LBNHC") == "LBNHC"


def test_defers_to_stock_before_0_29(monkeypatch, connector):
    # Before 0.29 the hook took legacy names (HND/NHD); never send BLHNC there.
    _caps(monkeypatch, connector, (0, 26))
    assert connector.requested_layout("HND") == "HND"
    assert connector.requested_layout(None) is None


def test_env_override(monkeypatch, connector):
    _caps(monkeypatch, connector, (0, 30))
    monkeypatch.setenv("CERTUS_KV_CACHE_LAYOUT", "LBHNC")
    assert connector.requested_layout("LBHNC") == "LBHNC"
    monkeypatch.setenv("CERTUS_KV_CACHE_LAYOUT", "")
    assert connector.requested_layout("LBHNC") == "LBHNC"   # empty: stock request


def test_classmethod_wraps_stock(monkeypatch, connector):
    _caps(monkeypatch, connector, (0, 30))

    class Stock:
        @classmethod
        def get_required_kvcache_layout(cls, vllm_config):
            return "LBHNC"

    monkeypatch.setattr(connector, "_OffloadingConnector", Stock)
    assert connector.CertusConnector.get_required_kvcache_layout(None) == "BLHNC"
