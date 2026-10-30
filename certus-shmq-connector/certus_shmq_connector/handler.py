# SPDX-License-Identifier: Apache-2.0
"""The worker that moves KV blocks GPU <-> Certus over the shared-memory ring.

Byte-for-byte the same job/threadpool machinery as the gRPC connector's
``handler.py`` — only the transport calls change: the two RPC bodies issue
``ring.copy_to_store_blocks`` / ``ring.lookup_blocks`` instead of
``stub.CopyToStore``/``stub.Lookup``, and rollback uses ``ring.abort_store``.
Each call sends the worker's per-layer table (encoded once at init) plus one
GPU block id per key; the server computes every region's offset. A server
without those opcodes gets the legacy per-region form: ``(key, regions)``
entries where ``regions`` is ``[(handle_bytes, gpu_device_id, block_offset,
stride_bytes)]`` (``_regions_for_block``), sent with ``ring.copy_to_store`` /
``ring.lookup``.

One class, ``CertusShmqWorker``, serves every supported vLLM version (see the
gRPC handler for the ≤0.24 vs 0.26 interface split — unchanged here).
"""

from __future__ import annotations

import atexit
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
from .ring import encode_layer_table

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


class _HandleEncodeLog:
    """Diagnostic log of each load/store body's request build and ring time, one
    JSON line per call, in ``$OPTIMIZER_ARTIFACTS_DIR/handle_encode.<pid>.jsonl``.
    Exists only when that variable is set. Lines are buffered and written every
    64 lines and at interpreter exit; a write error disables the log, it never
    raises."""

    _FLUSH_LINES = 64

    def __init__(self, directory: str):
        self._path: str | None = os.path.join(
            directory, f"handle_encode.{os.getpid()}.jsonl"
        )
        self._lines: list[str] = []
        self._lock = threading.Lock()
        self._t0_ns: int | None = None
        atexit.register(self.flush)

    def record(
        self, op: str, blocks: int, regions: int, path: str, encode_ns: int, rpc_ns: int
    ) -> None:
        if self._path is None:
            return
        now_ns = time.monotonic_ns()
        with self._lock:
            if self._t0_ns is None:
                self._t0_ns = now_ns
            self._lines.append(
                f'{{"op":"{op}","blocks":{blocks},"regions":{regions},'
                f'"path":"{path}","encode_us":{encode_ns / 1000:.1f},'
                f'"rpc_us":{rpc_ns / 1000:.1f},"t_us":{(now_ns - self._t0_ns) // 1000}}}\n'
            )
            full = len(self._lines) >= self._FLUSH_LINES
        if full:
            self.flush()

    def flush(self) -> None:
        with self._lock:
            if self._path is None or not self._lines:
                return
            lines, self._lines = self._lines, []
            try:
                with open(self._path, "a") as f:
                    f.write("".join(lines))
            except OSError:
                self._path = None


_ARTIFACTS_DIR = os.environ.get("OPTIMIZER_ARTIFACTS_DIR")
_ENCODE_LOG = _HandleEncodeLog(_ARTIFACTS_DIR) if _ARTIFACTS_DIR else None


