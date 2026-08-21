# SPDX-License-Identifier: Apache-2.0
"""Tests for the vLLM compatibility shim (compat.py).

These guard the *visible feature->version mapping* against silent rot and
exercise every capability branch in the adapters without a real vLLM. Two kinds
of check:

  * **Matrix** — ``caps_for(v)`` / ``FEATURES`` are pure functions of the version
    tuple, so the whole matrix is asserted directly (no import juggling).
  * **Detection + adapters** — a few tests reload ``compat`` against a fake vLLM
    built for a specific version (via ``conftest.build_fake_vllm``) to prove the
    end-to-end detection path and the adapter shapes. These reload compat, so a
    fixture restores the 0.20 baseline afterward to keep other test modules'
    imported symbols valid.
"""

from __future__ import annotations

import importlib
import os
from dataclasses import dataclass

import pytest

import conftest
from certus_shmq_connector import compat


# ── matrix: pure, version-tuple-driven ──


def test_supported_versions_match_conftest():
    # The fake factory can't import compat (load order), so it keeps its own copy
    # of the supported list. If they drift, parametrized tests silently skip a
    # real version — assert they're identical.
    assert compat.SUPPORTED_VERSIONS == conftest.SUPPORTED_VERSIONS


@pytest.mark.parametrize("version", compat.SUPPORTED_VERSIONS)
def test_caps_for_every_supported_version(version):
    caps = compat.caps_for(version)
    # Features true across the whole supported range today. (transfer_result_has_type
    # is NOT here — the 0.26 rewrite dropped the field; pinned separately below.)
    assert caps.kv_caches_tensors_attr is True
    assert caps.kv_cache_group_attrs is True


@pytest.mark.parametrize(
    "version,expected",
    [
        ((0, 20), True),
        ((0, 22), True),
        ((0, 23), True),
        ((0, 24), True),
        ((0, 26), False),  # 0.26 rewrite: submit_store/submit_load carry direction
    ],
)
def test_transfer_result_has_type_dropped_at_0_26(version, expected):
    # TransferResult lost its 5th ``transfer_type`` field in the 0.26 API rewrite
    # (direction now lives in the method name). Pin the threshold so a matrix edit
    # that moves it is caught.
    assert compat.caps_for(version).transfer_result_has_type is expected


@pytest.mark.parametrize(
    "version,expected",
    [
        ((0, 20), False),
        ((0, 22), False),
        ((0, 23), False),
        ((0, 24), False),
        ((0, 26), True),
    ],
)
def test_new_0_26_capabilities_gate_from_0_26(version, expected):
    # The four capabilities the 0.26 offloading-API rewrite introduced, all keyed
    # to the same v>=(0,26) threshold. Pin each so a matrix edit is caught.
    caps = compat.caps_for(version)
    assert caps.worker_split_submit is expected
    assert caps.spec_config_object is expected
    assert caps.lookup_returns_enum is expected
    assert caps.canonical_kv_caches is expected


@pytest.mark.parametrize(
    "version,expected",
    [
        ((0, 20), False),  # 0.20 scheduler calls manager methods without req_context
        ((0, 22), True),   # req_context added to every OffloadingManager method
        ((0, 23), True),
        ((0, 24), True),
        ((0, 26), True),
    ],
)
def test_req_context_arg_from_0_22(version, expected):
    # req_context is passed by vLLM's scheduler starting at 0.22; the manager's
    # superset signature (req_context=None) absorbs both eras, so this flag is
    # declarative. Pin the threshold so a matrix edit that moves it is caught.
    assert compat.caps_for(version).req_context_arg is expected


@pytest.mark.parametrize(
    "version,expected",
    [
        ((0, 20), False),
        ((0, 22), False),
        ((0, 23), False),
        ((0, 24), False),
        ((0, 26), True),
    ],
)
def test_disable_hybrid_kv_cache_manager_only_from_0_26(version, expected):
    # This is the one capability that actually varies across the supported set;
    # pin it explicitly so a matrix edit that moves the threshold is caught.
    assert compat.caps_for(version).needs_disable_hybrid_kv_cache_manager is expected


