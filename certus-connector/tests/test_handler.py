# SPDX-License-Identifier: Apache-2.0
"""Unit tests for GpuToCertusHandler and CertusToGpuHandler.

Runs WITHOUT vllm/torch/SPDK/CUDA — uses mocks throughout.
Run with:  pytest certus-connector/tests/test_handler.py -v
"""

from __future__ import annotations

import sys
import types
from collections import deque
from dataclasses import dataclass
from typing import Any
from unittest.mock import MagicMock, patch

import pytest

# ── Stub vllm modules ─────────────────────────────────────────────────────

_mock_modules = {}
for _mod in [
    "vllm", "vllm.v1", "vllm.v1.kv_offload", "vllm.v1.kv_offload.abstract",
    "vllm.v1.kv_offload.mediums", "vllm.v1.kv_offload.worker",
    "vllm.v1.kv_offload.worker.worker",
    "vllm.v1.kv_cache_interface", "vllm.v1.attention",
    "vllm.v1.attention.backend", "vllm.config", "vllm.logger",
    "vllm.v1.core", "vllm.v1.core.kv_cache_utils",
]:
    _mock_modules[_mod] = types.ModuleType(_mod)
    sys.modules[_mod] = _mock_modules[_mod]


# Minimal stubs for the types handler.py imports from vllm.
TransferType = tuple  # ("GPU", "Certus") or ("Certus", "GPU")


@dataclass
class TransferResult:
    job_id: int
    success: bool
    transfer_size: int
    transfer_time: float
    transfer_type: TransferType


@dataclass
class GPULoadStoreSpec:
    block_ids: list[int]

    @staticmethod
    def medium() -> str:
        return "GPU"


class OffloadingHandler:
    def transfer_async(self, job_id: int, spec: Any) -> bool: ...
    def get_finished(self) -> list[TransferResult]: ...
    def wait(self, job_ids: set[int]) -> None: ...


class LoadStoreSpec:
    """Minimal stub — mediums.CertusLoadStoreSpec inherits from this."""
    @staticmethod
    def medium() -> str:
        return ""


sys.modules["vllm.v1.kv_offload.abstract"].LoadStoreSpec = LoadStoreSpec
sys.modules["vllm.v1.kv_offload.mediums"].GPULoadStoreSpec = GPULoadStoreSpec
sys.modules["vllm.v1.kv_offload.worker.worker"].OffloadingHandler = OffloadingHandler
sys.modules["vllm.v1.kv_offload.worker.worker"].TransferResult = TransferResult
sys.modules["vllm.v1.kv_offload.worker.worker"].TransferSpec = tuple
sys.modules["vllm.v1.kv_offload.worker.worker"].TransferType = TransferType

from certus_connector.mediums import BlockLocation, CertusLoadStoreSpec  # noqa: E402
from certus_connector.handler import (  # noqa: E402
    GpuToCertusHandler,
    CertusToGpuHandler,
    MockCertusEngine,
    PendingJob,
)


# ── Helpers ────────────────────────────────────────────────────────────────

BLOCK_SIZE = 131_072  # 128 KiB


def make_gpu_spec(block_ids: list[int]) -> GPULoadStoreSpec:
    return GPULoadStoreSpec(block_ids=block_ids)


def make_certus_spec(nvme_slabs: list[int]) -> CertusLoadStoreSpec:
    locs = [BlockLocation(nvme_slab=s) for s in nvme_slabs]
    return CertusLoadStoreSpec(locs)


# ── MockCertusEngine tests ─────────────────────────────────────────────────

class TestMockCertusEngine:
    def test_store_async_returns_true(self):
        eng = MockCertusEngine()
        assert eng.store_async(1, [0, 1], [10, 11]) is True

    def test_load_async_returns_true(self):
        eng = MockCertusEngine()
        assert eng.load_async(2, [0], [10]) is True

    def test_poll_completions_empty(self):
        eng = MockCertusEngine()
        assert eng.poll_completions() == []

    def test_wait_job_noop(self):
        eng = MockCertusEngine()
        eng.wait_job(99)  # must not raise

    def test_shutdown_noop(self):
        eng = MockCertusEngine()
        eng.shutdown()  # must not raise


# ── GpuToCertusHandler tests ───────────────────────────────────────────────

