# SPDX-License-Identifier: Apache-2.0
"""OffloadingHandlers that move KV blocks GPU <-> Certus over gRPC.

Each ``transfer_async`` submits one gRPC call to a background thread pool and
returns immediately; ``get_finished`` reaps completed futures. Per block we
build a proto ``IpcHandle`` sharing the KV-cache allocation's IPC handle and
setting ``offset`` to the block's byte offset, so the server DMAs at
``open(handle) + offset``.
"""

from __future__ import annotations

import time
from collections import deque
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass

from vllm.v1.kv_offload.mediums import GPULoadStoreSpec
from vllm.v1.kv_offload.worker.worker import (
    OffloadingHandler,
    TransferResult,
    TransferSpec,
)

from . import dispatcher_pb2 as pb
from .client import all_success
from .gpu import KvCacheIpc
from .mediums import CertusLoadStoreSpec


@dataclass
class _PendingJob:
    job_id: int
    future: Future
    start_time: float
    num_blocks: int


def _ipc_handle(kv: KvCacheIpc, block_id: int, size: int) -> pb.IpcHandle:
    return pb.IpcHandle(
        cuda_ipc_handle=kv.handle_bytes,
        size=size,
        gpu_device_id=kv.gpu_device_id,
        offset=kv.block_offset(block_id),
    )


class _GrpcHandler(OffloadingHandler):
    """Common async plumbing for the store and load handlers."""

    def __init__(self, stub, kv: KvCacheIpc, block_size_bytes: int, executor: ThreadPoolExecutor):
        self._stub = stub
        self._kv = kv
        self._block_size_bytes = int(block_size_bytes)
        self._executor = executor
        self._pending: deque[_PendingJob] = deque()

    # Subclasses implement the actual RPC call.
    def _do_transfer(self, gpu_block_ids: list[int], keys: list[int]) -> bool:
        raise NotImplementedError

    def _submit(self, job_id: int, gpu_block_ids: list[int], keys: list[int]) -> bool:
        future = self._executor.submit(self._do_transfer, gpu_block_ids, keys)
        self._pending.append(
            _PendingJob(
                job_id=job_id,
                future=future,
                start_time=time.monotonic(),
                num_blocks=len(gpu_block_ids),
            )
        )
        return True

    def get_finished(self) -> list[TransferResult]:
        results: list[TransferResult] = []
        now = time.monotonic()
        # Reap completed jobs in submission order (FIFO), stopping at the first
        # still-running job so we preserve ordering guarantees.
        while self._pending and self._pending[0].future.done():
            job = self._pending.popleft()
            try:
                success = bool(job.future.result())
            except Exception as e:  # noqa: BLE001 - report as a failed transfer
                print(f"[certus-grpc] transfer job {job.job_id} failed: {e}", flush=True)
                success = False
            results.append(
                TransferResult(
                    job_id=job.job_id,
                    success=success,
                    transfer_size=job.num_blocks * self._block_size_bytes,
                    transfer_time=now - job.start_time,
                    transfer_type=self._transfer_type,
                )
            )
        return results

    def wait(self, job_ids: set[int]) -> None:
        for job in list(self._pending):
            if job.job_id in job_ids:
                job.future.result()


class GpuToCertusHandler(_GrpcHandler):
    """Store: GPU -> Certus DRAM/NVMe via CopyToStore."""

    _transfer_type = ("GPU", "Certus")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, GPULoadStoreSpec)
        assert isinstance(dst_spec, CertusLoadStoreSpec)
        gpu_block_ids = list(src_spec.block_ids)
        keys = dst_spec.keys
        return self._submit(job_id, gpu_block_ids, keys)

    def _do_transfer(self, gpu_block_ids: list[int], keys: list[int]) -> bool:
        entries = [
            pb.CopyToStoreEntry(
                key=key,
                ipc_handle=_ipc_handle(self._kv, block_id, self._block_size_bytes),
            )
            for block_id, key in zip(gpu_block_ids, keys)
        ]
        resp = self._stub.CopyToStore(pb.BatchCopyToStoreRequest(entries=entries))
        return all_success(resp.results)


class CertusToGpuHandler(_GrpcHandler):
    """Load: Certus DRAM/NVMe -> GPU via Lookup."""

    _transfer_type = ("Certus", "GPU")

    def transfer_async(self, job_id: int, spec: TransferSpec) -> bool:
        src_spec, dst_spec = spec
        assert isinstance(src_spec, CertusLoadStoreSpec)
        assert isinstance(dst_spec, GPULoadStoreSpec)
        gpu_block_ids = list(dst_spec.block_ids)
        keys = src_spec.keys
        return self._submit(job_id, gpu_block_ids, keys)

    def _do_transfer(self, gpu_block_ids: list[int], keys: list[int]) -> bool:
        entries = [
            pb.LookupEntry(
                key=key,
                ipc_handle=_ipc_handle(self._kv, block_id, self._block_size_bytes),
            )
            for block_id, key in zip(gpu_block_ids, keys)
        ]
        resp = self._stub.Lookup(pb.BatchLookupRequest(entries=entries))
        return all_success(resp.results)