@pytest.mark.parametrize(
    "version,expected",
    [
        ((0, 20), False),  # 0.20 did not default async scheduling on
        ((0, 22), True),   # 0.22+ auto-enables it; breaks OffloadingConnector serialization
        ((0, 23), True),
        ((0, 24), True),
        ((0, 26), True),
    ],
)
def test_disable_async_scheduling_from_0_22(version, expected):
    # 0.22 turns async scheduling on by default with no kv_connector guard, which
    # trips the offloading scheduler's transfer-serialization assert. Pin the
    # threshold so a matrix edit that moves it is caught.
    assert compat.caps_for(version).needs_disable_async_scheduling is expected


@pytest.mark.parametrize(
    "version,expected",
    [
        ((0, 20), False),
        ((0, 22), False),
        ((0, 23), True),   # 0.23 added the on_new_request abstract method
        ((0, 24), True),
        ((0, 26), True),
    ],
)
def test_on_new_request_from_0_23(version, expected):
    # 0.23 made OffloadingManager.on_new_request abstract; the manager must
    # implement it or vLLM can't instantiate it. (Measured: abstract at 0.23, not
    # 0.24.) Pin the threshold so a matrix edit that moves it is caught.
    assert compat.caps_for(version).has_on_new_request is expected


def test_features_and_caps_have_identical_keys():
    # Caps must expose exactly the FEATURES names — a new predicate without a
    # matching dataclass field (or vice versa) would raise at caps_for() time,
    # but assert it directly so the failure is a readable diff.
    caps_fields = set(compat.Caps.__dataclass_fields__)
    assert set(compat.FEATURES) == caps_fields


def test_render_matrix_lists_every_version_and_feature():
    text = compat.render_matrix()
    for v in compat.SUPPORTED_VERSIONS:
        assert f"{v[0]}.{v[1]}" in text
    for name in compat.FEATURES:
        assert name in text
    # Names the detected running version so operators can see what shim path is live.
    assert "detected running version" in text


# ── adapters: gpu_block_ids ──


def test_gpu_block_ids_coerces_to_plain_int_list():
    from vllm.v1.kv_offload.mediums import GPULoadStoreSpec

    spec = GPULoadStoreSpec(block_ids=[3, 7, 11])
    out = compat.gpu_block_ids(spec)
    assert out == [3, 7, 11]
    assert all(type(b) is int for b in out)  # not np.int64


# ── adapters: extract_gpu_ptrs guards the per-layer split (≤0.24 branch) ──


class _FakeTensor:
    """Just-enough torch.Tensor stand-in for extract_gpu_ptrs: a per-block
    ``stride(0)`` in elements, an element size, a data pointer, and a shape."""

    def __init__(self, stride0: int, elem: int = 1, ptr: int = 0x1000):
        self._stride0 = stride0
        self._elem = elem
        self._ptr = ptr
        self.shape = (2740, stride0)

    def stride(self, dim: int) -> int:
        assert dim == 0
        return self._stride0

    def element_size(self) -> int:
        return self._elem

    def data_ptr(self) -> int:
        return self._ptr


@dataclass
class _FakeCanonicalTensor:
    tensor: object
    page_size_bytes: int


class _FakeKVCaches:
    def __init__(self, tensors: list):
        self.tensors = tensors
        self.group_data_refs = [object()]


def test_extract_gpu_ptrs_single_tensor_returns_one_region():
    # 0.20/0.22 present ONE coalesced tensor (all layers). extract_gpu_ptrs
    # returns a LIST regardless of version; the single-tensor case is just N==1,
    # the one region the connector's one-region-per-key model started with.
    kv = _FakeKVCaches(
        [_FakeCanonicalTensor(_FakeTensor(2097152, ptr=0xDEAD000), 2097152)]
    )
    regions = compat.extract_gpu_ptrs(kv)
    assert regions == [(0xDEAD000, 2097152)]


