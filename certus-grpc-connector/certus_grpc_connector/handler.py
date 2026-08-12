# SPDX-License-Identifier: Apache-2.0
"""The worker that moves KV blocks GPU <-> Certus over gRPC.

One class, ``CertusGrpcWorker``, serves every supported vLLM version by
implementing BOTH worker interfaces the plugin API has had:

* **≤0.24** — a per-direction ``OffloadingHandler`` with
  ``transfer_async(job_id, spec)`` dispatched by the ``(src_medium, dst_medium)``
  pair the spec's ``get_handlers`` advertised. The spec yields the SAME worker
  instance for both medium pairs; ``transfer_async`` routes to the store or load
  body by the source spec's type.
* **0.26+** — a single ``OffloadingWorker`` with explicit
  ``submit_store(job_id, src, dst)`` / ``submit_load(job_id, src, dst)`` (the
  direction is in the method name, so there is no medium-pair routing and
  ``TransferResult`` no longer carries a ``transfer_type``).

Both interfaces share one background thread pool, one pending-job deque, and the
same store/load RPC bodies. The base class is resolved lazily via
``compat.worker_base_class()`` (a factory builds the subclass on first use) so the
base that is absent on the other era is never imported.

Each submit enqueues one gRPC call onto the pool and returns immediately;
``get_finished`` reaps completed futures in FIFO order. Per block we build a proto
``IpcHandle`` sharing the KV-cache allocation's IPC handle with ``offset`` set to
the block's byte offset, so the server DMAs at ``open(handle) + offset``.
"""

from __future__ import annotations

import threading
import time
from collections import deque
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass

from .compat import (
    GPULoadStoreSpec,
    gpu_block_ids,
    make_transfer_result,
    worker_base_class,
)

from . import dispatcher_pb2 as pb
from .client import all_success
from .gpu import KvCacheIpc
from .mediums import CertusLoadStoreSpec
from .telemetry import call_rpc

# Direction tags. Only used to populate the ≤0.24 ``TransferResult.transfer_type``
# (dropped on 0.26); kept as the natural label for a job's direction either way.
_STORE_TYPE = ("GPU", "Certus")
_LOAD_TYPE = ("Certus", "GPU")

# Diagnostic rate-limiter for CopyToStore failures: print each DISTINCT
# error_message once (with a sample key), plus a hard cap on total lines, so a
# run where every copy fails surfaces the real reason without emitting 48k lines.
_COPY_FAIL_LOCK = threading.Lock()
_COPY_FAIL_SEEN: set[str] = set()
_COPY_FAIL_LINES = 0
_COPY_FAIL_MAX_LINES = 40


def _log_copy_failure(failed_results, batch_len: int) -> None:
    global _COPY_FAIL_LINES
    with _COPY_FAIL_LOCK:
        for r in failed_results:
            if _COPY_FAIL_LINES >= _COPY_FAIL_MAX_LINES:
                return
            msg = getattr(r, "error_message", "") or "(empty)"
            code = getattr(r, "error_code", "?")
            if msg in _COPY_FAIL_SEEN:
                continue
            _COPY_FAIL_SEEN.add(msg)
            _COPY_FAIL_LINES += 1
            print(
                f"[certus-grpc] CopyToStore SERVER ERROR (distinct #{_COPY_FAIL_LINES}) "
                f"key={r.key} error_code={code} msg={msg!r} "
                f"(batch had {batch_len} keys)",
                flush=True,
            )


@dataclass
class _PendingJob:
    job_id: int
    future: Future
    start_time: float
    num_blocks: int
    # Direction of this job; carried per-job because one deque now holds both
    # store and load jobs (≤0.24 used two separate handler instances).
    transfer_type: tuple[str, str]


def _ipc_handles(regions: list[KvCacheIpc], block_id: int) -> list[pb.IpcHandle]:
    """One proto IpcHandle per KV region for this block.

    0.23+ splits a block into N per-layer allocations, so we emit N handles (each
    carrying its own IPC handle, per-layer ``size`` = stride, and per-region
    ``offset``); the server lays them out contiguously in the one reserved slot.
    A single-tensor block (0.20/0.22) is just ``N == 1``. Handles are ordered by
    region so store and load scatter/gather use the same slot layout.
    """
    return [
        pb.IpcHandle(
            cuda_ipc_handle=r.handle_bytes,
            size=r.stride_bytes,
            gpu_device_id=r.gpu_device_id,
            offset=r.block_offset(block_id),
        )
        for r in regions
    ]