class TestGpuToCertusHandler:
    @pytest.fixture
    def engine(self):
        return MockCertusEngine()

    @pytest.fixture
    def handler(self, engine):
        return GpuToCertusHandler(engine, BLOCK_SIZE)

    def test_transfer_async_success_enqueues_job(self, handler):
        spec = (make_gpu_spec([0, 1]), make_certus_spec([10, 11]))
        result = handler.transfer_async(job_id=1, spec=spec)
        assert result is True
        assert len(handler._pending) == 1
        assert handler._pending[0].job_id == 1
        assert handler._pending[0].num_blocks == 2

    def test_transfer_async_passes_correct_keys_to_engine(self, engine):
        recorded = {}

        def capturing_store(job_id, gpu_block_ids, keys):
            recorded["job_id"] = job_id
            recorded["gpu_block_ids"] = gpu_block_ids
            recorded["keys"] = keys
            return True

        engine.store_async = capturing_store
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)
        spec = (make_gpu_spec([5, 6, 7]), make_certus_spec([100, 101, 102]))
        handler.transfer_async(job_id=42, spec=spec)

        assert recorded["job_id"] == 42
        assert recorded["gpu_block_ids"] == [5, 6, 7]
        assert recorded["keys"] == [100, 101, 102]

    def test_transfer_async_engine_failure_does_not_enqueue(self, engine, handler):
        engine.store_async = lambda *a, **k: False
        spec = (make_gpu_spec([0]), make_certus_spec([10]))
        result = handler.transfer_async(job_id=1, spec=spec)
        assert result is False
        assert len(handler._pending) == 0

    def test_get_finished_returns_empty_when_no_completions(self, handler):
        spec = (make_gpu_spec([0]), make_certus_spec([10]))
        handler.transfer_async(job_id=1, spec=spec)
        assert handler.get_finished() == []

    def test_get_finished_returns_result_on_completion(self, engine):
        engine.poll_completions = lambda: [(1, True)]
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)
        spec = (make_gpu_spec([0, 1]), make_certus_spec([10, 11]))
        handler.transfer_async(job_id=1, spec=spec)

        results = handler.get_finished()
        assert len(results) == 1
        r = results[0]
        assert r.job_id == 1
        assert r.success is True
        assert r.transfer_size == 2 * BLOCK_SIZE
        assert r.transfer_time >= 0.0
        assert r.transfer_type == ("GPU", "Certus")

    def test_get_finished_reports_failure_result(self, engine):
        engine.poll_completions = lambda: [(1, False)]
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)
        spec = (make_gpu_spec([0]), make_certus_spec([10]))
        handler.transfer_async(job_id=1, spec=spec)

        results = handler.get_finished()
        assert len(results) == 1
        assert results[0].success is False

    def test_get_finished_only_pops_completed_head(self, engine):
        """Only jobs at the front of the deque whose completion is known are returned."""
        completions_store = []
        engine.poll_completions = lambda: list(completions_store)
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)

        for job_id in [1, 2, 3]:
            handler.transfer_async(job_id, (make_gpu_spec([job_id]), make_certus_spec([job_id * 10])))

        # Only job 1 completed — 2 and 3 still pending.
        completions_store[:] = [(1, True)]
        results = handler.get_finished()
        assert len(results) == 1
        assert results[0].job_id == 1
        assert len(handler._pending) == 2

        # Now job 2 completes.
        completions_store[:] = [(2, True)]
        results = handler.get_finished()
        assert len(results) == 1
        assert results[0].job_id == 2

    def test_get_finished_does_not_pop_if_head_incomplete(self, engine):
        """Jobs 2 and 3 complete, but job 1 is still pending — nothing returned."""
        completions_store = [(2, True), (3, True)]
        engine.poll_completions = lambda: list(completions_store)
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)

        for job_id in [1, 2, 3]:
            handler.transfer_async(job_id, (make_gpu_spec([job_id]), make_certus_spec([job_id * 10])))

        results = handler.get_finished()
        # job 1 (head) has no completion, so nothing is popped.
        assert results == []
        assert len(handler._pending) == 3

    def test_multiple_jobs_complete_in_order(self, engine):
        completions_store = [(1, True), (2, False), (3, True)]
        engine.poll_completions = lambda: list(completions_store)
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)

        for job_id in [1, 2, 3]:
            handler.transfer_async(job_id, (make_gpu_spec([job_id]), make_certus_spec([job_id * 10])))

        results = handler.get_finished()
        assert len(results) == 3
        assert [r.job_id for r in results] == [1, 2, 3]
        assert [r.success for r in results] == [True, False, True]
        assert len(handler._pending) == 0

    def test_get_finished_clears_pending_deque(self, engine):
        engine.poll_completions = lambda: [(1, True), (2, True)]
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)
        for jid in [1, 2]:
            handler.transfer_async(jid, (make_gpu_spec([jid]), make_certus_spec([jid * 10])))
        handler.get_finished()
        assert len(handler._pending) == 0

    def test_wait_delegates_to_engine(self, engine):
        waited = []
        engine.wait_job = lambda jid: waited.append(jid)
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)
        handler.wait({10, 20})
        assert set(waited) == {10, 20}

    def test_transfer_type_label(self, handler):
        assert handler._transfer_type == ("GPU", "Certus")

    def test_transfer_size_scales_with_block_count(self, engine):
        engine.poll_completions = lambda: [(1, True)]
        handler = GpuToCertusHandler(engine, BLOCK_SIZE)
        spec = (make_gpu_spec(list(range(5))), make_certus_spec(list(range(5))))
        handler.transfer_async(job_id=1, spec=spec)
        results = handler.get_finished()
        assert results[0].transfer_size == 5 * BLOCK_SIZE

    def test_get_finished_empty_pending_returns_empty(self, handler):
        assert handler.get_finished() == []