def test_extract_gpu_ptrs_returns_one_region_per_layer_tensor():
    # 0.23+ split the KV cache into per-layer tensors (Llama-3: 32). Multi-region
    # offload maps each to its own IPC region colocated in one slot, so this
    # returns a 32-element list (one (ptr, stride) per tensor) instead of the
    # pre-multiregion guard that refused a split (see multi-region-kv-offload.md).
    kv = _FakeKVCaches(
        [_FakeCanonicalTensor(_FakeTensor(65536), 65536) for _ in range(32)]
    )
    regions = compat.extract_gpu_ptrs(kv)
    assert len(regions) == 32
    assert regions == [(0x1000, 65536)] * 32


# ── adapters: make_transfer_result, both CAPS branches ──


def test_make_transfer_result_includes_type_when_capability_set():
    # Real supported matrix: transfer_result_has_type is True, so all 5 fields.
    r = compat.make_transfer_result(
        job_id=1, success=True, transfer_size=1024, transfer_time=0.5, transfer_type=("gpu", "certus")
    )
    assert r.job_id == 1
    assert r.success is True
    assert r.transfer_size == 1024
    assert r.transfer_time == 0.5
    assert r.transfer_type == ("gpu", "certus")


def test_make_transfer_result_omits_type_when_capability_unset(monkeypatch):
    # Defensive future branch: a version that drops transfer_type. Monkeypatch the
    # capability AND point TransferResult at a 4-field dataclass so the adapter is
    # exercised against a matching shape (no real supported version does this yet).
    from dataclasses import make_dataclass

    four_field = make_dataclass(
        "TransferResult",
        [("job_id", int), ("success", bool), ("transfer_size", int), ("transfer_time", float)],
    )
    # TransferResult is resolved lazily through the _VLLM_SYMBOLS cache, so point
    # the cache entry (not a module global) at the 4-field shape.
    compat._load_vllm_symbols()  # populate the cache from the installed fake
    monkeypatch.setitem(compat._VLLM_SYMBOLS, "TransferResult", four_field)
    # Caps is a frozen dataclass, so build a fresh one with the flag cleared and
    # swap the whole CAPS object (can't mutate a field in place).
    patched = compat.caps_for((0, 20))
    object.__setattr__(patched, "transfer_result_has_type", False)
    monkeypatch.setattr(compat, "CAPS", patched)

    r = compat.make_transfer_result(
        job_id=2, success=False, transfer_size=0, transfer_time=0.0, transfer_type=("x", "y")
    )
    assert r.job_id == 2
    assert r.success is False
    assert not hasattr(r, "transfer_type")


# ── end-to-end version detection via reload ──


@pytest.fixture
def restore_compat():
    """Rebuild the 0.20 baseline fakes and reload compat after a test that
    reloaded it against a different version, so modules that imported compat
    symbols at collection time keep valid references.

    Clears ``CERTUS_VLLM_VERSION`` FIRST: this fixture's teardown runs before the
    ``monkeypatch`` fixture's (teardown is reverse-of-setup order), so the env
    override set by test_env_override_beats_installed_version would otherwise
    still be live and make this reload re-detect 0.26 — leaving CAPS stuck at
    0.26 and breaking every later test that touches a 0.26-only symbol."""
    yield
    os.environ.pop("CERTUS_VLLM_VERSION", None)
    conftest.build_fake_vllm((0, 20))
    importlib.reload(compat)


@pytest.mark.parametrize("version", compat.SUPPORTED_VERSIONS)
def test_detect_version_end_to_end(version, monkeypatch, restore_compat):
    # No env override — exercise the module-__version__ detection path against a
    # fake built for this version.
    monkeypatch.delenv("CERTUS_VLLM_VERSION", raising=False)
    conftest.build_fake_vllm(version)
    reloaded = importlib.reload(compat)
    assert reloaded.VERSION == version
    assert reloaded.CAPS == reloaded.caps_for(version)
    assert (
        reloaded.CAPS.needs_disable_hybrid_kv_cache_manager is (version >= (0, 26))
    )


def test_env_override_beats_installed_version(monkeypatch, restore_compat):
    conftest.build_fake_vllm((0, 20))
    monkeypatch.setenv("CERTUS_VLLM_VERSION", "0.26.1")
    reloaded = importlib.reload(compat)
    assert reloaded.VERSION == (0, 26)
    assert reloaded.CAPS.needs_disable_hybrid_kv_cache_manager is True
