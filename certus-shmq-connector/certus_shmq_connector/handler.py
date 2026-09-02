# SPDX-License-Identifier: Apache-2.0
"""The worker that moves KV blocks GPU <-> Certus over the shared-memory ring.

Byte-for-byte the same job/threadpool machinery as the gRPC connector's
``handler.py`` — only the transport calls change: the two RPC bodies issue
``ring.copy_to_store(entries)`` / ``ring.lookup(entries)`` instead of
``stub.CopyToStore``/``stub.Lookup``, and rollback uses ``ring.abort_store``.
Each ``entry`` is ``(key, regions)`` where ``regions`` is the per-layer list
``[(handle_bytes, gpu_device_id, block_offset, stride_bytes)]`` — ``ring.py``
dedups the distinct handles into a table and chunks oversize batches to fit
``cap_req`` for us, so the worker just hands it the full per-block region list.

One class, ``CertusShmqWorker``, serves every supported vLLM version (see the
gRPC handler for the ≤0.24 vs 0.26 interface split — unchanged here).
"""

from __future__ import annotations

import json
import os
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

from .gpu import KvCacheIpc
from .mediums import CertusLoadStoreSpec, ns_key

# Direction tags. Only used to populate the ≤0.24 ``TransferResult.transfer_type``
# (dropped on 0.26); kept as the natural label for a job's direction either way.
_STORE_TYPE = ("GPU", "Certus")
_LOAD_TYPE = ("Certus", "GPU")

# Optional per-transfer submit trace. Gated on CERTUS_SHMQ_TRACE_SUBMIT (default
# off). When on, each submit_load/submit_store appends one JSONL record carrying
# the transfer's GPU-block count and key count, so a TRACE_OFFLOAD run can PROVE
# the worker's block count equals the scheduler's prepare_load ``load_blocks``:
# per record ``num_blocks == num_keys`` confirms the 1:1 gpu-block↔key zip is
# whole, and the multiset of ``num_blocks`` across submit records must equal the
# multiset of ``load_blocks``/``store_blocks`` across the connector trace.
#
# This is a per-TRANSFER hook (~one call per request), NOT a per-LAYER one, so it
# does not stall the engine the way tracing save_kv_layer would. Records go to
# TRACE_DIR (shared with the connector tracer) under submit_trace_<pid>.jsonl so
# a --rm container's host mount captures them; the worker pid differs from the
# scheduler pid, so it never collides with offloading_trace_<pid>.jsonl.
_SUBMIT_TRACE = os.environ.get("CERTUS_SHMQ_TRACE_SUBMIT", "0") not in (
    "0", "", "false", "False",
)
_SUBMIT_TRACE_LOCK = threading.Lock()
_SUBMIT_TRACE_FH = None


def _submit_trace(method: str, job_id: int, num_blocks: int, num_keys: int) -> None:
    """Append one submit-trace JSONL record (best-effort; never raises)."""
    global _SUBMIT_TRACE_FH
    try:
        with _SUBMIT_TRACE_LOCK:
            if _SUBMIT_TRACE_FH is None:
                d = os.environ.get("TRACE_DIR") or os.path.dirname(
                    os.path.abspath(__file__)
                )
                _SUBMIT_TRACE_FH = open(
                    os.path.join(d, f"submit_trace_{os.getpid()}.jsonl"), "a"
                )
            _SUBMIT_TRACE_FH.write(
                json.dumps(
                    {
                        "pid": os.getpid(),
                        "method": method,
                        "job_id": job_id,
                        "num_blocks": num_blocks,  # len(gpu_block_ids) this transfer
                        "num_keys": num_keys,  # len(keys); must equal num_blocks (1:1)
                    }
                )
                + "\n"
            )
            _SUBMIT_TRACE_FH.flush()
    except Exception:  # noqa: BLE001 - tracing must never break a transfer
        pass


# Diagnostic rate-limiter for CopyToStore failures. The ring transport returns
# only a per-key ok/fail bool (no server-side error string, unlike gRPC's
# TransferResult.error_message), so we log a bounded number of the FAILED KEYS
# rather than distinct messages — enough to prove a store-path regression isn't
# silent without spewing 48k lines when every copy fails.
_COPY_FAIL_LOCK = threading.Lock()
_COPY_FAIL_LINES = 0
_COPY_FAIL_MAX_LINES = 40


