# SPDX-License-Identifier: Apache-2.0
"""OffloadingHandlers for Certus NVMe offloading.

Handlers call non-blocking store_async/load_async directly (DMA issued
without waiting). A shared CompletionDispatcher routes poll_completions()
results to the correct handler.
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

from certus_connector._instrument import COUNTERS
from certus_connector.mediums import CertusLoadStoreSpec


# ── Completion routing ──


class CompletionDispatcher:
    """Routes poll_completions() results to the handler that owns each job.

    Completions are buffered per-type so that a poll triggered by one handler
    doesn't lose completions belonging to the other.
    """

    def __init__(self, engine: Any):
        self._engine = engine
        self._store_jobs: set[int] = set()
        self._load_jobs: set[int] = set()
        self._store_buf: dict[int, bool] = {}
        self._load_buf: dict[int, bool] = {}

    def register_store(self, job_id: int) -> None:
        self._store_jobs.add(job_id)

    def register_load(self, job_id: int) -> None:
        self._load_jobs.add(job_id)

    def _drain(self) -> None:
        """Drain engine completions into per-type buffers."""
        raw = self._engine.poll_completions()
        for job_id, success in raw:
            if job_id in self._store_jobs:
                self._store_jobs.discard(job_id)
                self._store_buf[job_id] = success
            elif job_id in self._load_jobs:
                self._load_jobs.discard(job_id)
                self._load_buf[job_id] = success

    def poll_stores(self) -> dict[int, bool]:
        """Return buffered store completions (drains engine first)."""
        self._drain()
        result = self._store_buf
        self._store_buf = {}
        return result

    def poll_loads(self) -> dict[int, bool]:
        """Return buffered load completions (drains engine first)."""
        self._drain()
        result = self._load_buf
        self._load_buf = {}
        return result


# ── Handler implementations ──


@dataclass
class PendingJob:
    job_id: int
    start_time: float
    num_blocks: int
    transfer_type: TransferType


class GpuToCertusHandler(OffloadingHandler):
    """Store: GPU → pinned CPU → NVMe (+ DRAM residency)."""

    def __init__(
        self,
        engine: Any,
        block_size_bytes: int,
        dispatcher: CompletionDispatcher,
    ):
        self._engine = engine
        self._block_size_bytes = block_size_bytes
        self._dispatcher = dispatcher
        self._pending: deque[PendingJob] = deque()
        self._transfer_type: TransferType = ("GPU", "Certus")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, GPULoadStoreSpec)
        assert isinstance(dst_spec, CertusLoadStoreSpec)

        gpu_block_ids = list(src_spec.block_ids)
        # Address-based store (mirror of the load path): DMA each GPU block
        # straight into its pre-reserved DRAM slot. dram_ptr was resolved by
        # prepare_store and carried in the spec, so no key lookup is needed and
        # the transfer cannot fail with KeyNotFound — the slot is held live by a
        # write reference until complete_store.
        dst_ptrs = [loc.dram_ptr for loc in dst_spec.locations]

        self._dispatcher.register_store(job_id)
        self._engine.store_dma(job_id, gpu_block_ids, dst_ptrs)
        self._pending.append(PendingJob(
            job_id=job_id,
            start_time=time.monotonic(),
            num_blocks=len(gpu_block_ids),
            transfer_type=self._transfer_type,
        ))
        COUNTERS.store_blocks_submitted += len(gpu_block_ids)
        return True

    def get_finished(self) -> list[TransferResult]:
        results: list[TransferResult] = []
        store_completions = self._dispatcher.poll_stores()
        now = time.monotonic()
        if self._pending and not store_completions:
            head = self._pending[0]
            age = now - head.start_time
            if age > 5.0:
                print(f"[STORE] get_finished STALL: oldest job={head.job_id} age={age:.1f}s, {len(self._pending)} pending, poll returned 0", flush=True)
        while self._pending and self._pending[0].job_id in store_completions:
            job = self._pending.popleft()
            success = store_completions.pop(job.job_id)
            elapsed = now - job.start_time
            nbytes = job.num_blocks * self._block_size_bytes
            results.append(TransferResult(
                job_id=job.job_id,
                success=success,
                transfer_size=nbytes,
                transfer_time=elapsed,
                transfer_type=job.transfer_type,
            ))
            COUNTERS.store_blocks_completed += job.num_blocks
            COUNTERS.store_total_bytes += nbytes
            COUNTERS.store_latencies.append(elapsed * 1000)
        if results:
            print(f"[STORE] get_finished -> {len(results)} done, {len(self._pending)} pending", flush=True)
        return results

    def wait(self, job_ids: set[int]) -> None:
        for jid in job_ids:
            self._engine.wait_job(jid)


class CertusToGpuHandler(OffloadingHandler):
    """Load: DRAM→GPU (fast) or NVMe→CPU→GPU (cache miss)."""

    def __init__(
        self,
        engine: Any,
        block_size_bytes: int,
        dispatcher: CompletionDispatcher,
    ):
        self._engine = engine
        self._block_size_bytes = block_size_bytes
        self._dispatcher = dispatcher
        self._pending: deque[PendingJob] = deque()
        self._transfer_type: TransferType = ("Certus", "GPU")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, CertusLoadStoreSpec)
        assert isinstance(dst_spec, GPULoadStoreSpec)

        gpu_block_ids = list(dst_spec.block_ids)
        src_ptrs = [loc.dram_ptr for loc in src_spec.locations]

        self._dispatcher.register_load(job_id)
        self._engine.load_dma(job_id, gpu_block_ids, src_ptrs)
        self._pending.append(PendingJob(
            job_id=job_id,
            start_time=time.monotonic(),
            num_blocks=len(gpu_block_ids),
            transfer_type=self._transfer_type,
        ))
        COUNTERS.load_blocks_submitted += len(gpu_block_ids)
        return True

    def get_finished(self) -> list[TransferResult]:
        results: list[TransferResult] = []
        load_completions = self._dispatcher.poll_loads()
        now = time.monotonic()
        if self._pending and not load_completions:
            head = self._pending[0]
            age = now - head.start_time
            if age > 5.0:
                print(f"[LOAD] get_finished STALL: oldest job={head.job_id} age={age:.1f}s, {len(self._pending)} pending, poll returned 0", flush=True)
        while self._pending and self._pending[0].job_id in load_completions:
            job = self._pending.popleft()
            success = load_completions.pop(job.job_id)
            elapsed = now - job.start_time
            nbytes = job.num_blocks * self._block_size_bytes
            results.append(TransferResult(
                job_id=job.job_id,
                success=success,
                transfer_size=nbytes,
                transfer_time=elapsed,
                transfer_type=job.transfer_type,
            ))
            COUNTERS.load_blocks_completed += job.num_blocks
            COUNTERS.load_total_bytes += nbytes
            COUNTERS.load_latencies.append(elapsed * 1000)
        if results:
            print(f"[LOAD] get_finished -> {len(results)} done, {len(self._pending)} pending", flush=True)
        return results

    def wait(self, job_ids: set[int]) -> None:
        for jid in job_ids:
            self._engine.wait_job(jid)