# ── CertusToGpuHandler tests ───────────────────────────────────────────────

class TestCertusToGpuHandler:
    @pytest.fixture
    def engine(self):
        return MockCertusEngine()

    @pytest.fixture
    def handler(self, engine):
        return CertusToGpuHandler(engine, BLOCK_SIZE)

    def test_transfer_async_success_enqueues_job(self, handler):
        spec = (make_certus_spec([10, 11]), make_gpu_spec([0, 1]))
        result = handler.transfer_async(job_id=1, spec=spec)
        assert result is True
        assert len(handler._pending) == 1
        assert handler._pending[0].job_id == 1

    def test_transfer_async_passes_correct_keys_to_engine(self, engine):
        recorded = {}

        def capturing_load(job_id, gpu_block_ids, keys):
            recorded["job_id"] = job_id
            recorded["gpu_block_ids"] = gpu_block_ids
            recorded["keys"] = keys
            return True

        engine.load_async = capturing_load
        handler = CertusToGpuHandler(engine, BLOCK_SIZE)
        spec = (make_certus_spec([200, 201]), make_gpu_spec([8, 9]))
        handler.transfer_async(job_id=7, spec=spec)

        assert recorded["job_id"] == 7
        assert recorded["gpu_block_ids"] == [8, 9]
        assert recorded["keys"] == [200, 201]

    def test_transfer_async_engine_failure_does_not_enqueue(self, engine, handler):
        engine.load_async = lambda *a, **k: False
        spec = (make_certus_spec([10]), make_gpu_spec([0]))
        result = handler.transfer_async(job_id=1, spec=spec)
        assert result is False
        assert len(handler._pending) == 0

    def test_get_finished_returns_empty_when_no_completions(self, handler):
        spec = (make_certus_spec([10]), make_gpu_spec([0]))
        handler.transfer_async(job_id=1, spec=spec)
        assert handler.get_finished() == []

    def test_get_finished_returns_result_on_completion(self, engine):
        engine.poll_completions = lambda: [(1, True)]
        handler = CertusToGpuHandler(engine, BLOCK_SIZE)
        spec = (make_certus_spec([10, 11]), make_gpu_spec([0, 1]))
        handler.transfer_async(job_id=1, spec=spec)

        results = handler.get_finished()
        assert len(results) == 1
        r = results[0]
        assert r.job_id == 1
        assert r.success is True
        assert r.transfer_size == 2 * BLOCK_SIZE
        assert r.transfer_time >= 0.0
        assert r.transfer_type == ("Certus", "GPU")

    def test_get_finished_reports_failure_result(self, engine):
        engine.poll_completions = lambda: [(1, False)]
        handler = CertusToGpuHandler(engine, BLOCK_SIZE)
        spec = (make_certus_spec([10]), make_gpu_spec([0]))
        handler.transfer_async(job_id=1, spec=spec)

        results = handler.get_finished()
        assert results[0].success is False

    def test_get_finished_only_pops_completed_head(self, engine):
        completions_store = []
        engine.poll_completions = lambda: list(completions_store)
        handler = CertusToGpuHandler(engine, BLOCK_SIZE)

        for job_id in [1, 2, 3]:
            handler.transfer_async(job_id, (make_certus_spec([job_id * 10]), make_gpu_spec([job_id])))

        completions_store[:] = [(1, True)]
        results = handler.get_finished()
        assert len(results) == 1
        assert results[0].job_id == 1
        assert len(handler._pending) == 2

    def test_get_finished_does_not_pop_if_head_incomplete(self, engine):
        completions_store = [(2, True), (3, True)]
        engine.poll_completions = lambda: list(completions_store)
        handler = CertusToGpuHandler(engine, BLOCK_SIZE)

        for job_id in [1, 2, 3]:
            handler.transfer_async(job_id, (make_certus_spec([job_id * 10]), make_gpu_spec([job_id])))

        results = handler.get_finished()
        assert results == []

    def test_multiple_jobs_complete_in_order(self, engine):
        completions_store = [(1, True), (2, False), (3, True)]
        engine.poll_completions = lambda: list(completions_store)
        handler = CertusToGpuHandler(engine, BLOCK_SIZE)

        for job_id in [1, 2, 3]:
            handler.transfer_async(job_id, (make_certus_spec([job_id * 10]), make_gpu_spec([job_id])))

        results = handler.get_finished()
        assert len(results) == 3
        assert [r.job_id for r in results] == [1, 2, 3]
        assert [r.success for r in results] == [True, False, True]

    def test_wait_delegates_to_engine(self, engine):
        waited = []
        engine.wait_job = lambda jid: waited.append(jid)
        handler = CertusToGpuHandler(engine, BLOCK_SIZE)
        handler.wait({5, 6})
        assert set(waited) == {5, 6}

    def test_transfer_type_label(self, handler):
        assert handler._transfer_type == ("Certus", "GPU")

    def test_transfer_size_scales_with_block_count(self, engine):
        engine.poll_completions = lambda: [(1, True)]
        handler = CertusToGpuHandler(engine, BLOCK_SIZE)
        spec = (make_certus_spec(list(range(4))), make_gpu_spec(list(range(4))))
        handler.transfer_async(job_id=1, spec=spec)
        results = handler.get_finished()
        assert results[0].transfer_size == 4 * BLOCK_SIZE

    def test_get_finished_empty_pending_returns_empty(self, handler):
        assert handler.get_finished() == []


