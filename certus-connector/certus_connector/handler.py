# SPDX-License-Identifier: Apache-2.0
"""OffloadingHandlers for Certus: parallel DMA via ThreadPoolExecutor.

Each handler submits store_async/load_async calls through a thread pool
(GIL released in Rust via py.allow_threads). A shared CompletionDispatcher
routes poll_completions() results to the correct handler.
"""

from __future__ import annotations

import time
from collections import deque
from concurrent.futures import ThreadPoolExecutor
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


# ── Completion routing ──


class CompletionDispatcher:
    """Routes poll_completions() results to the handler that owns each job."""

    def __init__(self, engine: Any):
        self._engine = engine
        self._store_jobs: set[int] = set()
        self._load_jobs: set[int] = set()

    def register_store(self, job_id: int) -> None:
        self._store_jobs.add(job_id)

    def register_load(self, job_id: int) -> None:
        self._load_jobs.add(job_id)

    def poll(self) -> tuple[dict[int, bool], dict[int, bool]]:
        """Poll engine and split completions into (store_map, load_map)."""
        raw = self._engine.poll_completions()
        stores: dict[int, bool] = {}
        loads: dict[int, bool] = {}
        for job_id, success in raw:
            if job_id in self._store_jobs:
                self._store_jobs.discard(job_id)
                stores[job_id] = success
            elif job_id in self._load_jobs:
                self._load_jobs.discard(job_id)
                loads[job_id] = success
        if raw:
            print(f"[DISP] poll -> {len(stores)} stores, {len(loads)} loads completed (pending: {len(self._store_jobs)}s/{len(self._load_jobs)}l)", flush=True)
        return stores, loads


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
        max_workers: int = 8,
    ):
        self._engine = engine
        self._block_size_bytes = block_size_bytes
        self._dispatcher = dispatcher
        self._pool = ThreadPoolExecutor(max_workers=max_workers)
        self._pending: deque[PendingJob] = deque()
        self._transfer_type: TransferType = ("GPU", "Certus")

    def _do_store(self, job_id: int, gpu_block_ids: list, keys: list):
        print(f"[STORE] _do_store ENTER job={job_id}", flush=True)
        try:
            result = self._engine.store_async(job_id, gpu_block_ids, keys)
            print(f"[STORE] _do_store EXIT job={job_id} result={result}", flush=True)
            return result
        except Exception as e:
            print(f"[STORE] _do_store EXCEPTION job={job_id}: {e}", flush=True)
            raise

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, GPULoadStoreSpec)
        assert isinstance(dst_spec, CertusLoadStoreSpec)

        gpu_block_ids = list(src_spec.block_ids)
        keys = [loc.nvme_slab for loc in dst_spec.locations]

        print(f"[STORE] transfer_async job={job_id} blocks={len(gpu_block_ids)}", flush=True)
        self._dispatcher.register_store(job_id)
        self._pool.submit(self._do_store, job_id, gpu_block_ids, keys)
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
        store_completions, _ = self._dispatcher.poll()
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
        max_workers: int = 8,
    ):
        self._engine = engine
        self._block_size_bytes = block_size_bytes
        self._dispatcher = dispatcher
        self._pool = ThreadPoolExecutor(max_workers=max_workers)
        self._pending: deque[PendingJob] = deque()
        self._transfer_type: TransferType = ("Certus", "GPU")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, CertusLoadStoreSpec)
        assert isinstance(dst_spec, GPULoadStoreSpec)

        gpu_block_ids = list(dst_spec.block_ids)
        keys = [loc.nvme_slab for loc in src_spec.locations]

        print(f"[LOAD] transfer_async job={job_id} blocks={len(gpu_block_ids)}", flush=True)
        self._dispatcher.register_load(job_id)
        self._pool.submit(self._engine.load_async, job_id, gpu_block_ids, keys)
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
        _, load_completions = self._dispatcher.poll()
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
