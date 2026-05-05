# SPDX-License-Identifier: Apache-2.0
"""OffloadingHandler for Certus: delegates to a native transfer engine.

The handler wraps a CertusTransferEngine (Rust/PyO3 in production, mock for
testing) that performs:
- Store: CUDA DMA GPU→pinned CPU, then SPDK write to NVMe slab
- Load (NVMe): SPDK read to pinned CPU, then CUDA DMA to GPU
- Load (DRAM): CUDA DMA from pinned CPU slot directly to GPU

The engine interface is defined here so the Rust implementation can
match it exactly via PyO3.
"""

from __future__ import annotations

import time
from abc import ABC, abstractmethod
from collections import deque
from dataclasses import dataclass

from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
from vllm.v1.kv_offload.worker.worker import (
    OffloadingHandler,
    TransferResult,
    TransferSpec,
    TransferType,
)

from certus_connector.mediums import CertusLoadStoreSpec


# ── Engine interface (implemented in Rust via PyO3 for production) ──


class CertusTransferEngine(ABC):
    """Interface for the native SPDK + CUDA DMA engine.

    Production: Rust implementation via PyO3 (certus_native).
    Testing: MockCertusTransferEngine below.
    """

    @abstractmethod
    def store_async(
        self,
        job_id: int,
        gpu_block_ids: list[int],
        nvme_slabs: list[int],
        dram_slots: list[int | None],
    ) -> bool:
        """GPU→pinned CPU→NVMe. If dram_slot is set, data stays in DRAM too."""
        ...

    @abstractmethod
    def load_async(
        self,
        job_id: int,
        gpu_block_ids: list[int],
        nvme_slabs: list[int],
        dram_slots: list[int | None],
    ) -> bool:
        """NVMe/DRAM→GPU. Uses dram_slot if available (fast path)."""
        ...

    @abstractmethod
    def poll_completions(self) -> list[tuple[int, bool]]:
        """Return list of (job_id, success) for finished transfers."""
        ...

    @abstractmethod
    def wait_job(self, job_id: int) -> None:
        """Block until job completes."""
        ...

    @abstractmethod
    def promote_async(self, job_id: int, nvme_slab: int, dram_slot: int) -> bool:
        """Background NVMe→DRAM copy (no GPU involved)."""
        ...

    @abstractmethod
    def shutdown(self) -> None:
        """Release SPDK resources, pinned buffers, CUDA contexts."""
        ...


# ── Mock engine for testing without SPDK/CUDA ──


class MockCertusTransferEngine(CertusTransferEngine):
    """In-memory mock that simulates store/load without hardware."""

    def __init__(self):
        self._completed: deque[tuple[int, bool]] = deque()
        self._pending: set[int] = set()
        # Simulated storage: slab_id → bytes
        self._nvme_data: dict[int, bytes] = {}
        # Simulated DRAM: slot_id → bytes
        self._dram_data: dict[int, bytes] = {}

    def store_async(self, job_id, gpu_block_ids, nvme_slabs, dram_slots):
        for i, slab in enumerate(nvme_slabs):
            data = f"block_{gpu_block_ids[i]}".encode()
            self._nvme_data[slab] = data
            if dram_slots[i] is not None:
                self._dram_data[dram_slots[i]] = data
        self._completed.append((job_id, True))
        return True

    def load_async(self, job_id, gpu_block_ids, nvme_slabs, dram_slots):
        self._completed.append((job_id, True))
        return True

    def poll_completions(self):
        results = list(self._completed)
        self._completed.clear()
        return results

    def wait_job(self, job_id):
        pass

    def promote_async(self, job_id, nvme_slab, dram_slot):
        data = self._nvme_data.get(nvme_slab, b"")
        self._dram_data[dram_slot] = data
        self._completed.append((job_id, True))
        return True

    def shutdown(self):
        self._nvme_data.clear()
        self._dram_data.clear()


# ── Handler implementations ──


@dataclass
class PendingJob:
    job_id: int
    start_time: float
    num_blocks: int
    transfer_type: TransferType


class GpuToCertusHandler(OffloadingHandler):
    """Store: GPU → pinned CPU → NVMe (+ DRAM residency)."""

    def __init__(self, engine: CertusTransferEngine, block_size_bytes: int):
        self._engine = engine
        self._block_size_bytes = block_size_bytes
        self._pending: deque[PendingJob] = deque()
        self._transfer_type: TransferType = ("GPU", "Certus")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, GPULoadStoreSpec)
        assert isinstance(dst_spec, CertusLoadStoreSpec)

        gpu_block_ids = list(src_spec.block_ids)
        nvme_slabs = [loc.nvme_slab for loc in dst_spec.locations]
        dram_slots = [loc.dram_slot for loc in dst_spec.locations]

        success = self._engine.store_async(job_id, gpu_block_ids, nvme_slabs, dram_slots)
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
        completed_ids = {jid for jid, _ in self._engine.poll_completions()}
        now = time.monotonic()
        while self._pending and self._pending[0].job_id in completed_ids:
            job = self._pending.popleft()
            completed_ids.discard(job.job_id)
            results.append(TransferResult(
                job_id=job.job_id,
                success=True,
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

    def __init__(self, engine: CertusTransferEngine, block_size_bytes: int):
        self._engine = engine
        self._block_size_bytes = block_size_bytes
        self._pending: deque[PendingJob] = deque()
        self._transfer_type: TransferType = ("Certus", "GPU")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, CertusLoadStoreSpec)
        assert isinstance(dst_spec, GPULoadStoreSpec)

        gpu_block_ids = list(dst_spec.block_ids)
        nvme_slabs = [loc.nvme_slab for loc in src_spec.locations]
        dram_slots = [loc.dram_slot for loc in src_spec.locations]

        success = self._engine.load_async(job_id, gpu_block_ids, nvme_slabs, dram_slots)
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
        completed_ids = {jid for jid, _ in self._engine.poll_completions()}
        now = time.monotonic()
        while self._pending and self._pending[0].job_id in completed_ids:
            job = self._pending.popleft()
            completed_ids.discard(job.job_id)
            results.append(TransferResult(
                job_id=job.job_id,
                success=True,
                transfer_size=job.num_blocks * self._block_size_bytes,
                transfer_time=now - job.start_time,
                transfer_type=job.transfer_type,
            ))
        return results

    def wait(self, job_ids: set[int]) -> None:
        for jid in job_ids:
            self._engine.wait_job(jid)