def _log_copy_failure(failed_keys: list[int], batch_len: int) -> None:
    global _COPY_FAIL_LINES
    with _COPY_FAIL_LOCK:
        if _COPY_FAIL_LINES >= _COPY_FAIL_MAX_LINES:
            return
        _COPY_FAIL_LINES += 1
        sample = failed_keys[:8]
        print(
            f"[certus-shmq] CopyToStore FAILED (#{_COPY_FAIL_LINES}) "
            f"{len(failed_keys)}/{batch_len} keys, sample={sample}",
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


def _regions_for_block(regions: list[KvCacheIpc], block_id: int):
    """One ring region tuple per KV region for this block.

    0.23+ splits a block into N per-layer allocations, so we emit N regions (each
    carrying its own IPC handle, per-layer ``size`` = stride, and per-region byte
    ``offset``); the server lays them out contiguously in the one reserved slot. A
    single-tensor block (0.20/0.22) is just ``N == 1``. Regions are ordered by
    layer so store and load scatter/gather use the same slot layout.

    Tuple order matches ``ring.encode_handle_batch``:
    ``(handle_bytes, gpu_device_id, offset, size)``.
    """
    return [
        (r.handle_bytes, r.gpu_device_id, r.block_offset(block_id), r.stride_bytes)
        for r in regions
    ]


# Cache of the built worker class (base resolved once, lazily).
_WORKER_CLASS = None


def _build_worker_class():
    """Build ``CertusShmqWorker`` subclassing the version-appropriate base.

    Done in a factory (not at import) so ``compat.worker_base_class()`` — which
    touches a symbol that exists on only one era — is resolved on first real use,
    keeping the pure-matrix import path vLLM-free.
    """
    base = worker_base_class()

    class CertusShmqWorker(base):  # type: ignore[misc, valid-type]
        """Moves KV blocks GPU <-> Certus over the ring, serving both API eras."""

        def __init__(
            self,
            ring,
            kv_regions: list[KvCacheIpc],
            block_size_bytes: int,
            executor: ThreadPoolExecutor,
            *,
            rank: int = 0,
            world_size: int = 1,
        ):
            self._ring = ring
            self._kv_regions = kv_regions
            self._block_size_bytes = int(block_size_bytes)
            self._executor = executor
            self._pending: deque[_PendingJob] = deque()
            # TP shard rank folded into every server key so this worker's shard
            # of a block never collides with another rank's identical logical key
            # in the shared certus-server tier. Identity at world_size==1.
            self._rank = int(rank)
            self._world_size = int(world_size)

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
                        f"[certus-shmq] transfer job {job.job_id} failed: {e}",
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
            if _SUBMIT_TRACE:
                _submit_trace("submit_store", job_id, len(block_ids), len(dst_spec.keys))
            return self._submit(
                job_id, block_ids, dst_spec.keys, self._do_store, _STORE_TYPE
            )

        def submit_load(self, job_id: int, src_spec, dst_spec) -> bool:
            """Async Certus -> GPU (0.26). src=Certus spec, dst=GPU spec."""
            block_ids = gpu_block_ids(dst_spec)
            if _SUBMIT_TRACE:
                _submit_trace("submit_load", job_id, len(block_ids), len(src_spec.keys))
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

        # ── transport bodies (run on the pool; identical across versions) ──

        def _do_store(self, gpu_block_ids: list[int], keys: list[int]) -> bool:
            # Fold this worker's TP rank into every key: the manager Reserved the
            # matching per-rank namespaced keys (see manager.prepare_store), so the
            # store lands under the same key the pre-load Check/Pin will look up.
            ns_keys = [ns_key(k, self._rank, self._world_size) for k in keys]
            entries = [
                (ns_k, _regions_for_block(self._kv_regions, block_id))
                for block_id, ns_k in zip(gpu_block_ids, ns_keys)
            ]
            try:
                results = self._ring.copy_to_store(entries)
            except Exception as e:  # noqa: BLE001 - store failure must not crash vLLM
                # A whole-batch transport failure: roll back all reservations and
                # report success (see the invariant note below). Blocks stay
                # uncached.
                print(
                    f"[certus-shmq] CopyToStore transport error: {e} — "
                    f"aborting {len(ns_keys)} keys",
                    flush=True,
                )
                try:
                    self._ring.abort_store(ns_keys)
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
            failed = [ns_k for ns_k, ok in zip(ns_keys, results) if not ok]
            if failed:
                # DIAGNOSTIC: the ring transport reports only a per-key bool, so we
                # can't surface the server's reason string here (gRPC could). Log a
                # bounded sample of the failed keys so a store-path regression isn't
                # silent, then roll them back.
                _log_copy_failure(failed, len(ns_keys))
                print(
                    f"[certus-shmq] CopyToStore failed for {len(failed)}/{len(ns_keys)} "
                    f"blocks — aborting those reservations, leaving them uncached",
                    flush=True,
                )
                try:
                    self._ring.abort_store(failed)
                except Exception as e:  # noqa: BLE001 - best-effort rollback
                    print(f"[certus-shmq] AbortStore rollback failed: {e}", flush=True)
            return True

        def _do_load(self, gpu_block_ids: list[int], keys: list[int]) -> bool:
            # Same rank fold as the store path — load this worker's own shard.
            ns_keys = [ns_key(k, self._rank, self._world_size) for k in keys]
            entries = [
                (ns_k, _regions_for_block(self._kv_regions, block_id))
                for block_id, ns_k in zip(gpu_block_ids, ns_keys)
            ]
            results = self._ring.lookup(entries)
            # Per-key misses are possible under memory pressure (cold promote
            # AllocationFailed) or if the entry was evicted between the
            # scheduler's lookup and prepare_load's check_and_pin. Report them
            # as warnings but return True — a False return kills the vLLM
            # engine (worker asserts transfer success). The missed keys' GPU
            # blocks retain their prior content; vLLM's scheduler will detect
            # the stale KV on the next attention step and reschedule the
            # affected tokens for recomputation.
            if not all(results):
                failed = sum(1 for ok in results if not ok)
                for ns_k, ok in zip(ns_keys, results):
                    if not ok:
                        print(
                            f"[certus-shmq] LOAD MISS key={ns_k} "
                            f"(cold promote failed or entry evicted — "
                            f"GPU block retains prior content, will recompute)",
                            flush=True,
                        )
                print(
                    f"[certus-shmq] {failed}/{len(results)} keys missed in "
                    f"load batch — returning success to avoid engine crash",
                    flush=True,
                )
            return True

    return CertusShmqWorker


def worker_class():
    """Return the (cached) version-appropriate ``CertusShmqWorker`` class."""
    global _WORKER_CLASS
    if _WORKER_CLASS is None:
        _WORKER_CLASS = _build_worker_class()
    return _WORKER_CLASS