# ── Cross-handler interaction tests ───────────────────────────────────────

class TestHandlerInterleaved:
    """Simulate store followed by load — the common hot path."""

    def test_store_then_load_with_independent_engines(self):
        """Store and load handlers share a key namespace but different engines."""
        store_completions = [(1, True)]
        load_completions = [(2, True)]

        store_engine = MockCertusEngine()
        store_engine.poll_completions = lambda: list(store_completions)
        load_engine = MockCertusEngine()
        load_engine.poll_completions = lambda: list(load_completions)

        store_handler = GpuToCertusHandler(store_engine, BLOCK_SIZE)
        load_handler = CertusToGpuHandler(load_engine, BLOCK_SIZE)

        store_spec = (make_gpu_spec([0, 1]), make_certus_spec([100, 101]))
        store_handler.transfer_async(1, store_spec)

        load_spec = (make_certus_spec([100, 101]), make_gpu_spec([0, 1]))
        load_handler.transfer_async(2, load_spec)

        store_results = store_handler.get_finished()
        load_results = load_handler.get_finished()

        assert len(store_results) == 1
        assert store_results[0].transfer_type == ("GPU", "Certus")
        assert len(load_results) == 1
        assert load_results[0].transfer_type == ("Certus", "GPU")

    def test_interleaved_jobs_do_not_cross_contaminate(self):
        """Two handlers using the same engine: pending deques are independent."""
        engine = MockCertusEngine()
        store_completions: list[tuple[int, bool]] = []
        engine.poll_completions = lambda: list(store_completions)

        store_handler = GpuToCertusHandler(engine, BLOCK_SIZE)
        load_handler = CertusToGpuHandler(engine, BLOCK_SIZE)

        store_handler.transfer_async(1, (make_gpu_spec([0]), make_certus_spec([10])))
        load_handler.transfer_async(2, (make_certus_spec([10]), make_gpu_spec([0])))

        store_completions[:] = [(1, True), (2, True)]

        store_results = store_handler.get_finished()
        load_results = load_handler.get_finished()

        assert len(store_results) == 1
        assert store_results[0].job_id == 1
        assert len(load_results) == 1
        assert load_results[0].job_id == 2
