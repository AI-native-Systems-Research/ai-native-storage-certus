# SPDX-License-Identifier: Apache-2.0
"""OffloadingHandlers for Certus: delegate DMA transfers to CertusEngine.

The handlers extract keys from CertusLoadStoreSpec and call CertusEngine's
store_async/load_async methods. The dispatcher inside CertusEngine resolves
block locations internally — handlers don't need to pass slab/slot addresses.
"""

from __future__ import annotations

import time
from collections import deque
from dataclasses import dataclass
from typing import Any

from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
from vllm.v1.kv_offload.worker.worker import (
    OffloadingHandler,
    TransferResult,
    TransferSpec,
    TransferType,
)

from certus_connector.mediums import CertusLoadStoreSpec


# ── Mock engine for testing without SPDK/CUDA ──


class MockCertusEngine:
    """In-memory mock matching CertusEngine's interface. No hardware needed."""

    def store_async(self, job_id: int, gpu_block_ids: list[int], keys: list[int]) -> bool:
        return True

    def load_async(self, job_id: int, gpu_block_ids: list[int], keys: list[int]) -> bool:
        return True

    def poll_completions(self) -> list[tuple[int, bool]]:
        return []

    def wait_job(self, job_id: int) -> None:
        pass

    def shutdown(self) -> None:
        pass


# ── Handler implementations ──


@dataclass
class PendingJob:
    job_id: int
    start_time: float
    num_blocks: int
    transfer_type: TransferType


class GpuToCertusHandler(OffloadingHandler):
    """Store: GPU → pinned CPU → NVMe (+ DRAM residency)."""

    def __init__(self, engine: Any, block_size_bytes: int):
        self._engine = engine
        self._block_size_bytes = block_size_bytes
        self._pending: deque[PendingJob] = deque()
        self._transfer_type: TransferType = ("GPU", "Certus")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, GPULoadStoreSpec)
        assert isinstance(dst_spec, CertusLoadStoreSpec)

        gpu_block_ids = list(src_spec.block_ids)
        keys = [loc.nvme_slab for loc in dst_spec.locations]

        success = self._engine.store_async(job_id, gpu_block_ids, keys)
        if success:
            self._pending.append(PendingJob(
                job_id=job_id,
                start_time=time.monotonic(),
                num_blocks=len(gpu_block_ids),
                transfer_type=self._transfer_type,
            ))
        return success

    def get_finished(self) -> list[TransferResult]:
        results: list[TransferResult] = []
        completions = {jid: ok for jid, ok in self._engine.poll_completions()}
        now = time.monotonic()
        while self._pending and self._pending[0].job_id in completions:
            job = self._pending.popleft()
            success = completions.pop(job.job_id)
            results.append(TransferResult(
                job_id=job.job_id,
                success=success,
                transfer_size=job.num_blocks * self._block_size_bytes,
                transfer_time=now - job.start_time,
                transfer_type=job.transfer_type,
            ))
        return results

    def wait(self, job_ids: set[int]) -> None:
        for jid in job_ids:
            self._engine.wait_job(jid)


class CertusToGpuHandler(OffloadingHandler):
    """Load: DRAM→GPU (fast) or NVMe→CPU→GPU (cache miss)."""

    def __init__(self, engine: Any, block_size_bytes: int):
        self._engine = engine
        self._block_size_bytes = block_size_bytes
        self._pending: deque[PendingJob] = deque()
        self._transfer_type: TransferType = ("Certus", "GPU")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, CertusLoadStoreSpec)
        assert isinstance(dst_spec, GPULoadStoreSpec)

        gpu_block_ids = list(dst_spec.block_ids)
        keys = [loc.nvme_slab for loc in src_spec.locations]

        success = self._engine.load_async(job_id, gpu_block_ids, keys)
        if success:
            self._pending.append(PendingJob(
                job_id=job_id,
                start_time=time.monotonic(),
                num_blocks=len(gpu_block_ids),
                transfer_type=self._transfer_type,
            ))
        return success

    def get_finished(self) -> list[TransferResult]:
        results: list[TransferResult] = []
        completions = {jid: ok for jid, ok in self._engine.poll_completions()}
        now = time.monotonic()
        while self._pending and self._pending[0].job_id in completions:
            job = self._pending.popleft()
            success = completions.pop(job.job_id)
            results.append(TransferResult(
                job_id=job.job_id,
                success=success,
                transfer_size=job.num_blocks * self._block_size_bytes,
                transfer_time=now - job.start_time,
                transfer_type=job.transfer_type,
            ))
        return results

    def wait(self, job_ids: set[int]) -> None:
        for jid in job_ids:
            self._engine.wait_job(jid)