def _is_unknown_opcode(e: Exception) -> bool:
    """True for a server's rejection of a *_BLOCKS opcode it does not know."""
    return "unknown opcode" in str(e)


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
            # Per-layer table for the *_BLOCKS ops, encoded once: the server
            # computes each region as base_delta + block_id * stride (size =
            # stride, layer order — exactly what _regions_for_block emits). None
            # means this worker always uses the legacy per-region ops.
            try:
                self._layer_table = encode_layer_table(
                    [
                        (r.handle_bytes, r.gpu_device_id, int(r.base_delta),
                         int(r.stride_bytes), int(r.stride_bytes))
                        for r in kv_regions
                    ]
                )
            except Exception:  # noqa: BLE001 - odd region objects use the legacy path
                self._layer_table = None
            self._encode_log = _ENCODE_LOG

        # Cleared the first time a server rejects a *_BLOCKS opcode as unknown,
        # so every later call (on any worker) goes straight to the legacy ops.
        _blocks_supported = True

        def _ring_blocks(self, op: str, gpu_block_ids, ns_keys):
            """Send one load/store over the *_BLOCKS op, or the legacy per-region
            op when the server or this worker's regions don't support it. Returns
            the per-key results in key order."""
            log = self._encode_log
            if self._layer_table is not None and CertusShmqWorker._blocks_supported:
                fn = self._ring.lookup_blocks if op == "load" else self._ring.copy_to_store_blocks
                timings = [] if log is not None else None
                t0 = time.perf_counter_ns() if log is not None else 0
                try:
                    results = fn(self._layer_table, ns_keys, gpu_block_ids, timings=timings)
                except Exception as e:  # noqa: BLE001 - only unknown-opcode falls back
                    if not _is_unknown_opcode(e):
                        raise
                    CertusShmqWorker._blocks_supported = False
                else:
                    if log is not None:
                        total = time.perf_counter_ns() - t0
                        enc = timings[0] if timings else 0
                        log.record(op, len(ns_keys), len(ns_keys) * len(self._kv_regions),
                                   "blocks", enc, total - enc)
                    return results
            t0 = time.perf_counter_ns() if log is not None else 0
            entries = [
                (ns_k, _regions_for_block(self._kv_regions, block_id))
                for block_id, ns_k in zip(gpu_block_ids, ns_keys)
            ]
            t1 = time.perf_counter_ns() if log is not None else 0
            if op == "load":
                results = self._ring.lookup(entries)
            else:
                results = self._ring.copy_to_store(entries)
            if log is not None:
                log.record(op, len(ns_keys), len(ns_keys) * len(self._kv_regions),
                           "legacy", t1 - t0, time.perf_counter_ns() - t1)
            return results

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
            # Reap EVERY completed job, regardless of its position in the queue —
            # not just a leading FIFO run. The previous version stopped at the
            # first still-running job, so one slow transfer withheld all the
            # completed transfers queued behind it. vLLM keeps a request's GPU
            # blocks pinned and defers the request until its transfer jobs are
            # reported finished (OffloadingConnectorScheduler.get_num_new_matched_
            # tokens returns None while transfer_jobs is non-empty), so withholding
            # completions pins blocks and defers requests; under offload saturation
            # (256+ concurrent streams) this collapses into a capacity deadlock
            # (0 running / N waiting, GPU idle). vLLM matches completions by
            # explicit job_id in update_connector_output, so out-of-order reaping
            # is correct — in-order delivery was never required by the consumer.
            if not self._pending:
                return results
            remaining: deque[_PendingJob] = deque()
            for job in self._pending:
                if not job.future.done():
                    remaining.append(job)
                    continue
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
            self._pending = remaining
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
            if len(gpu_block_ids) != len(keys):
                raise AssertionError(
                    f"store: {len(gpu_block_ids)} GPU blocks != {len(keys)} keys "
                    f"(one key per GPU block required)"
                )
            # Fold this worker's TP rank into every key: the manager Reserved the
            # matching per-rank namespaced keys (see manager.prepare_store), so the
            # store lands under the same key the pre-load Check/Pin will look up.
            ns_keys = [ns_key(k, self._rank, self._world_size) for k in keys]
            try:
                results = self._ring_blocks("store", gpu_block_ids, ns_keys)
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
            if len(gpu_block_ids) != len(keys):
                raise AssertionError(
                    f"load: {len(gpu_block_ids)} GPU blocks != {len(keys)} keys "
                    f"(one key per GPU block required)"
                )
            # Same rank fold as the store path — load this worker's own shard.
            ns_keys = [ns_key(k, self._rank, self._world_size) for k in keys]
            results = self._ring_blocks("load", gpu_block_ids, ns_keys)
            # Diagnostic: a load must not fail (vLLM asserts), and it shouldn't be
            # able to — prepare_load pinned these keys. If the server reports any
            # per-key miss, dump exactly which key so we can see WHY a Lookup missed
            # a key that lookup()/Check said was present. (The ring transport has no
            # per-key error string, unlike gRPC.)
            if not all(results):
                for ns_k, ok in zip(ns_keys, results):
                    if not ok:
                        print(
                            f"[certus-shmq] LOAD FAILURE key={ns_k} "
                            f"(this key was Check-hit and Pinned in prepare_load)",
                            flush=True,
                        )
                return False
            return True

    return CertusShmqWorker


def worker_class():
    """Return the (cached) version-appropriate ``CertusShmqWorker`` class."""
    global _WORKER_CLASS
    if _WORKER_CLASS is None:
        _WORKER_CLASS = _build_worker_class()
    return _WORKER_CLASS