# Cache of the built worker class (base resolved once, lazily).
_WORKER_CLASS = None


def _build_worker_class():
    """Build ``CertusGrpcWorker`` subclassing the version-appropriate base.

    Done in a factory (not at import) so ``compat.worker_base_class()`` — which
    touches a symbol that exists on only one era — is resolved on first real use,
    keeping the pure-matrix import path vLLM-free.
    """
    base = worker_base_class()

    class CertusGrpcWorker(base):  # type: ignore[misc, valid-type]
        """Moves KV blocks GPU <-> Certus, serving both plugin-API eras."""

        def __init__(
            self,
            stub,
            kv_regions: list[KvCacheIpc],
            block_size_bytes: int,
            executor: ThreadPoolExecutor,
        ):
            self._stub = stub
            self._kv_regions = kv_regions
            self._block_size_bytes = int(block_size_bytes)
            self._executor = executor
            self._pending: deque[_PendingJob] = deque()

        # ── shared async plumbing ──

        def _submit(
            self,
            job_id: int,
            gpu_block_ids: list[int],
            keys: list[int],
            fn,
            transfer_type: tuple[str, str],
        ) -> bool:
            future = self._executor.submit(fn, gpu_block_ids, keys)
            self._pending.append(
                _PendingJob(
                    job_id=job_id,
                    future=future,
                    start_time=time.monotonic(),
                    num_blocks=len(gpu_block_ids),
                    transfer_type=transfer_type,
                )
            )
            return True

        def get_finished(self) -> "list":
            results = []
            now = time.monotonic()
            # Reap completed jobs in submission order (FIFO), stopping at the
            # first still-running job so ordering guarantees are preserved.
            while self._pending and self._pending[0].future.done():
                job = self._pending.popleft()
                try:
                    success = bool(job.future.result())
                except Exception as e:  # noqa: BLE001 - report as a failed transfer
                    print(
                        f"[certus-grpc] transfer job {job.job_id} failed: {e}",
                        flush=True,
                    )
                    success = False
                results.append(
                    make_transfer_result(
                        job_id=job.job_id,
                        success=success,
                        transfer_size=job.num_blocks * self._block_size_bytes,
                        transfer_time=now - job.start_time,
                        transfer_type=job.transfer_type,
                    )
                )
            return results

        def wait(self, job_ids: set) -> None:
            for job in list(self._pending):
                if job.job_id in job_ids:
                    job.future.result()

        def shutdown(self) -> None:
            return

        # ── 0.26 explicit-direction interface ──

        def submit_store(self, job_id: int, src_spec, dst_spec) -> bool:
            """Async GPU -> Certus (0.26). src=GPU spec, dst=Certus spec."""
            block_ids = gpu_block_ids(src_spec)
            return self._submit(
                job_id, block_ids, dst_spec.keys, self._do_store, _STORE_TYPE
            )

        def submit_load(self, job_id: int, src_spec, dst_spec) -> bool:
            """Async Certus -> GPU (0.26). src=Certus spec, dst=GPU spec."""
            block_ids = gpu_block_ids(dst_spec)
            return self._submit(
                job_id, block_ids, src_spec.keys, self._do_load, _LOAD_TYPE
            )

        # ── ≤0.24 medium-pair interface ──

        def transfer_async(self, job_id: int, spec) -> bool:
            """Route a (src_spec, dst_spec) pair to the store or load body by the
            source medium type — the spec yields this one instance for both
            medium pairs, so the direction is recovered from the spec shapes."""
            src_spec, dst_spec = spec
            if isinstance(src_spec, GPULoadStoreSpec):
                assert isinstance(dst_spec, CertusLoadStoreSpec)
                return self.submit_store(job_id, src_spec, dst_spec)
            assert isinstance(src_spec, CertusLoadStoreSpec)
            assert isinstance(dst_spec, GPULoadStoreSpec)
            return self.submit_load(job_id, src_spec, dst_spec)

        # ── RPC bodies (run on the pool; identical across versions) ──

        def _do_store(self, gpu_block_ids: list[int], keys: list[int]) -> bool:
            entries = [
                pb.CopyToStoreEntry(
                    key=key,
                    ipc_handles=_ipc_handles(self._kv_regions, block_id),
                )
                for block_id, key in zip(gpu_block_ids, keys)
            ]
            try:
                resp = call_rpc(
                    self._stub,
                    "CopyToStore",
                    pb.BatchCopyToStoreRequest(entries=entries),
                    items=len(entries),
                )
            except Exception as e:  # noqa: BLE001 - store failure must not crash vLLM
                # A whole-batch RPC failure: roll back all reservations and report
                # success (see the invariant note below). Blocks stay uncached.
                print(
                    f"[certus-grpc] CopyToStore RPC error: {e} — aborting {len(keys)} keys",
                    flush=True,
                )
                try:
                    call_rpc(
                        self._stub,
                        "AbortStore",
                        pb.BatchAbortStoreRequest(keys=keys),
                        items=len(keys),
                    )
                except Exception:  # noqa: BLE001
                    pass
                return True

            # CRITICAL: the store path must NEVER report success=False. vLLM's
            # offloading worker asserts transfer_result.success and a False
            # return kills the engine. A failed CopyToStore only means "this block
            # won't be cached" — the KV data is still valid in GPU memory, so it
            # is safe to drop. But because store is split-phase (Reserve ->
            # CopyToStore -> CommitStore), we must roll back any key whose copy
            # failed, so the subsequent CommitStore can't publish an unpopulated
            # slot as a valid entry. Abort the failed keys; report success.
            failed_results = [r for r in resp.results if not r.success]
            failed = [r.key for r in failed_results]
            if failed:
                # DIAGNOSTIC: the server already returns the real reason per key in
                # error_message (e.g. "GPU async DMA copy failed: cudaMemcpyAsync
                # D2H failed: ..." or "size (N) exceeds destination buffer length
                # (M)"). We normally discard it; surface the first few distinct
                # messages so a store-path regression isn't silent. Rate-limited so
                # a 48k-failure run doesn't spew.
                _log_copy_failure(failed_results, len(keys))
                print(
                    f"[certus-grpc] CopyToStore failed for {len(failed)}/{len(keys)} "
                    f"blocks — aborting those reservations, leaving them uncached",
                    flush=True,
                )
                try:
                    call_rpc(
                        self._stub,
                        "AbortStore",
                        pb.BatchAbortStoreRequest(keys=failed),
                        items=len(failed),
                    )
                except Exception as e:  # noqa: BLE001 - best-effort rollback
                    print(f"[certus-grpc] AbortStore rollback failed: {e}", flush=True)
            return True

        def _do_load(self, gpu_block_ids: list[int], keys: list[int]) -> bool:
            entries = [
                pb.LookupEntry(
                    key=key,
                    ipc_handles=_ipc_handles(self._kv_regions, block_id),
                )
                for block_id, key in zip(gpu_block_ids, keys)
            ]
            resp = call_rpc(
                self._stub,
                "Lookup",
                pb.BatchLookupRequest(entries=entries),
                items=len(entries),
            )
            # Diagnostic: a load must not fail (vLLM asserts), and it shouldn't be
            # able to — prepare_load pinned these keys. If the server reports any
            # per-key failure, dump exactly which key + error so we can see WHY a
            # Lookup missed a key that lookup()/Check said was present.
            if not all_success(resp.results):
                for r in resp.results:
                    if not r.success:
                        print(
                            f"[certus-grpc] LOAD FAILURE key={r.key} "
                            f"error_code={r.error_code} msg={r.error_message!r} "
                            f"(this key was Check-hit and Pinned in prepare_load)",
                            flush=True,
                        )
                return False
            return True

    return CertusGrpcWorker


def worker_class():
    """Return the (cached) version-appropriate ``CertusGrpcWorker`` class."""
    global _WORKER_CLASS
    if _WORKER_CLASS is None:
        _WORKER_CLASS = _build_worker_class()
    return _WORKER_CLASS
