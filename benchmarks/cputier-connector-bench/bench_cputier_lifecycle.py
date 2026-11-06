#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""End-to-end cputier connector benchmark: full vLLM offloading lifecycle.

The cputier counterpart of benchmarks/certus-connector-bench/bench_connector_lifecycle.py.
Instead of the Certus-SHMQ connector it drives vLLM's in-tree native offload
stack -- the same classes ``OffloadingConnector`` builds when vLLM is started
with ``spec_name=TieringOffloadingSpec`` (see benchmarks/vllm-serve/run-serve-cputier.sh):

  - TieringOffloadingManager (CPU primary tier + optional "fs" disk tier):
    on_new_request, touch, lookup, prepare_store, complete_store,
    prepare_load, complete_load, take_events, on_schedule_end,
    on_request_finished
  - CPUOffloadingWorker: submit_store, submit_load, get_finished
    (GPU<->pinned /dev/shm mmap via swap_blocks on per-transfer CUDA streams)

No vLLM engine, model, or HTTP server is started. The benchmark builds the
OffloadingConfig by hand, allocates a fake per-layer GPU KV cache, and plays
the role of the vLLM scheduler: one ReqContext per request, lookups that come
back RETRY / HIT_PENDING (async fs probe, secondary->CPU promotion) are retried
across scheduler steps (``on_schedule_end``), exactly as the connector does.
That retry wait is reported separately as the ``lookup_wait`` phase.

Call order: a request's lookups come first and its touch() after them (after
every lookup pass, including the ones deferred on RETRY), as vLLM's offloading
scheduler does since 0.21 (vLLM PR #41228; still so in 0.30). --lookup-order
touch-first touches once before the lookups instead (vLLM <= 0.20 and this
benchmark's earlier behaviour). prefix-miss retries each key in place and
touches once after the whole scan. Store-only paths touch before prepare_store.

Output (report + CSV columns) matches certus-connector-bench so the two
backends can be compared side by side.

Requires vLLM >= 0.30 (the ``vllm.v1.kv_offload.config.OffloadingConfig`` API)
and a CUDA GPU. vLLM is not installed on the host; run it in the vLLM image:

    benchmarks/cputier-connector-bench/run-in-container.sh            # default 4 phases

    # CPU-only offload (CPUOffloadingSpec, no disk tier)
    run-in-container.sh --spec cpu

    # Larger batches, longer run
    run-in-container.sh --bs 64 --num-blocks 256 --min-duration 10

    # Run the SAME pattern as certus_fio but through the cputier connector
    run-in-container.sh --pattern cold_prefill_store

    # Round-trip data check (warm + cold) before benchmarking
    run-in-container.sh --verify

    # CSV output for automated collection
    run-in-container.sh --csv results.csv
"""

from __future__ import annotations

import argparse
import ast
import atexit
import collections
import csv
import hashlib
import itertools
import math
import os
import random
import shutil
import signal
import statistics
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field, fields
from pathlib import Path

os.environ.setdefault("VLLM_LOGGING_LEVEL", "WARNING")

_here = os.path.dirname(os.path.abspath(__file__))
_repo = os.path.join(_here, "..", "..")

import torch

assert torch.cuda.is_available(), "CUDA GPU required"

try:
    from vllm.v1.kv_offload.base import (
        CanonicalKVCacheRef,
        CanonicalKVCaches,
        CanonicalKVCacheTensor,
        GPULoadStoreSpec,
        LookupResult,
        Medium,
        ReqContext,
        ScheduleEndContext,
        make_offload_key,
    )
    from vllm.v1.kv_offload.config import (
        OffloadingCacheConfig,
        OffloadingConfig,
        OffloadingGroupConfig,
        OffloadingModelConfig,
        OffloadingParallelConfig,
    )
    import vllm.v1.kv_offload.cpu.spec as _cpu_spec_mod
    from vllm.v1.kv_offload.cpu.spec import CPUOffloadingSpec
    from vllm.v1.kv_offload.tiering.spec import TieringOffloadingSpec
except ImportError as _e:
    try:
        from importlib.metadata import version as _dv
        _have = _dv("vllm")
    except Exception:
        _have = "not installed"
    sys.exit(
        f"error: this benchmark needs vLLM >= 0.30 (native kv_offload API); "
        f"found vllm {_have} ({_e}).\n"
        f"       Run it in the vLLM image: benchmarks/cputier-connector-bench/run-in-container.sh"
    )

# CPUOffloadingSpec.create_worker() calls a gloo barrier across worker ranks
# before unlinking its shared mmap. With one rank the barrier is a no-op; under
# real TP (--gpus) every rank process installs a shared multiprocessing barrier
# instead, so no rank unlinks the file before the others have mapped it
# (TieringOffloadingSpec does not use it).
_cpu_spec_mod._all_workers_barrier = lambda: None

# Default block size: 2 MiB (Llama-3-8B: 16 tokens × 32 layers × 4096 bytes),
# same as certus-connector-bench. Split across --layers per-layer KV tensors.
DEFAULT_BLOCK_BYTES = 2 * 1024 * 1024
DEFAULT_LAYERS = 32
POLL_S = 0.0001
# Stand-in for one engine step (model forward) between scheduler retries.
STEP_BACKOFF_S = 0.001
# Order of the lookup() and touch() calls for one request's keys (--lookup-order).
# vLLM's offloading scheduler (get_num_new_matched_tokens) looks up first and
# touches after since 0.21 (vLLM PR #41228, still so in 0.30, including the
# calls it defers on RETRY); vLLM <= 0.20 touched first.
LOOKUP_FIRST = "lookup-first"
TOUCH_FIRST = "touch-first"
LOOKUP_ORDERS = (LOOKUP_FIRST, TOUCH_FIRST)


def parse_size(s: str) -> int:
    """Parse '4G', '512M', '1048576' into bytes (binary units)."""
    s = str(s).strip()
    mult = {"K": 1 << 10, "M": 1 << 20, "G": 1 << 30, "T": 1 << 40}
    if s and s[-1].upper() in mult:
        return int(float(s[:-1]) * mult[s[-1].upper()])
    return int(s)


def _mk(cls, **kw):
    """Construct a vLLM config dataclass, dropping fields this version lacks."""
    names = {f.name for f in fields(cls)}
    return cls(**{k: v for k, v in kw.items() if k in names})


# ── content-hashed keys (realistic vLLM pattern) ───────────────────────────

def make_content_keys(n: int, seed: int = 42) -> list[bytes]:
    """Generate n distinct 36-byte OffloadKeys like vLLM's content-hashed blocks.

    An OffloadKey is a 32-byte block hash + 4-byte KV-cache-group index. The
    benchmark models a single KV cache group, so the group index is always 0
    (the fs tier encodes it in the file path).
    """
    return [
        make_offload_key(hashlib.blake2b(f"{seed}:{i}".encode(), digest_size=32).digest(), 0)
        for i in range(n)
    ]


def make_int_keys(ints: list[int]) -> list[bytes]:
    """Integer keys (certus_fio keyspace) packed as OffloadKeys, no hashing."""
    return [make_offload_key(i.to_bytes(32, "big"), 0) for i in ints]


def _fresh_seed() -> int:
    # The fs tier persists across modes (and across runs with --keep-fs), so
    # every key set uses a fresh seed to avoid hitting stale files.
    return random.randint(10_000_000, 99_000_000)


def rekey(keys: list[bytes], generation: int) -> list[bytes]:
    """Derive a distinct key set per store pass (generation 0 = ``keys``).

    The fs tier skips writing a block whose file already exists, so storing
    the same keys again would never reach the disk after the first pass.
    """
    if generation == 0:
        return keys
    tag = generation.to_bytes(8, "big")
    return [make_offload_key(hashlib.blake2b(k + tag, digest_size=32).digest(), 0)
            for k in keys]


# ── fake GPU KV cache ───────────────────────────────────────────────────────

def make_kv_caches(
    gpu_id: int, num_layers: int, block_bytes: int, num_gpu_blocks: int,
) -> tuple[CanonicalKVCaches, list[torch.Tensor]]:
    """Allocate one int8 (num_gpu_blocks, page) tensor per layer.

    This is the canonical layout the OffloadingConnector worker hands to
    get_worker(): every layer is its own tensor and one KV cache group
    references all of them, so each block transfer is ``num_layers`` copy
    descriptors of ``block_bytes / num_layers`` bytes.
    """
    assert block_bytes % num_layers == 0, "--block-bytes must be divisible by --layers"
    page = block_bytes // num_layers
    tensors, refs, raw = [], [], []
    for i in range(num_layers):
        t = torch.randint(-128, 128, (num_gpu_blocks, page), dtype=torch.int8,
                          device=f"cuda:{gpu_id}")
        tensors.append(CanonicalKVCacheTensor(tensor=t, page_size_bytes=page))
        refs.append(CanonicalKVCacheRef(tensor_idx=i, page_size_bytes=page))
        raw.append(t)
    return CanonicalKVCaches(tensors=tensors, group_data_refs=[refs]), raw


# ── phase timing ────────────────────────────────────────────────────────────

@dataclass
class PhaseTiming:
    name: str
    samples: list[float] = field(default_factory=list)

    def record(self, seconds: float):
        self.samples.append(seconds)

    @property
    def p50_us(self) -> float:
        return statistics.median(self.samples) * 1e6 if self.samples else 0.0

    @property
    def p99_us(self) -> float:
        if not self.samples:
            return 0.0
        s = sorted(self.samples)
        return s[min(int(len(s) * 0.99), len(s) - 1)] * 1e6

    @property
    def mean_us(self) -> float:
        return statistics.mean(self.samples) * 1e6 if self.samples else 0.0


@dataclass
class BenchResult:
    label: str
    total_blocks: int
    total_bytes: int
    wall_seconds: float
    phases: dict[str, PhaseTiming]
    # (GB/s, seconds) including time excluded from wall_seconds, e.g. a store
    # pass's disk write-through drain; None when nothing was excluded.
    sustained: tuple[float, float] | None = None
    # Work the backend skipped or did not do; the comparison report flags rows
    # where these are nonzero: store_drops, load_misses, disk_read_bytes.
    counts: dict = field(default_factory=dict)

    @property
    def throughput_gbs(self) -> float:
        return self.total_bytes / self.wall_seconds / 1e9 if self.wall_seconds > 0 else 0.0

    @property
    def iops(self) -> float:
        return self.total_blocks / self.wall_seconds if self.wall_seconds > 0 else 0.0

    def print_report(self):
        print(f"\n{'='*70}")
        print(f"  {self.label}")
        print(f"{'='*70}")
        print(f"  blocks={self.total_blocks}  "
              f"throughput={self.throughput_gbs:.2f} GB/s  "
              f"iops={self.iops:.0f} blk/s  "
              f"wall={self.wall_seconds:.3f}s")
        print(f"  Phase breakdown:")
        for name, timing in self.phases.items():
            if timing.samples:
                print(f"    {name:20s}  p50={timing.p50_us:8.1f}us  "
                      f"p99={timing.p99_us:8.1f}us  "
                      f"mean={timing.mean_us:8.1f}us  "
                      f"n={len(timing.samples)}")

    def csv_row(self, extra: dict | None = None) -> dict:
        row = {
            "label": self.label,
            "total_blocks": self.total_blocks,
            "total_bytes": self.total_bytes,
            "wall_seconds": f"{self.wall_seconds:.6f}",
            "throughput_gbs": f"{self.throughput_gbs:.4f}",
            "iops": f"{self.iops:.1f}",
            "sustained_gbs": f"{self.sustained[0]:.4f}" if self.sustained else "",
        }
        row.update({k: str(v) for k, v in self.counts.items()})
        for name, timing in self.phases.items():
            row[f"{name}_p50_us"] = f"{timing.p50_us:.1f}"
            row[f"{name}_p99_us"] = f"{timing.p99_us:.1f}"
            row[f"{name}_mean_us"] = f"{timing.mean_us:.1f}"
        if extra:
            row.update(extra)
        return row


def _phases(*names: str) -> dict[str, PhaseTiming]:
    return {n: PhaseTiming(n) for n in names}


def _rec(phases: dict | None, name: str, t0: float):
    if phases is not None:
        phases.setdefault(name, PhaseTiming(name)).record(time.perf_counter() - t0)


# ── offloading config (shared by the scheduler side and every TP rank) ──────

def build_offloading_config(args, block_bytes: int, fs_root: str | None, engine_id: str,
                            *, rank: int, world_size: int):
    """The OffloadingConfig vLLM would build for this run.

    ``block_bytes`` is one worker's KV bytes per block (``worker_kv_bytes_per_block``).
    Under TP every rank gets the same config except ``parallel.rank``; the CPU
    tier holds all ranks' shards of a chunk side by side in one shared mmap
    named after ``engine_id``.
    """
    extra: dict = {
        "cpu_bytes_to_use": args.cpu_bytes,
        "eviction_policy": args.eviction_policy,
    }
    if args.spec == "tiering":
        extra["spec_name"] = "TieringOffloadingSpec"
        extra["secondary_tiers"] = []
        if fs_root is not None:
            extra["secondary_tiers"].append({
                "type": "fs",
                "root_dir": fs_root,
                "n_read_threads": args.fs_read_threads,
                "n_write_threads": args.fs_write_threads,
                "enable_kv_events": args.kv_events,
            })
    else:
        extra["spec_name"] = "CPUOffloadingSpec"

    layer_names = tuple(f"model.layers.{i}.self_attn.attn" for i in range(args.layers))
    return _mk(
        OffloadingConfig,
        groups=(_mk(OffloadingGroupConfig, tokens_per_block=args.tokens_per_block,
                    layer_names=layer_names, group_id=0),),
        worker_kv_bytes_per_block=block_bytes,
        enable_kv_cache_events=args.kv_events,
        extra_config=extra,
        engine_id=engine_id,
        model=_mk(OffloadingModelConfig, name=args.model_name, dtype="float16"),
        cache=_mk(OffloadingCacheConfig, tokens_per_hash=args.tokens_per_block,
                  blocks_per_chunk=args.blocks_per_chunk),
        parallel=_mk(OffloadingParallelConfig, rank=rank, world_size=world_size,
                     tp_size=world_size, pp_size=1, pcp_size=1, dcp_size=1,
                     data_parallel_index=0, data_parallel_size=1,
                     data_parallel_rank_local=None, is_parallelism_agnostic=True),
    )


# ── real tensor parallelism (--gpus) ────────────────────────────────────────
#
# vLLM under TP=W runs one scheduler-side manager and W CPUOffloadingWorkers,
# one per GPU, all joining the same /dev/shm region (each rank owns its slot of
# every chunk; vLLM derives the slot from the CUDA device index). The scheduler
# hands every worker the same transfer spec and a job is finished once every
# rank reports it. _CputierTPWorkerGroup reproduces that: rank 0 runs in this
# process, ranks 1..W-1 in spawned processes, behind the single-worker
# submit_store / submit_load / get_finished / shutdown surface.


def _gpu_spec_ids(block_ids) -> GPULoadStoreSpec:
    ids = list(block_ids)
    return GPULoadStoreSpec(ids, group_sizes=(len(ids),), block_indices=(0,))


def _cputier_rank_main(conn, barrier, args, block_bytes, num_gpu_blocks, fs_root,
                       engine_id, rank, world_size, gpu):
    """Entry point of one non-zero TP rank (spawned process)."""
    torch.cuda.set_device(gpu)
    _cpu_spec_mod._all_workers_barrier = barrier.wait
    try:
        config = build_offloading_config(args, block_bytes, fs_root, engine_id,
                                         rank=rank, world_size=world_size)
        spec_cls = TieringOffloadingSpec if args.spec == "tiering" else CPUOffloadingSpec
        spec = spec_cls(config)
        kv_caches, _tensors = make_kv_caches(
            gpu, args.layers, block_bytes, num_gpu_blocks * args.blocks_per_chunk,
        )
        worker = spec.get_worker(kv_caches)
    except Exception as e:  # report instead of leaving the parent waiting
        conn.send(("error", f"{type(e).__name__}: {e}"))
        raise
    conn.send(("ready", rank))
    inflight = 0
    try:
        while True:
            if conn.poll(POLL_S if inflight else 0.05):
                msg = conn.recv()
                if msg[0] == "stop":
                    break
                op, job_id, block_ids, cpu_spec = msg
                gspec = _gpu_spec_ids(block_ids)   # the same GPU blocks as rank 0's
                if op == "store":
                    ok = worker.submit_store(job_id, gspec, cpu_spec)
                else:
                    ok = worker.submit_load(job_id, cpu_spec, gspec)
                if ok:
                    inflight += 1
                else:
                    conn.send((job_id, False))
            for res in worker.get_finished():
                conn.send((res.job_id, bool(res.success)))
                inflight -= 1
    finally:
        worker.shutdown()


class _CputierTPWorkerGroup:
    """W CPUOffloadingWorkers, one per GPU, behind the single-worker interface."""

    _READY_TIMEOUT_S = 120.0

    def __init__(self, args, block_bytes, num_gpu_blocks, fs_root, engine_id, gpus, spec):
        world_size = len(gpus)
        self._world_size = world_size
        self._pending: dict[int, list] = {}  # job_id -> [remaining, ok]
        self._done: collections.deque = collections.deque()
        ctx = __import__("multiprocessing").get_context("spawn")
        # Bounded so a rank that dies before reaching it fails the run instead
        # of hanging rank 0's get_worker() forever.
        barrier = ctx.Barrier(world_size, timeout=self._READY_TIMEOUT_S)
        _cpu_spec_mod._all_workers_barrier = barrier.wait
        self._conns, self._procs = [], []
        # Start the other ranks first: under CPUOffloadingSpec every rank's
        # get_worker() waits on the barrier, which rank 0 reaches below.
        for rank in range(1, world_size):
            parent, child = ctx.Pipe()
            proc = ctx.Process(
                target=_cputier_rank_main,
                args=(child, barrier, args, block_bytes, num_gpu_blocks, fs_root,
                      engine_id, rank, world_size, gpus[rank]),
                daemon=True,
            )
            proc.start()
            self._conns.append(parent)
            self._procs.append(proc)
        try:
            self.kv_caches, self.gpu_tensors = make_kv_caches(
                gpus[0], args.layers, block_bytes,
                num_gpu_blocks * args.blocks_per_chunk,
            )
            self._rank0 = spec.get_worker(self.kv_caches)
            for rank, conn in enumerate(self._conns, start=1):
                if not conn.poll(self._READY_TIMEOUT_S):
                    raise RuntimeError(f"TP rank {rank} did not start "
                                       f"(exitcode={self._procs[rank - 1].exitcode})")
                kind, detail = conn.recv()
                if kind != "ready":
                    raise RuntimeError(f"TP rank {rank} failed to start: {detail}")
        except Exception:
            self._stop_children()
            raise

    def _submit(self, op, job_id, gpu_spec, cpu_spec):
        ids = [int(b) for b in gpu_spec.block_ids]   # every rank addresses the same blocks
        self._pending[job_id] = [self._world_size, True]
        for conn in self._conns:
            conn.send((op, job_id, ids, cpu_spec))
        if op == "store":
            ok = self._rank0.submit_store(job_id, gpu_spec, cpu_spec)
        else:
            ok = self._rank0.submit_load(job_id, cpu_spec, gpu_spec)
        if not ok:
            self._rank_done(job_id, False)
        return True

    def submit_store(self, job_id, src_spec, dst_spec):
        return self._submit("store", job_id, src_spec, dst_spec)

    def submit_load(self, job_id, src_spec, dst_spec):
        return self._submit("load", job_id, dst_spec, src_spec)

    def _rank_done(self, job_id, success):
        entry = self._pending[job_id]
        entry[0] -= 1
        entry[1] = entry[1] and success
        if entry[0] == 0:
            del self._pending[job_id]
            self._done.append((job_id, entry[1]))

    def get_finished(self):
        for res in self._rank0.get_finished():
            self._rank_done(res.job_id, bool(res.success))
        for conn in self._conns:
            while conn.poll():
                job_id, success = conn.recv()
                self._rank_done(job_id, success)
        out = [_GroupResult(job_id, ok) for job_id, ok in self._done]
        self._done.clear()
        return out

    def _stop_children(self):
        for conn in self._conns:
            try:
                conn.send(("stop",))
            except (BrokenPipeError, OSError):
                pass
        for proc in self._procs:
            proc.join(timeout=30)
            if proc.is_alive():
                proc.terminate()
        self._conns, self._procs = [], []

    def shutdown(self):
        self._stop_children()
        self._rank0.shutdown()


@dataclass
class _GroupResult:
    """What CputierStack.poll() reads from a finished job."""
    job_id: int
    success: bool


# ── the cputier stack + scheduler emulation ─────────────────────────────────

class CputierStack:
    """OffloadingSpec + manager + worker + fake GPU KV cache.

    Methods mirror what OffloadingConnectorScheduler / OffloadingConnectorWorker
    call. vLLM's scheduler is single-threaded, so every manager/worker call
    goes through one lock; contention mode relies on it to share the stack
    between a store and a load thread (their DMA still overlaps on the GPU,
    each transfer runs on its own CUDA stream).
    """

    _instances = itertools.count()

    def __init__(self, args, block_bytes: int, num_gpu_blocks: int, fs_root: str | None,
                 gpus: list[int] | None = None):
        gpus = gpus or [args.gpu]
        world_size = len(gpus)
        self.block_bytes = block_bytes
        self.bpc = args.blocks_per_chunk
        # Throughput counts every rank's shard: each rank moves block_bytes.
        self.chunk_bytes = block_bytes * self.bpc * world_size
        self.has_secondary = args.spec == "tiering" and fs_root is not None
        # True: touch() before a request's lookups (vLLM <= 0.20); False:
        # after every lookup pass (vLLM >= 0.21). See resolve_lookups().
        self.touch_first = args.lookup_order == TOUCH_FIRST
        self.engine_id = f"cputier-bench-{os.getpid()}-{next(self._instances)}"

        config = build_offloading_config(args, block_bytes, fs_root, self.engine_id,
                                         rank=0, world_size=world_size)
        spec_cls = TieringOffloadingSpec if args.spec == "tiering" else CPUOffloadingSpec
        self.spec = spec_cls(config)
        if self.spec.num_chunks <= 0:
            raise SystemExit(f"error: --cpu-bytes {args.cpu_bytes} holds no "
                             f"{self.spec.kv_bytes_per_chunk}-byte chunks")
        self.num_chunks = self.spec.num_chunks
        self._mmap_path = f"/dev/shm/vllm_offload_{self.engine_id}.mmap"
        atexit.register(self._unlink_mmap)

        # Scheduler side first: TieringOffloadingSpec.get_manager() creates the
        # /dev/shm region that the workers then join.
        self.manager = self.spec.get_manager()
        if world_size > 1:
            self.worker = _CputierTPWorkerGroup(
                args, block_bytes, num_gpu_blocks, fs_root, self.engine_id,
                gpus, self.spec,
            )
            self.kv_caches = self.worker.kv_caches
            self.gpu_tensors = self.worker.gpu_tensors
        else:
            self.kv_caches, self.gpu_tensors = make_kv_caches(
                args.gpu, args.layers, block_bytes, num_gpu_blocks * self.bpc,
            )
            self.worker = self.spec.get_worker(self.kv_caches)

        self.lock = threading.RLock()
        self._job_ids = itertools.count(1)
        self._req_ids = itertools.count()
        self._finished: set[int] = set()
        self.failed_jobs = 0
        self.counters = collections.Counter()

    # -- requests / scheduler steps --

    def new_request(self) -> ReqContext:
        ctx = ReqContext(req_id=f"bench-{next(self._req_ids)}")
        with self.lock:
            self.manager.on_new_request(ctx)
        return ctx

    def step_end(self, new_req_ids=()):
        with self.lock:
            self.manager.on_schedule_end(
                ScheduleEndContext(new_req_ids=new_req_ids, preempted_req_ids=()))

    def finish_request(self, ctx: ReqContext):
        """Request finished, inside a scheduler step (the step loop ends the step once
        per tick with step_end)."""
        with self.lock:
            self.manager.on_request_finished(ctx)

    def end_request(self, ctx: ReqContext):
        """Request finished, then the scheduler step that saw it ends."""
        with self.lock:
            self.manager.on_request_finished(ctx)
            self.manager.on_schedule_end(
                ScheduleEndContext(new_req_ids=(ctx.req_id,), preempted_req_ids=()))

    # -- manager --

    def touch(self, keys, ctx):
        with self.lock:
            self.manager.touch(keys, ctx)

    def lookup(self, key, ctx) -> LookupResult:
        with self.lock:
            return self.manager.lookup(key, ctx)

    def prepare_store(self, keys, ctx):
        with self.lock:
            return self.manager.prepare_store(keys, ctx)

    def complete_store(self, keys, ctx, success: bool = True):
        with self.lock:
            self.manager.complete_store(keys, ctx, success)

    def prepare_load(self, keys, ctx):
        with self.lock:
            return self.manager.prepare_load(keys, ctx)

    def complete_load(self, keys, ctx):
        with self.lock:
            self.manager.complete_load(keys, ctx)

    def take_events(self) -> list:
        with self.lock:
            events = list(self.manager.take_events())
        for ev in events:
            tier = "cpu" if ev.medium == Medium.CPU else "fs"
            self.counters[f"{tier}_{'removed' if ev.removed else 'stored'}"] += len(ev.keys)
        return events

    def evict_primary(self):
        """Empty the CPU tier. Drains in-flight fs writes first; fs data stays."""
        with self.lock:
            self.manager.reset_cache()

    # -- worker --

    def _gpu_spec(self, num_keys: int) -> GPULoadStoreSpec:
        n = num_keys * self.bpc
        return GPULoadStoreSpec(list(range(n)), group_sizes=(n,), block_indices=(0,))

    def submit_store(self, store_spec, num_keys: int) -> int:
        job_id = next(self._job_ids)
        with self.lock:
            ok = self.worker.submit_store(job_id, self._gpu_spec(num_keys), store_spec)
        assert ok, f"submit_store({job_id}) rejected"
        return job_id

    def submit_load(self, load_spec, num_keys: int) -> int:
        job_id = next(self._job_ids)
        with self.lock:
            ok = self.worker.submit_load(job_id, load_spec, self._gpu_spec(num_keys))
        assert ok, f"submit_load({job_id}) rejected"
        return job_id

    def _gpu_spec_at(self, num_keys: int, base: int) -> GPULoadStoreSpec:
        n = num_keys * self.bpc
        return GPULoadStoreSpec(list(range(base, base + n)), group_sizes=(n,), block_indices=(0,))

    def submit_store_at(self, store_spec, num_keys: int, base: int) -> int:
        """submit_store to GPU blocks base.. (the in-flight passes' slots)."""
        job_id = next(self._job_ids)
        with self.lock:
            ok = self.worker.submit_store(job_id, self._gpu_spec_at(num_keys, base), store_spec)
        assert ok, f"submit_store({job_id}) rejected"
        return job_id

    def submit_load_at(self, load_spec, num_keys: int, base: int) -> int:
        job_id = next(self._job_ids)
        with self.lock:
            ok = self.worker.submit_load(job_id, load_spec, self._gpu_spec_at(num_keys, base))
        assert ok, f"submit_load({job_id}) rejected"
        return job_id

    def poll_results(self) -> list:
        """One non-blocking get_finished() call; its TransferResults (job_id, success,
        transfer_time where the worker reports it)."""
        with self.lock:
            res = list(self.worker.get_finished())
        self.failed_jobs += sum(1 for r in res if not r.success)
        return res

    def poll(self):
        with self.lock:
            for r in self.worker.get_finished():
                self._finished.add(r.job_id)
                if not r.success:
                    self.failed_jobs += 1

    def is_done(self, job_id: int) -> bool:
        return job_id in self._finished

    def reap(self, job_id: int):
        self._finished.discard(job_id)

    def wait_jobs(self, job_ids):
        pending = set(job_ids)
        while True:
            self.poll()
            pending -= self._finished
            if not pending:
                break
            time.sleep(POLL_S)
        self._finished.difference_update(job_ids)

    # -- teardown --

    def _unlink_mmap(self):
        try:
            os.unlink(self._mmap_path)
        except OSError:
            pass

    def shutdown(self):
        for obj in (self.worker, self.manager):
            try:
                obj.shutdown()
            except Exception as e:
                print(f"  warning: {type(obj).__name__}.shutdown failed: {e}", file=sys.stderr)
        self._unlink_mmap()


# ── lifecycle building blocks ───────────────────────────────────────────────

def resolve_lookups(
    stack: CputierStack,
    keys: list[bytes],
    ctx: ReqContext,
    *,
    prefix_stop: bool = False,
    phases: dict | None = None,
    names: dict | None = None,
    timeout: float = 60.0,
) -> tuple[list[bytes], int]:
    """Per-key lookup, retried across scheduler steps until every key resolves,
    with the request's touch(keys) in the order of ``stack.touch_first``.

    The first pass is the ``lookup`` phase. A primary miss consults the fs tier:
    an async probe returns RETRY until the next on_schedule_end() flushes it,
    and a disk hit queues a promotion and returns HIT_PENDING until the
    promotion lands in the CPU tier. Re-stepping until then is ``lookup_wait``.

    touch: with ``stack.touch_first`` (vLLM <= 0.20) it runs once, before the
    first pass. Otherwise (vLLM >= 0.21) it runs after every pass, as
    get_num_new_matched_tokens calls _touch() after _lookup() even when it
    defers the request on RETRY. Only the first touch is the ``touch`` phase;
    later ones fall inside ``lookup_wait``.

    With ``prefix_stop`` it behaves like vLLM's maximal-prefix lookup: the scan
    stops at the first miss. Returns (hits, misses).
    """
    n = names or {}
    status: list[bool | None] = [None] * len(keys)
    waited: set[int] = set()
    steps = 0
    if stack.touch_first:
        t0 = time.perf_counter()
        stack.touch(keys, ctx)
        _rec(phases, n.get("touch", "touch"), t0)
    t0 = time.perf_counter()
    t_wait = t0
    first = True
    while True:
        pending = False
        for i, k in enumerate(keys):
            if status[i] is not None:
                if prefix_stop and status[i] is False:
                    break
                continue
            r = stack.lookup(k, ctx)
            if r is LookupResult.HIT:
                status[i] = True
            elif r is LookupResult.MISS:
                status[i] = False
                if prefix_stop:
                    break
            else:  # RETRY / HIT_PENDING
                pending = True
                waited.add(i)
                if prefix_stop:
                    break
        if first:
            _rec(phases, n.get("lookup", "lookup"), t0)
        if not stack.touch_first:
            t_touch = time.perf_counter()
            stack.touch(keys, ctx)
            if first:
                _rec(phases, n.get("touch", "touch"), t_touch)
        if first:
            first = False
            t_wait = time.perf_counter()
        if not pending:
            break
        if time.perf_counter() - t0 > timeout:
            raise TimeoutError(f"lookups unresolved after {timeout}s ({steps} steps)")
        stack.step_end()
        steps += 1
        time.sleep(POLL_S)

    if prefix_stop:
        hits = list(itertools.takewhile(lambda ks: ks[1] is True, zip(keys, status)))
        hits = [k for k, _ in hits]
    else:
        hits = [k for k, s in zip(keys, status) if s]
    misses = sum(1 for s in status if s is False)

    if steps:
        _rec(phases, n.get("lookup_wait", "lookup_wait"), t_wait)
        stack.counters["lookup_steps"] += steps
        stack.counters["promoted"] += sum(1 for i in waited if status[i])
        # A promotion can evict a block that already resolved HIT in an
        # earlier pass; prepare_load() requires every key to be resident.
        hits = [k for k in hits if stack.lookup(k, ctx) is LookupResult.HIT]
    return hits, misses


def store_batch(
    stack: CputierStack, keys: list[bytes],
    phases: dict | None = None, names: dict | None = None,
    resident: list | None = None,
) -> tuple[int, int]:
    """One request: touch → prepare_store → submit → finish → complete_store.

    Returns (stored, dropped). Keys already in the CPU tier are neither.
    Unless the batch was dropped, all its keys are appended to ``resident``.
    """
    n = names or {}
    t0 = time.perf_counter()
    ctx = stack.new_request()
    _rec(phases, n.get("new_request", "new_request"), t0)

    t0 = time.perf_counter()
    stack.touch(keys, ctx)
    _rec(phases, n.get("touch", "touch"), t0)

    t0 = time.perf_counter()
    result = stack.prepare_store(keys, ctx)
    _rec(phases, n.get("prepare_store", "prepare_store"), t0)

    stored = 0
    if result is not None and result.keys_to_store:
        to_store = result.keys_to_store
        t0 = time.perf_counter()
        job = stack.submit_store(result.store_spec, len(to_store))
        _rec(phases, n.get("submit_store", "submit_store"), t0)

        t0 = time.perf_counter()
        stack.wait_jobs((job,))
        _rec(phases, n.get("get_finished", "get_finished"), t0)

        t0 = time.perf_counter()
        stack.complete_store(to_store, ctx, success=True)
        _rec(phases, n.get("complete_store", "complete_store"), t0)
        stored = len(to_store)

    t0 = time.perf_counter()
    stack.end_request(ctx)
    _rec(phases, n.get("schedule_end", "schedule_end"), t0)
    if result is None:
        return 0, len(keys)
    if resident is not None:
        resident.extend(keys)
    return stored, 0


def load_batch(
    stack: CputierStack, keys: list[bytes],
    phases: dict | None = None, names: dict | None = None,
) -> tuple[int, int]:
    """One request: lookup(+wait) with touch (see resolve_lookups) → prepare_load
    → submit → finish → complete_load.

    Returns (loaded, misses).
    """
    n = names or {}
    t0 = time.perf_counter()
    ctx = stack.new_request()
    _rec(phases, n.get("new_request", "new_request"), t0)

    hits, misses = resolve_lookups(stack, keys, ctx, phases=phases, names=names)

    if hits:
        t0 = time.perf_counter()
        load_spec = stack.prepare_load(hits, ctx)
        _rec(phases, n.get("prepare_load", "prepare_load"), t0)

        t0 = time.perf_counter()
        job = stack.submit_load(load_spec, len(hits))
        _rec(phases, n.get("submit_load", "submit_load"), t0)

        t0 = time.perf_counter()
        stack.wait_jobs((job,))
        _rec(phases, n.get("get_finished", "get_finished"), t0)

        t0 = time.perf_counter()
        stack.complete_load(hits, ctx)
        _rec(phases, n.get("complete_load", "complete_load"), t0)

    t0 = time.perf_counter()
    stack.end_request(ctx)
    _rec(phases, n.get("schedule_end", "schedule_end"), t0)
    return len(hits), misses


def populate_keys(stack: CputierStack, keys: list[bytes], batch_size: int) -> int:
    """Store all keys so they're available for load benchmarks."""
    dropped = 0
    for i in range(0, len(keys), batch_size):
        _, d = store_batch(stack, keys[i : i + batch_size])
        dropped += d
    if dropped:
        print(f"  warning: populate dropped {dropped}/{len(keys)} blocks (CPU tier full)")
    return dropped


# ── store lifecycle ─────────────────────────────────────────────────────────

def bench_store_lifecycle(
    stack: CputierStack, keys: list[bytes], batch_size: int, min_duration: float,
    *, label: str = "Store Lifecycle (touch → prepare_store → submit → finish → complete)",
) -> BenchResult:
    """Full store lifecycle: touch → prepare_store → submit_store → get_finished → complete_store.

    Every pass stores a fresh key set (see rekey) so each block is really
    written through to the fs tier. After a pass the CPU tier is emptied
    (``reset``), which first waits for that pass's fs write-through. The reset
    is excluded from wall time, so the headline throughput is the GPU→CPU
    store path (comparable to certus-connector-bench's DRAM store); the
    sustained figure including the fs drain is printed below it.
    """
    phases = _phases("new_request", "touch", "prepare_store", "submit_store",
                     "get_finished", "complete_store", "schedule_end", "reset")
    total_blocks = drops = 0
    excluded = 0.0
    wall_start = time.perf_counter()
    iteration = 0

    while (time.perf_counter() - wall_start - excluded) < min_duration or iteration < 2:
        pass_keys = rekey(keys, iteration)
        for i in range(0, len(pass_keys), batch_size):
            s, d = store_batch(stack, pass_keys[i : i + batch_size], phases)
            total_blocks += s
            drops += d
        iteration += 1
        t0 = time.perf_counter()
        stack.evict_primary()
        _rec(phases, "reset", t0)
        excluded += time.perf_counter() - t0

    elapsed = time.perf_counter() - wall_start
    result = BenchResult(
        label=label,
        total_blocks=total_blocks,
        total_bytes=total_blocks * stack.chunk_bytes,
        wall_seconds=elapsed - excluded,
        phases=phases,
    )
    result.sustained = (result.total_bytes / elapsed / 1e9, elapsed)
    result.counts = {"store_drops": drops}
    result.print_report()
    if stack.has_secondary:
        print(f"  Sustained incl. fs write-through drain: "
              f"{result.total_bytes / elapsed / 1e9:.2f} GB/s over {elapsed:.3f}s")
    if drops:
        print(f"  Store drops (CPU tier full / pinned by fs write-through): {drops}")
    return result


# ── load lifecycle ──────────────────────────────────────────────────────────

def bench_load_lifecycle(
    stack: CputierStack, keys: list[bytes], batch_size: int, min_duration: float,
    *, cold: bool = False, label: str | None = None,
) -> BenchResult:
    """Full load lifecycle: lookup → touch (touch → lookup with --lookup-order
    touch-first) → prepare_load → submit_load → get_finished → complete_load.

    Warm: keys resident in the CPU tier (pinned host RAM → GPU).
    Cold: the CPU tier is emptied before every pass, so each block is found on
    the fs tier, promoted disk → CPU (``lookup_wait``), then copied to GPU.
    The empty-out (``demote``) is excluded from wall time.
    """
    label_temp = "Cold" if cold else "Warm"
    phases = _phases("new_request", "touch", "lookup", "lookup_wait", "prepare_load",
                     "submit_load", "get_finished", "complete_load", "schedule_end")
    if cold:
        phases["demote"] = PhaseTiming("demote")

    total_blocks = misses = 0
    excluded = 0.0
    promoted_before = stack.counters["promoted"]
    wall_start = time.perf_counter()
    iteration = 0

    while (time.perf_counter() - wall_start - excluded) < min_duration or iteration < 2:
        if cold:
            t0 = time.perf_counter()
            stack.evict_primary()
            _rec(phases, "demote", t0)
            excluded += time.perf_counter() - t0

        for i in range(0, len(keys), batch_size):
            loaded, m = load_batch(stack, keys[i : i + batch_size], phases)
            total_blocks += loaded
            misses += m
        iteration += 1

    wall = time.perf_counter() - wall_start - excluded
    result = BenchResult(
        label=label or (f"{label_temp} Load Lifecycle "
                         f"({'touch → lookup' if stack.touch_first else 'lookup → touch'} "
                         f"→ prepare → submit → finish → complete)"),
        total_blocks=total_blocks,
        total_bytes=total_blocks * stack.chunk_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.counts = {"load_misses": misses}
    if cold:
        result.counts["disk_read_bytes"] = (stack.counters["promoted"] - promoted_before) * stack.chunk_bytes
    result.print_report()
    if misses or cold:
        print(f"  Load misses: {misses}  "
              f"Promoted from fs: {stack.counters['promoted'] - promoted_before}")
    return result


# ── mixed store+load under eviction pressure ───────────────────────────────

def bench_mixed_eviction(
    stack: CputierStack, batch_size: int, min_duration: float,
    *, working_set: int = 512, load_fraction: float = 0.5,
) -> BenchResult:
    """Interleaved store + load under CPU-tier pressure.

    New blocks are continuously stored (pushing the LRU evictor) while the
    previous generation's blocks are loaded back. Once the CPU tier is full,
    evicted blocks come back from the fs tier (promotion) or miss (CPU-only).
    Every store also cascades to the fs tier, whose in-flight writes pin CPU
    chunks; if the disk falls behind, prepare_store fails (store drops) and the
    loop backs off one engine step. Only blocks that were actually stored are
    loaded by the next generation. ``take_events()`` is drained each generation.

    Before the timed loop the CPU tier is filled with other blocks, untimed, so every
    timed store evicts from the first generation on (as certus-connector-bench fills
    its tier to the eviction threshold): with a 12 GiB tier a 512-block working set
    would otherwise not fill it within the pass. The fill is evicted by the pass and
    the tier emptied after it.
    """
    names_store = {"touch": "store_touch", "get_finished": "store_finish",
                   "new_request": "store_new_request", "schedule_end": "store_schedule_end"}
    names_load = {"touch": "load_touch", "get_finished": "load_finish",
                  "new_request": "load_new_request", "schedule_end": "load_schedule_end"}
    phases = _phases(
        "store_touch", "prepare_store", "submit_store", "store_finish", "complete_store",
        "load_touch", "lookup", "lookup_wait", "prepare_load", "submit_load",
        "load_finish", "complete_load", "take_events",
    )

    load_count = max(1, int(working_set * load_fraction))
    store_count = working_set - load_count

    print(f"\n  Eviction mode: working_set={working_set} "
          f"(store={store_count}/gen, load={load_count}/gen, "
          f"chunk_bytes={stack.chunk_bytes})")
    stack.evict_primary()
    stack.take_events()
    base_seed = _fresh_seed()
    # Fill the CPU tier (its LRU evicts once no chunk is free), untimed, leaving room for
    # the seed generation. Its seed is negative, apart from every generation's
    # (base_seed + 1e6 x generation).
    fill = make_content_keys(max(0, stack.num_chunks - load_count), seed=-base_seed)
    t0 = time.perf_counter()
    try:
        populate_keys(stack, fill, batch_size)
        stack.take_events()
        print(f"  CPU tier ({stack.num_chunks} chunks) filled first ({len(fill)} blocks and the "
              f"seed, {time.perf_counter() - t0:.1f}s, untimed): every store evicts.")
        # The seed: only what was stored is loaded (a dropped seed block is not a miss).
        seed_keys = make_content_keys(load_count, seed=base_seed)
        loadable_keys: list[bytes] = []
        for i in range(0, load_count, batch_size):
            store_batch(stack, seed_keys[i:i + batch_size], resident=loadable_keys)
        counters_before = stack.counters.copy()
        return _mixed_eviction_loop(stack, batch_size, min_duration, working_set, load_fraction,
                                    store_count, base_seed, loadable_keys, counters_before,
                                    phases, names_store, names_load)
    finally:
        stack.evict_primary()   # the fill and the pass's blocks leave the CPU tier


def _mixed_eviction_loop(stack, batch_size, min_duration, working_set, load_fraction,
                         store_count, base_seed, loadable_keys, counters_before,
                         phases, names_store, names_load) -> BenchResult:
    """The timed part of bench_mixed_eviction (unchanged by the fill)."""
    total_blocks = store_drops = load_misses = 0
    wall_start = time.perf_counter()
    generation = 0

    while (time.perf_counter() - wall_start) < min_duration or generation < 3:
        generation += 1

        new_keys = make_content_keys(store_count, seed=base_seed + 1_000_000 * generation)
        stored_keys: list[bytes] = []
        for i in range(0, len(new_keys), batch_size):
            s, d = store_batch(stack, new_keys[i : i + batch_size], phases, names_store,
                               resident=stored_keys)
            total_blocks += s
            store_drops += d
            if d:
                # The scheduler drops the store and moves on; the next attempt
                # comes no sooner than the next engine step.
                time.sleep(STEP_BACKOFF_S)

        t0 = time.perf_counter()
        stack.take_events()
        _rec(phases, "take_events", t0)

        for i in range(0, len(loadable_keys), batch_size):
            loaded, m = load_batch(stack, loadable_keys[i : i + batch_size], phases, names_load)
            total_blocks += loaded
            load_misses += m

        # Next generation loads what this one actually stored (dropped
        # batches were never offloaded, so loading them is not a miss).
        loadable_keys = stored_keys

    wall = time.perf_counter() - wall_start
    stack.take_events()
    delta = stack.counters - counters_before

    result = BenchResult(
        label=(
            f"Mixed Store+Load Under Eviction Pressure "
            f"(ws={working_set}, load_frac={load_fraction:.0%})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * stack.chunk_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.counts = {"store_drops": store_drops, "load_misses": load_misses,
                     "disk_read_bytes": delta["promoted"] * stack.chunk_bytes}
    result.print_report()
    print(f"  CPU evictions (REMOVED): {delta['cpu_removed']}  "
          f"Store drops (tier full): {store_drops}  "
          f"Load misses: {load_misses}")
    if stack.has_secondary:
        print(f"  fs tier: writes={delta['fs_stored']} "
              f"({delta['fs_stored'] * stack.chunk_bytes / 1e6:.0f} MB)  "
              f"promotions={delta['promoted']} "
              f"({delta['promoted'] * stack.chunk_bytes / 1e6:.0f} MB)")
        if store_drops and not delta["fs_stored"]:
            print("  NOTE: store drops with zero fs writes means the CPU->fs cascade")
            print("  is not completing; check --fs-root is writable.")
    return result


# ── mode: prefix-miss ────────────────────────────────────────────────────────

def bench_prefix_miss(
    stack: CputierStack, batch_size: int, min_duration: float,
    *, hit_ratio: float = 0.75, num_blocks: int = 128,
) -> BenchResult:
    """Measure the cost of vLLM's maximal-prefix lookup at varying hit ratios.

    Pre-stores ``hit_ratio`` of the keys, then repeatedly runs a per-key lookup
    breaking at the first miss → touch(batch) → load the prefix (touch(batch)
    first with --lookup-order touch-first). With an fs
    tier, a key absent from the CPU tier is not a MISS straight away: the
    async disk probe returns RETRY for one scheduler step first, so
    ``lookup_miss`` includes that round-trip.
    """
    all_keys = make_content_keys(num_blocks, seed=_fresh_seed())
    present_count = max(0, min(num_blocks, int(num_blocks * hit_ratio)))
    present_keys = all_keys[:present_count]
    if present_keys:
        populate_keys(stack, present_keys, batch_size)

    phases = _phases("touch", "lookup_hit", "lookup_miss", "prefix_scan", "load")

    total_scans = total_prefix_hits = total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for batch_start in range(0, num_blocks, batch_size):
            batch = all_keys[batch_start : batch_start + batch_size]
            ctx = stack.new_request()

            if stack.touch_first:
                t0 = time.perf_counter()
                stack.touch(batch, ctx)
                _rec(phases, "touch", t0)

            # Maximal prefix scan: per-key lookup, break at first miss; a
            # RETRY/HIT_PENDING key is re-asked after a scheduler step.
            t_scan = time.perf_counter()
            prefix_hits = []
            for k in batch:
                t_lk = time.perf_counter()
                while True:
                    r = stack.lookup(k, ctx)
                    if r is LookupResult.HIT or r is LookupResult.MISS:
                        break
                    stack.step_end()
                    time.sleep(POLL_S)
                if r is LookupResult.HIT:
                    _rec(phases, "lookup_hit", t_lk)
                    prefix_hits.append(k)
                else:
                    _rec(phases, "lookup_miss", t_lk)
                    break
            _rec(phases, "prefix_scan", t_scan)

            # vLLM >= 0.21 touches the request's keys after its lookups.
            if not stack.touch_first:
                t0 = time.perf_counter()
                stack.touch(batch, ctx)
                _rec(phases, "touch", t0)

            total_scans += 1
            total_prefix_hits += len(prefix_hits)

            if prefix_hits:
                t0 = time.perf_counter()
                load_spec = stack.prepare_load(prefix_hits, ctx)
                job = stack.submit_load(load_spec, len(prefix_hits))
                stack.wait_jobs((job,))
                stack.complete_load(prefix_hits, ctx)
                _rec(phases, "load", t0)
                total_blocks += len(prefix_hits)

            stack.end_request(ctx)

        iteration += 1

    wall = time.perf_counter() - wall_start
    avg_prefix = total_prefix_hits / total_scans if total_scans else 0
    result = BenchResult(
        label=(
            f"Prefix-Miss Scan (hit_ratio={hit_ratio:.0%}, "
            f"avg_prefix={avg_prefix:.1f}/{batch_size})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * stack.chunk_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.print_report()
    print(f"  Scans={total_scans}  avg_prefix_len={avg_prefix:.1f}  "
          f"lookup_hits={len(phases['lookup_hit'].samples)}  "
          f"lookup_misses={len(phases['lookup_miss'].samples)}")
    return result


# ── default mode and contention: jobs in flight (vLLM 0.30's step loop) ────────
#
# As certus-connector-bench's default and contention modes: up to --inflight jobs of
# --job-blocks keys outstanding (stores: --store-job-blocks x --store-inflight),
# submitted without waiting. Each tick: submit the stores prepared last tick, start
# loads into free slots, make one non-blocking get_finished() poll, end the scheduler
# step (on_schedule_end, which flushes the fs tier's async probes and promotions), then
# sleep to the next --step-us boundary. This is vLLM's own async model for this
# connector: OffloadingConnectorScheduler/Worker submit and reap the same way.
#
# A load job's lookups run in the tick it is scheduled. A key that answers RETRY or
# HIT_PENDING (async fs probe, fs->CPU promotion) parks the job; it is looked up again
# on a later tick, after on_schedule_end, as vLLM defers the request to its next step
# (phase lookup_wait: first lookup to the tick it resolved). Then touch, prepare_load
# and submit_load. Stores: new_request, touch, prepare_store in one tick, submitted the
# next. Each job has its own GPU block range (slot x job blocks x --blocks-per-chunk).
#
# CPUOffloadingWorker chains its transfers per direction on the GPU (each transfer's
# stream waits for the previous one's end event), so several jobs in flight overlap
# only their host work (lookups, prepare, submit) with a running copy, and a load and a
# store overlap each other. A small gain from --inflight is a property of the connector.
#
# Phases: get_finished = each job's submit-to-reap time (load_job / store_job repeat it
# under certus-connector-bench's names), reap_call = the one poll per tick,
# *_transfer = the worker's TransferResult.transfer_time.


def step_idle(step_us: float, t_tick: float) -> None:
    """End of one tick: time.sleep(0) at --step-us 0, else sleep to the boundary."""
    if step_us <= 0:
        time.sleep(0)
        return
    rest = t_tick + step_us / 1e6 - time.perf_counter()
    if rest > 0:
        time.sleep(rest)


class StepLoop:
    """Load and store jobs in flight on one CputierStack, each in its own GPU slot."""

    def __init__(self, stack: CputierStack, phases: dict, *, load_slots=0, load_job_blocks=0,
                 store_slots=0, store_job_blocks=0, lookup_timeout=60.0):
        self.stack, self.phases = stack, phases
        self.load_job_blocks, self.store_job_blocks = load_job_blocks, store_job_blocks
        bpc = stack.bpc
        self.free_load = list(range(load_slots))
        self.free_store = list(range(store_slots))
        self.load_base = lambda slot: slot * load_job_blocks * bpc
        self.store_base = lambda slot: (load_slots * load_job_blocks + slot * store_job_blocks) * bpc
        self.parked: list[dict] = []          # load jobs waiting on RETRY / HIT_PENDING
        self.deferred: list[tuple] = []       # prepared stores: (ctx, keys, spec, slot, tag)
        self.pending: dict[int, tuple] = {}   # job id -> (kind, ctx, keys, slot, t_submit, tag)
        self.load_job = PhaseTiming("load_job")
        self.store_job = PhaseTiming("store_job")
        self.load_transfer = PhaseTiming("load_transfer")
        self.store_transfer = PhaseTiming("store_transfer")
        self.loaded = self.stored = self.load_misses = self.store_drops = 0
        self.lookup_timeout = lookup_timeout
        self.n_load = self.n_store = 0        # jobs holding a slot (parked/deferred included)
        self._released: set = set()

    # -- loads --
    def start_load(self, keys, tag=None) -> None:
        """Take a slot and run the job's first lookup pass (resolved later if parked)."""
        slot = self.free_load.pop(0)
        self.n_load += 1
        st = self.stack
        t0 = time.perf_counter()
        ctx = st.new_request()
        _rec(self.phases, "new_request", t0)
        job = {"ctx": ctx, "keys": keys, "slot": slot, "tag": tag,
               "status": [None] * len(keys), "t0": time.perf_counter(), "first": True}
        if st.touch_first:
            t0 = time.perf_counter()
            st.touch(keys, ctx)
            _rec(self.phases, "touch", t0)
        self._lookup_pass(job)

    def _lookup_pass(self, job) -> None:
        st, keys, status = self.stack, job["keys"], job["status"]
        t0 = time.perf_counter()
        pending = False
        for i, k in enumerate(keys):
            if status[i] is not None:
                continue
            r = st.lookup(k, job["ctx"])
            if r is LookupResult.HIT:
                status[i] = True
            elif r is LookupResult.MISS:
                status[i] = False
            else:                              # RETRY / HIT_PENDING: next tick
                pending = True
                job["waited"] = True
        if job["first"]:
            _rec(self.phases, "lookup", t0)
        if not st.touch_first:                 # vLLM >= 0.21: touch after every pass
            t1 = time.perf_counter()
            st.touch(keys, job["ctx"])
            if job["first"]:
                _rec(self.phases, "touch", t1)
        if job["first"]:
            job["first"] = False
            job["t_wait"] = time.perf_counter()
        if pending:
            if time.perf_counter() - job["t0"] > self.lookup_timeout:
                raise TimeoutError(f"lookups unresolved after {self.lookup_timeout}s")
            self.parked.append(job)
            return
        if job.get("waited"):
            # A promotion can evict a key that resolved HIT in an earlier tick, and
            # prepare_load needs every key resident now: look the earlier hits up again.
            # One that is no longer HIT goes back to unresolved and the job parks again
            # (RETRY / HIT_PENDING) or counts it as missed (MISS).
            again = False
            for i, k in enumerate(keys):
                if status[i]:
                    r = st.lookup(k, job["ctx"])
                    if r is LookupResult.MISS:
                        status[i] = False
                    elif r is not LookupResult.HIT:
                        status[i] = None
                        again = True
            if again:
                if time.perf_counter() - job["t0"] > self.lookup_timeout:
                    raise TimeoutError(f"lookups unresolved after {self.lookup_timeout}s")
                self.parked.append(job)
                return
            _rec(self.phases, "lookup_wait", job["t_wait"])
            st.counters["promoted"] += sum(1 for s in status if s)
        hits = [k for k, s in zip(keys, status) if s]
        self.load_misses += len(keys) - len(hits)
        if not hits:
            self._release_load(job["slot"], job["tag"])
            st.finish_request(job["ctx"])
            return
        t0 = time.perf_counter()
        spec = st.prepare_load(hits, job["ctx"])
        _rec(self.phases, "prepare_load", t0)
        t0 = time.perf_counter()
        jid = st.submit_load_at(spec, len(hits), self.load_base(job["slot"]))
        _rec(self.phases, "submit_load", t0)
        self.pending[jid] = ("load", job["ctx"], hits, job["slot"], time.perf_counter(), job["tag"])

    def retry_parked(self) -> None:
        jobs, self.parked = self.parked, []
        for job in jobs:
            self._lookup_pass(job)

    def _release_load(self, slot, tag=None) -> None:
        self.free_load.append(slot)
        self.n_load -= 1
        if tag is not None:
            self._released.add(tag)

    def released_loads(self) -> set:
        """Tags of load jobs ended without a transfer (every key missed) since last call."""
        out, self._released = self._released, set()
        return out

    # -- stores --
    def prepare_store(self, keys, tag=None) -> bool:
        st = self.stack
        t0 = time.perf_counter()
        ctx = st.new_request()
        _rec(self.phases, "new_request", t0)
        t0 = time.perf_counter()
        st.touch(keys, ctx)
        _rec(self.phases, "store_touch" if "store_touch" in self.phases else "touch", t0)
        t0 = time.perf_counter()
        res = st.prepare_store(keys, ctx)
        _rec(self.phases, "prepare_store", t0)
        if res is None:
            self.store_drops += len(keys)
            st.finish_request(ctx)
            return False
        if not res.keys_to_store:          # already resident: nothing to send
            st.finish_request(ctx)
            return True
        self.deferred.append((ctx, list(res.keys_to_store), res.store_spec,
                              self.free_store.pop(0), tag))
        self.n_store += 1
        return True

    def submit_deferred(self) -> None:
        st = self.stack
        while self.deferred:
            ctx, keys, spec, slot, tag = self.deferred.pop(0)
            t0 = time.perf_counter()
            jid = st.submit_store_at(spec, len(keys), self.store_base(slot))
            _rec(self.phases, "submit_store", t0)
            self.pending[jid] = ("store", ctx, keys, slot, time.perf_counter(), tag)

    # -- reaping --
    def reap(self) -> list:
        st = self.stack
        t0 = time.perf_counter()
        results = st.poll_results()
        _rec(self.phases, "reap_call", t0)
        now = time.perf_counter()
        out, failed = [], []
        for res in results:
            if res.job_id not in self.pending:
                continue
            kind, ctx, keys, slot, t_sub, tag = self.pending.pop(res.job_id)
            tt = getattr(res, "transfer_time", None)
            if kind == "load":
                self.load_job.record(now - t_sub)
                if tt is not None:
                    self.load_transfer.record(tt)
                t1 = time.perf_counter()
                st.complete_load(keys, ctx)
                _rec(self.phases, "complete_load", t1)
                self._release_load(slot)
                if res.success:
                    self.loaded += len(keys)
            else:
                self.store_job.record(now - t_sub)
                if tt is not None:
                    self.store_transfer.record(tt)
                t1 = time.perf_counter()
                st.complete_store(keys, ctx, success=bool(res.success))
                _rec(self.phases, "complete_store", t1)
                self.free_store.append(slot)
                self.n_store -= 1
                if res.success:
                    self.stored += len(keys)
            st.finish_request(ctx)
            if not res.success:
                failed.append(res.job_id)
            out.append((kind, keys, tag))
        if failed:
            raise RuntimeError(f"in-flight transfer job(s) {failed} failed")
        return out

    def tick_end(self) -> None:
        """The scheduler step's end, once per tick (vLLM calls on_schedule_end every step)."""
        t0 = time.perf_counter()
        self.stack.step_end()
        _rec(self.phases, "schedule_end", t0)

    def outstanding(self) -> bool:
        return bool(self.pending or self.deferred or self.parked)

    def abort(self) -> None:
        """On an error: unreserve prepared stores, drain what is in flight."""
        st = self.stack
        for ctx, keys, _spec, _slot, _tag in self.deferred:
            try:
                st.complete_store(keys, ctx, success=False)
                st.finish_request(ctx)
            except Exception:  # noqa: BLE001 - best effort while failing
                pass
        self.deferred = []
        for job in self.parked:
            try:
                st.finish_request(job["ctx"])
            except Exception:  # noqa: BLE001
                pass
        self.parked = []
        deadline = time.monotonic() + 120.0
        while self.pending and time.monotonic() < deadline:
            for res in st.poll_results():
                item = self.pending.pop(res.job_id, None)
                if item is None:
                    continue
                kind, ctx, keys = item[0], item[1], item[2]
                try:
                    if kind == "load":
                        st.complete_load(keys, ctx)
                    else:
                        st.complete_store(keys, ctx, success=bool(res.success))
                    st.finish_request(ctx)
                except Exception:  # noqa: BLE001
                    pass
            time.sleep(0)


def job_units(keys: list[bytes], job_blocks: int) -> list[list[bytes]]:
    return [keys[i:i + job_blocks] for i in range(0, len(keys), job_blocks)]


def inflight_label(args, kind: str) -> str:
    if kind == "contention":
        s = (f"load {args.job_blocks}x{args.inflight} "
             f"store {args.store_job_blocks}x{args.store_inflight}")
    elif kind == "store":
        s = f"job_blocks={args.store_job_blocks} inflight={args.store_inflight}"
    else:
        s = f"job_blocks={args.job_blocks} inflight={args.inflight}"
    return s + f" step_us={args.step_us:g} layers={args.layers}"


def _run_loads(loop: StepLoop, units, args, rng, *, min_duration: float, min_passes: int = 2,
               before_pass=None, phases=None):
    """Load passes over ``units`` (shuffled per pass) until --min-duration and
    ``min_passes``; ``before_pass`` runs untimed before each pass. Returns timed seconds."""
    timed, passes = 0.0, 0
    while timed < min_duration or passes < min_passes:
        if before_pass is not None:
            before_pass()
        t_pass = time.perf_counter()
        todo = list(units)
        rng.shuffle(todo)
        try:
            while todo or loop.outstanding():
                t = time.perf_counter()
                loop.retry_parked()
                while todo and loop.free_load:
                    loop.start_load(todo.pop(0))
                loop.reap()
                loop.tick_end()
                if todo or loop.outstanding():
                    step_idle(args.step_us, t)
        except BaseException:
            loop.abort()
            raise
        timed += time.perf_counter() - t_pass
        passes += 1
    return timed, passes


def _finish_load_phases(phases, loop):
    phases["get_finished"] = loop.load_job
    phases["load_job"] = loop.load_job
    phases["load_transfer"] = loop.load_transfer


def bench_store_inflight(stack: CputierStack, keys, args, min_duration) -> BenchResult:
    """Store with jobs in flight: every pass stores a fresh key set (rekey, so each block
    is really written through to the fs tier) as --store-job-blocks jobs with up to
    --store-inflight outstanding. After a pass the CPU tier is emptied (``reset``, which
    first waits for the pass's fs write-through), excluded from wall time, so the
    headline is the GPU->CPU store path; the sustained figure includes the drains."""
    phases = _phases("new_request", "touch", "prepare_store", "submit_store",
                     "get_finished", "complete_store", "schedule_end", "reset", "reap_call")
    loop = StepLoop(stack, phases, store_slots=args.store_inflight,
                    store_job_blocks=args.store_job_blocks)
    timed, passes = 0.0, 0
    t_all = time.perf_counter()
    while timed < min_duration or passes < 2:
        t_pass = time.perf_counter()    # fresh keys inside the window, as the old pass had
        todo = job_units(rekey(keys, passes), args.store_job_blocks)
        try:
            while todo or loop.outstanding():
                t = time.perf_counter()
                loop.submit_deferred()
                while todo and loop.free_store:
                    if not loop.prepare_store(todo.pop(0)):
                        break                  # tier full: retried no sooner than next tick
                loop.reap()
                loop.tick_end()
                if todo or loop.outstanding():
                    step_idle(args.step_us, t)
        except BaseException:
            loop.abort()
            raise
        timed += time.perf_counter() - t_pass
        passes += 1
        t0 = time.perf_counter()
        stack.evict_primary()
        _rec(phases, "reset", t0)
    elapsed = time.perf_counter() - t_all
    phases["get_finished"] = loop.store_job
    phases["store_job"] = loop.store_job
    phases["store_transfer"] = loop.store_transfer
    result = BenchResult(
        label=f"Store Lifecycle ({inflight_label(args, 'store')})",
        total_blocks=loop.stored, total_bytes=loop.stored * stack.chunk_bytes,
        wall_seconds=timed, phases=phases)
    result.sustained = (result.total_bytes / elapsed / 1e9, elapsed)
    result.counts = {"store_drops": loop.store_drops}
    result.print_report()
    print(f"  store jobs={len(loop.store_job.samples)}  passes={passes}")
    if stack.has_secondary:
        print(f"  Sustained incl. fs write-through drain: "
              f"{result.sustained[0]:.2f} GB/s over {elapsed:.3f}s")
    if loop.store_drops:
        print(f"  Store drops (CPU tier full / pinned by fs write-through): {loop.store_drops}")
    return result


def bench_load_inflight(stack: CputierStack, keys, args, min_duration, *, cold: bool) -> BenchResult:
    """Load with jobs in flight: ``keys`` (stored, resident for warm) as --job-blocks jobs
    with up to --inflight outstanding, shuffled per pass. Warm: pinned host RAM -> GPU.
    Cold: the CPU tier is emptied before every pass (``demote``, untimed), so every key
    is found on the fs tier and promoted disk -> CPU across scheduler steps
    (``lookup_wait``), then copied to the GPU."""
    import random as _random
    phases = _phases("new_request", "touch", "lookup", "lookup_wait", "prepare_load",
                     "submit_load", "get_finished", "complete_load", "schedule_end", "reap_call")
    if cold:
        phases["demote"] = PhaseTiming("demote")
    loop = StepLoop(stack, phases, load_slots=args.inflight, load_job_blocks=args.job_blocks)
    promoted_before = stack.counters["promoted"]

    def demote():
        t0 = time.perf_counter()
        stack.evict_primary()
        _rec(phases, "demote", t0)

    timed, passes = _run_loads(loop, job_units(keys, args.job_blocks), args,
                               _random.Random(args.seed), min_duration=min_duration,
                               before_pass=demote if cold else None)
    _finish_load_phases(phases, loop)
    temp = "Cold" if cold else "Warm"
    result = BenchResult(
        label=f"{temp} Load Lifecycle ({inflight_label(args, 'load')})",
        total_blocks=loop.loaded, total_bytes=loop.loaded * stack.chunk_bytes,
        wall_seconds=timed, phases=phases)
    result.counts = {"load_misses": loop.load_misses}
    promoted = stack.counters["promoted"] - promoted_before
    if cold:
        result.counts["disk_read_bytes"] = promoted * stack.chunk_bytes
    result.print_report()
    print(f"  load jobs={len(loop.load_job.samples)}  passes={passes}")
    if loop.load_misses or cold:
        print(f"  Load misses: {loop.load_misses}  Promoted from fs: {promoted}")
    return result


# The share of the CPU tier the warm working set may take (certus-connector-bench's
# INFLIGHT_TIER_SHARE).
INFLIGHT_TIER_SHARE = 0.9
# --num-blocks when not given, as certus-connector-bench derives it: for default and
# contention the working set is INFLIGHT_TIER_SHARE of what the tier holds before it
# evicts (--cpu-bytes x EVICTION_THRESHOLD / --block-bytes), at most INFLIGHT_MAX_BLOCKS
# (4000 at 12G and 2 MiB blocks, about 2948 at 8G); the other modes keep 128.
INFLIGHT_MAX_BLOCKS = 4000
EVICTION_THRESHOLD = 0.8      # certus-server's --memory-tier-eviction-threshold default here
SEQUENTIAL_NUM_BLOCKS = 128


def default_num_blocks(args, mode: str) -> int:
    if mode in ("default", "contention"):
        usable = int(args.cpu_bytes * EVICTION_THRESHOLD // args.block_bytes)
        return max(1, min(INFLIGHT_MAX_BLOCKS, int(usable * INFLIGHT_TIER_SHARE)))
    return SEQUENTIAL_NUM_BLOCKS
# Contention's store side when not given, as certus-connector-bench's CONTENTION_STORE_DEFAULTS.
CONTENTION_STORE_DEFAULTS = {"store_job_blocks": 32, "store_inflight": 1}


def bench_contention(stack: CputierStack, keys, args, min_duration) -> list[BenchResult]:
    """Loads and stores in flight together on one stack, as in serving, as
    certus-connector-bench's contention mode: the load working set (``keys``) is stored
    first; then for the timed window --inflight load jobs of --job-blocks keys (the working
    set, shuffled per pass) and --store-inflight store jobs of --store-job-blocks fresh
    keys run together through the step loop. Nothing is excluded: the fs write-through
    and any CPU-tier eviction it forces run inside the window. A load key evicted by then
    misses (counted). One scheduler, one worker, single-threaded, as vLLM's."""
    import random as _random
    store_phases = _phases("new_request", "store_touch", "prepare_store", "submit_store",
                           "complete_store")
    load_phases = _phases("touch", "lookup", "lookup_wait", "prepare_load", "submit_load",
                          "complete_load")
    phases = {**store_phases, **load_phases, "schedule_end": PhaseTiming("schedule_end"),
              "reap_call": PhaseTiming("reap_call")}
    populate_keys(stack, keys, args.store_job_blocks)
    stack.take_events()
    removed_before = stack.counters["cpu_removed"]
    loop = StepLoop(stack, phases, load_slots=args.inflight, load_job_blocks=args.job_blocks,
                    store_slots=args.store_inflight, store_job_blocks=args.store_job_blocks)
    units = job_units(keys, args.job_blocks)
    rng = _random.Random(args.seed)
    order: list = []
    busy: set = set()
    base_seed = _fresh_seed()
    gen = passes = 0
    t_start = time.perf_counter()
    try:
        while True:
            t = time.perf_counter()
            running = not (t - t_start >= min_duration and passes >= 2 and not order)
            loop.submit_deferred()
            if running:
                loop.retry_parked()
                # As certus-connector-bench's _LoadScheduler: a job whose keys are still
                # in flight is skipped for that turn; each job tried at most once a tick.
                tried = 0
                while loop.free_load and tried < len(units):
                    if not order:
                        order = list(range(len(units)))
                        rng.shuffle(order)
                        passes += 1
                    idx = next((i for i in order if i not in busy), None)
                    if idx is None:
                        break
                    order.remove(idx)
                    tried += 1
                    busy.add(idx)
                    loop.start_load(units[idx], tag=idx)
                # One reservation attempt per free store slot per tick; a refused one
                # (tier full) is counted as dropped and tried again next tick.
                for _ in range(len(loop.free_store)):
                    gen += 1
                    loop.prepare_store(make_content_keys(args.store_job_blocks,
                                                         seed=base_seed + gen))
            else:
                loop.retry_parked()
            for kind, _keys, tag in loop.reap():
                if kind == "load":
                    busy.discard(tag)
            busy -= loop.released_loads()
            loop.tick_end()
            if not running and not loop.outstanding():
                break
            step_idle(args.step_us, t)
    except BaseException:
        loop.abort()
        raise
    wall = time.perf_counter() - t_start
    stack.take_events()
    evictions = stack.counters["cpu_removed"] - removed_before
    for k in ("touch", "lookup", "lookup_wait", "prepare_load", "submit_load", "complete_load",
              "reap_call"):
        load_phases[k] = phases[k]
    load_phases["get_finished"] = loop.load_job
    load_phases["load_transfer"] = loop.load_transfer
    for k in ("new_request", "store_touch", "prepare_store", "submit_store", "complete_store"):
        store_phases[k] = phases[k]
    store_phases["get_finished"] = loop.store_job
    store_phases["store_transfer"] = loop.store_transfer
    label = inflight_label(args, "contention")
    store_result = BenchResult(
        label=f"Contention: Store Thread ({label})",
        total_blocks=loop.stored, total_bytes=loop.stored * stack.chunk_bytes,
        wall_seconds=wall, phases=store_phases)
    store_result.counts = {"store_drops": loop.store_drops}
    load_result = BenchResult(
        label=f"Contention: Load Thread ({label})",
        total_blocks=loop.loaded, total_bytes=loop.loaded * stack.chunk_bytes,
        wall_seconds=wall, phases=load_phases)
    load_result.counts = {"load_misses": loop.load_misses}
    for r in (store_result, load_result):
        r.params = {"store_job_blocks": args.store_job_blocks,
                    "store_inflight": args.store_inflight}
    store_result.print_report()
    print(f"  Store drops: {loop.store_drops}")
    load_result.print_report()
    print(f"  Load misses: {loop.load_misses}  Evictions: {evictions}  passes: {passes}")
    return [store_result, load_result]


# ── mode: scheduler-step ─────────────────────────────────────────────────────

def bench_scheduler_step(
    stack: CputierStack, batch_size: int, min_duration: float,
    *, requests_per_step: int = 8, num_sessions: int = 32, num_blocks: int = 128,
) -> BenchResult:
    """Simulate continuous-batching scheduler steps matching production vLLM.

    Same call pattern as certus-connector-bench's scheduler-step mode, so the
    two backends compare directly:
      - Per-request maximal prefix scan (break on the first miss), then touch()
        of the request's keys (vLLM >= 0.21; touch first with --lookup-order
        touch-first). The hits are pinned right after the request's lookup,
        like the connector's update_state_after_alloc(), before later
        requests' stores can evict them.
      - Sessions share one prefix pool and diverge at geometric points, so
        short prefixes miss more (like production prefix caching).
      - Loads and stores are deduplicated across the step's requests.
      - Stores are submitted after the load drain and drained at the start of
        the next step (untimed); drain_workers measures only loads. A request
        with a store in flight finishes once that store completes, as vLLM
        delays freeing it.
      - touch_lookup, prepare_load and prepare_store are timed per step.
    """
    keys_per_request = max(4, num_blocks // requests_per_step)
    prefix_len = keys_per_request // 2
    suffix_len = keys_per_request - prefix_len

    phases = _phases("touch_lookup", "prepare_store", "prepare_load", "submit",
                     "drain_workers", "complete", "schedule_end", "take_events")

    # Build a shared prefix pool, then give each session a prefix slice of
    # geometric length. Fresh keys each run: the fs tier can't remove blocks.
    base_seed = _fresh_seed()
    max_prefix_pool = min(prefix_len * 4, num_blocks * 2)
    shared_prefix_pool = make_content_keys(max_prefix_pool, seed=base_seed)
    populate_keys(stack, shared_prefix_pool, batch_size)

    session_prefixes: list[list[bytes]] = []
    rng = random.Random(42)
    for _ in range(num_sessions):
        geo_len = max(2, min(len(shared_prefix_pool), int(rng.expovariate(1.0 / prefix_len))))
        session_prefixes.append(shared_prefix_pool[:geo_len])

    total_blocks = total_steps = total_prefix_hits = total_suffix_stores = drops = 0
    # Previous step's deferred store: (job ids, [(ctx, keys_to_store)]).
    prev_store: tuple[list[int], list[tuple[ReqContext, list[bytes]]]] | None = None
    wall_start = time.perf_counter()

    def drain_prev_store():
        nonlocal total_blocks
        job_ids, stored = prev_store
        stack.wait_jobs(job_ids)
        for ctx, keys in stored:
            stack.complete_store(keys, ctx, success=True)
            total_blocks += len(keys)
        with stack.lock:
            for ctx, _ in stored:
                stack.manager.on_request_finished(ctx)

    while (time.perf_counter() - wall_start) < min_duration or total_steps < 3:
        # Drain the PREVIOUS step's deferred store (not timed: production
        # processes store completions asynchronously between steps).
        if prev_store is not None:
            drain_prev_store()
            prev_store = None

        # (ctx, load_keys, store_keys)
        plan: list[tuple[ReqContext, list[bytes], list[bytes]]] = []
        load_specs = []
        seen_load: set[bytes] = set()
        seen_store: set[bytes] = set()
        t_touch_lookup = t_prepare_load = 0.0

        for r in range(requests_per_step):
            s_idx = (total_steps * requests_per_step + r) % num_sessions
            suffix = make_content_keys(
                suffix_len,
                seed=base_seed + s_idx * 10_000 + 5_000 + total_steps * 100 + r,
            )
            req_keys = session_prefixes[s_idx] + suffix
            ctx = stack.new_request()
            t0 = time.perf_counter()
            # Lookups and the request's touch, in --lookup-order (see resolve_lookups).
            prefix_hits, _ = resolve_lookups(stack, req_keys, ctx, prefix_stop=True)
            t_touch_lookup += time.perf_counter() - t0

            load_keys = [k for k in prefix_hits if k not in seen_load]
            seen_load.update(load_keys)
            store_keys = [k for k in req_keys[len(prefix_hits):] if k not in seen_store]
            seen_store.update(store_keys)

            t0 = time.perf_counter()
            load_specs.append(stack.prepare_load(load_keys, ctx) if load_keys else None)
            t_prepare_load += time.perf_counter() - t0
            plan.append((ctx, load_keys, store_keys))
            total_prefix_hits += len(prefix_hits)
            total_suffix_stores += len(req_keys) - len(prefix_hits)

        phases["touch_lookup"].record(t_touch_lookup)
        if any(spec is not None for spec in load_specs):
            phases["prepare_load"].record(t_prepare_load)

        store_results = []
        if any(sk for _, _, sk in plan):
            t0 = time.perf_counter()
            for ctx, _, store_keys in plan:
                res = stack.prepare_store(store_keys, ctx) if store_keys else None
                if store_keys and res is None:
                    drops += len(store_keys)
                store_results.append(res)
            _rec(phases, "prepare_store", t0)
        else:
            store_results = [None] * len(plan)

        # Submit loads only; stores are deferred to after the drain.
        t0 = time.perf_counter()
        load_jobs = [stack.submit_load(lspec, len(lk))
                     for (_, lk, _), lspec in zip(plan, load_specs) if lspec is not None]
        _rec(phases, "submit", t0)

        # Drain workers: only the loads (production behavior).
        t0 = time.perf_counter()
        stack.wait_jobs(load_jobs)
        _rec(phases, "drain_workers", t0)

        t0 = time.perf_counter()
        for ctx, lk, _ in plan:
            if lk:
                stack.complete_load(lk, ctx)
                total_blocks += len(lk)
        _rec(phases, "complete", t0)

        # Requests without a store in flight finish now; the rest finish once
        # their store completes (next step).
        stored = [(ctx, res.keys_to_store) for (ctx, _, _), res in zip(plan, store_results)
                  if res is not None and res.keys_to_store]
        storing = {id(ctx) for ctx, _ in stored}
        t0 = time.perf_counter()
        with stack.lock:
            for ctx, _, _ in plan:
                if id(ctx) not in storing:
                    stack.manager.on_request_finished(ctx)
        stack.step_end(new_req_ids=[ctx.req_id for ctx, _, _ in plan])
        _rec(phases, "schedule_end", t0)

        t0 = time.perf_counter()
        stack.take_events()
        _rec(phases, "take_events", t0)

        total_steps += 1

        # Submit stores after the drain; they are drained at the next step's start.
        if stored:
            job_ids = [stack.submit_store(res.store_spec, len(res.keys_to_store))
                       for res in store_results if res is not None and res.keys_to_store]
            prev_store = (job_ids, stored)

    # Drain the last step's deferred store.
    if prev_store is not None:
        drain_prev_store()

    wall = time.perf_counter() - wall_start
    result = BenchResult(
        label=(
            f"Scheduler Step (reqs={requests_per_step}, "
            f"sessions={num_sessions}, keys/req={keys_per_request})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * stack.chunk_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.print_report()
    print(f"  Steps={total_steps}  steps/s={total_steps/wall:.1f}  "
          f"prefix_hits={total_prefix_hits}  suffix_stores={total_suffix_stores}  "
          f"store_drops={drops}")
    return result


# ── pattern-driven mode (same YAML patterns as certus_fio) ──────────────────

def _load_pattern(name: str, overrides: dict | None = None):
    """Load a workload pattern YAML with certus_fio's WorkloadPattern.

    certus_fio imports the shmq ring helpers and libcudart at module level,
    so only its ``eval_expr`` and ``WorkloadPattern`` definitions are compiled
    here, from the certus_fio source.
    """
    import yaml

    src_path = os.path.join(_repo, "tools", "certus-fio", "certus_fio.py")
    tree = ast.parse(Path(src_path).read_text(), src_path)
    wanted = [node for node in tree.body
              if isinstance(node, (ast.FunctionDef, ast.ClassDef))
              and node.name in ("eval_expr", "WorkloadPattern")]
    ns: dict = {"math": math, "yaml": yaml, "Path": Path}
    exec(compile(ast.Module(body=wanted, type_ignores=[]), src_path, "exec"), ns)

    patterns_dir = os.path.join(_repo, "knowledge", "workload_patterns")
    path = os.path.join(patterns_dir, f"{name}.yaml")
    if not os.path.exists(path):
        candidates = [f[:-5] for f in os.listdir(patterns_dir) if f.endswith(".yaml")]
        print(f"Pattern '{name}' not found. Available:", file=sys.stderr)
        for c in sorted(candidates):
            print(f"  {c}", file=sys.stderr)
        sys.exit(1)
    return ns["WorkloadPattern"](path, overrides), ns["eval_expr"]


def bench_pattern(
    pattern, eval_expr_fn, stack: CputierStack, batch_size: int, min_duration: float,
) -> list[BenchResult]:
    """Run a certus_fio YAML pattern through the cputier connector lifecycle.

    Each pattern phase's store/load/store_3phase/load_3phase ops become the
    full manager+worker lifecycle over the pattern's integer keyspace. The
    CPU-tier geometry is fixed when the stack is built, so every keyspace uses
    the stack's block size (see main()).
    """
    results = []
    key_base = 10_000_000
    ks_names = list(pattern.keyspaces.keys())

    for phase_def in pattern.phases:
        phase_id = phase_def["id"]
        for op_def in phase_def.get("operations", []):
            op = op_def["op"]
            ks_name = op_def.get("keys", ks_names[0])
            ks = pattern.keyspaces[ks_name]
            cardinality = ks["cardinality"]
            repeat = eval_expr_fn(op_def.get("repeat", cardinality), pattern.params)
            bs = min(eval_expr_fn(op_def.get("batch_size", batch_size), pattern.params), repeat)

            ks_offset = ks_names.index(ks_name) * 10_000_000
            all_ints = list(range(key_base + ks_offset, key_base + ks_offset + cardinality))
            all_keys = make_int_keys(all_ints)
            keys = list(itertools.islice(itertools.cycle(all_keys), repeat))

            if op in ("store", "store_3phase"):
                stack.evict_primary()
                results.append(bench_store_lifecycle(
                    stack, keys, bs, min_duration,
                    label=f"[{pattern.id}] {phase_id}/{op} (cputier lifecycle)",
                ))

            elif op in ("load", "load_3phase"):
                if any(pc["subject"] == ks_name and pc["state"] == "present_in_store"
                       for pc in pattern.preconditions):
                    stack.evict_primary()
                    populate_keys(stack, all_keys, bs)
                is_cold = any(
                    pc["subject"] == ks_name and pc["state"] == "absent_from_local_cache"
                    for pc in pattern.preconditions
                )
                if is_cold and not stack.has_secondary:
                    print(f"  skip {phase_id}/{op}: cold load needs the fs tier")
                    continue
                results.append(bench_load_lifecycle(
                    stack, keys, bs, min_duration, cold=is_cold,
                    label=(f"[{pattern.id}] {phase_id}/{op} "
                           f"{'cold' if is_cold else 'warm'} (cputier lifecycle)"),
                ))

        stack.evict_primary()

    return results


# ── correctness check ───────────────────────────────────────────────────────

def verify_roundtrip(stack: CputierStack, num_keys: int, *, cold: bool) -> bool:
    """Store random GPU blocks, zero them, load them back, compare bytes."""
    n = num_keys * stack.bpc
    for t in stack.gpu_tensors:
        t[:n].random_(-128, 128)
    expected = [t[:n].clone() for t in stack.gpu_tensors]
    torch.cuda.synchronize()

    keys = make_content_keys(num_keys, seed=_fresh_seed())
    stored, dropped = store_batch(stack, keys)
    if stored != num_keys:
        print(f"  verify: stored {stored}/{num_keys} (dropped {dropped})")
        return False

    for t in stack.gpu_tensors:
        t[:n].zero_()
    torch.cuda.synchronize()
    if cold:
        stack.evict_primary()

    loaded, misses = load_batch(stack, keys)
    torch.cuda.synchronize()
    if loaded != num_keys:
        print(f"  verify: loaded {loaded}/{num_keys} (misses {misses})")
        return False
    bad = sum(0 if torch.equal(t[:n], e) else 1 for t, e in zip(stack.gpu_tensors, expected))
    if bad or stack.failed_jobs:
        print(f"  verify: {bad}/{len(expected)} layer tensors differ, "
              f"{stack.failed_jobs} failed transfer jobs")
        return False
    return True


# ── main ────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="End-to-end cputier (vLLM native offload) connector lifecycle benchmark",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Modes (--mode):\n"
            "\n"
            "  default        store, warm load, cold load with --inflight jobs of --job-blocks\n"
            "                 outstanding (vLLM 0.30's step loop), then mixed eviction\n"
            "  prefix-miss    Vary prefix hit ratio to measure maximal-prefix lookup cost\n"
            "  contention     Loads and stores in flight together on one stack (step loop)\n"
            "  scheduler-step Full scheduler step simulation with multi-request prefix sharing\n"
            "\n"
            "Without --mode, runs the default mode (or --pattern if given).\n"
            "\n"
            "Exercises vLLM's TieringOffloadingManager (CPU primary + fs disk tier)\n"
            "and CPUOffloadingWorker directly, emulating the scheduler's per-request\n"
            "and per-step calls, including RETRY/HIT_PENDING lookup retries."
        ),
    )
    parser.add_argument("--pattern", type=str, default=None,
                        help="Run a certus_fio YAML pattern through the connector "
                        "(e.g. cold_prefill_store, warm_prefill_load_and_suffix_store)")
    parser.add_argument("--override", nargs="*", default=[],
                        help="Pattern parameter overrides (e.g. requested_blocks=256)")
    # cputier stack (the server flags of certus-connector-bench)
    parser.add_argument("--spec", choices=["tiering", "cpu"], default="tiering",
                        help="tiering = TieringOffloadingSpec (CPU + fs tier, the cputier "
                             "default); cpu = CPUOffloadingSpec (host RAM only)")
    parser.add_argument("--cpu-bytes", type=parse_size, default=parse_size("12G"),
                        help="CPU primary tier size, e.g. 12G (cpu_bytes_to_use; default 12G, "
                             "certus-connector-bench's --tier-gib)")
    parser.add_argument("--fs-root", type=str, default=None,
                        help="Directory for the fs disk tier. A per-run subdirectory is "
                             "created and removed at exit (see --keep-fs). Default: a "
                             "temp dir under $TMPDIR")
    parser.add_argument("--no-fs", action="store_true",
                        help="(tiering) No secondary tier: CPU tier only")
    parser.add_argument("--keep-fs", action="store_true",
                        help="Keep the fs tier block files at exit")
    parser.add_argument("--fs-read-threads", type=int, default=16)
    parser.add_argument("--fs-write-threads", type=int, default=16)
    parser.add_argument("--eviction-policy", type=str, default="lru",
                        help="CPU tier cache policy (lru, arc)")
    parser.add_argument("--layers", type=int, default=DEFAULT_LAYERS,
                        help=f"Per-layer KV tensors a block is split across "
                             f"(default: {DEFAULT_LAYERS})")
    parser.add_argument("--blocks-per-chunk", type=int, default=1,
                        help="GPU blocks per offloaded chunk (one key per chunk)")
    parser.add_argument("--tokens-per-block", type=int, default=16)
    parser.add_argument("--model-name", type=str, default="cputier-bench",
                        help="Model name recorded in the fs tier's file layout")
    parser.add_argument("--no-kv-events", dest="kv_events", action="store_false",
                        help="Disable KV cache events (take_events then yields nothing)")
    parser.add_argument("--verify", action="store_true",
                        help="Check warm (and cold) round-trip data integrity first")
    # workload (same as certus-connector-bench)
    parser.add_argument("--bs", type=int, default=16,
                        help="Batch size (blocks per batch)")
    parser.add_argument("--num-blocks", type=int, default=None,
                        help="Total blocks per iteration (default, contention: derived from "
                             "--cpu-bytes as certus-connector-bench derives it from its tier, "
                             "90%% of cpu_bytes x 0.8 / --block-bytes, at most 4000; the "
                             "other modes: 128)")
    parser.add_argument("--block-bytes", type=int, default=DEFAULT_BLOCK_BYTES,
                        help=f"Per-block size in bytes (default: {DEFAULT_BLOCK_BYTES})")
    parser.add_argument("--gpu", type=int, default=0,
                        help="CUDA device index")
    parser.add_argument("--tp", type=int, default=1,
                        help="Tensor-parallel world size: one CPUOffloadingWorker per GPU "
                             "(rank i on device --gpu+i) sharing one CPU-tier region, one "
                             "scheduler-side manager, as vLLM's --tensor-parallel-size. "
                             "--block-bytes is each rank's shard size.")
    parser.add_argument("--gpus", type=str, default=None,
                        help="Comma-separated CUDA devices for the TP ranks instead of "
                             "--gpu..--gpu+tp-1; implies --tp = the device count.")
    parser.add_argument("--min-duration", type=float, default=5.0,
                        help="Minimum seconds per benchmark phase")
    parser.add_argument("--warmup", type=int, default=4,
                        help="Warmup store/load cycles before measurement")
    parser.add_argument("--working-set", type=int, default=512,
                        help="Working set size for eviction-pressure mode (blocks)")
    parser.add_argument("--csv", type=str, default=None,
                        help="Append results to CSV file")
    parser.add_argument("--tag", type=str, default="",
                        help="Tag column for CSV (e.g. branch name)")
    parser.add_argument("--no-cold", action="store_true",
                        help="Skip the cold-load benchmark (fs→CPU→GPU)")
    parser.add_argument("--no-eviction", action="store_true",
                        help="Skip the eviction-pressure benchmark")
    parser.add_argument("--mode", type=str, default=None,
                        choices=["default", "prefix-miss", "contention", "scheduler-step", "all"],
                        help="Benchmark mode (omit for default or --pattern; "
                             "'all' runs every mode sequentially)")
    # jobs in flight (default and contention), certus-connector-bench's defaults
    parser.add_argument("--job-blocks", type=int, default=64,
                        help="(default, contention) blocks per load job, and per store job in "
                             "the default mode unless --store-job-blocks (default 64)")
    parser.add_argument("--inflight", type=int, default=4,
                        help="(default, contention) jobs kept outstanding (default 4); "
                             "prefix-miss and scheduler-step do not use it")
    parser.add_argument("--step-us", type=float, default=100.0,
                        help="(default, contention) scheduler tick in us (default 100; 0 = "
                             "time.sleep(0) each tick)")
    parser.add_argument("--store-job-blocks", type=int, default=None,
                        help="blocks per store job (default mode: --job-blocks; contention: 32)")
    parser.add_argument("--store-inflight", type=int, default=None,
                        help="store jobs outstanding (default mode: --inflight; contention: 1)")
    parser.add_argument("--seed", type=int, default=0, help="seed for the load job order")
    parser.add_argument("--hit-ratio", type=float, default=0.75,
                        help="(prefix-miss) Fraction of keys pre-stored (0.0-1.0)")
    parser.add_argument("--requests-per-step", type=int, default=8,
                        help="(scheduler-step) Requests per scheduler step")
    parser.add_argument("--num-sessions", type=int, default=32,
                        help="(scheduler-step) Distinct conversations in the pool")
    parser.add_argument("--lookup-order", choices=LOOKUP_ORDERS, default=LOOKUP_FIRST,
                        help="Order of a request's lookup() and touch() calls, in every "
                             "mode that does both: lookup-first (default) is vLLM >= 0.21 "
                             "(vLLM PR #41228, still so in 0.30); touch-first is vLLM <= 0.20")
    args = parser.parse_args()
    if args.step_us < 0:
        parser.error("--step-us must be >= 0")
    # --num-blocks per mode (default_num_blocks) unless given; args.num_blocks is the
    # in-flight modes' value (or the run's one mode's), _run_mode sets each mode's.
    args.num_blocks_given = args.num_blocks
    if args.num_blocks is None:
        # The same choice as `mode` below: no --mode is pattern under --pattern, else default.
        run_mode = args.mode or ("pattern" if args.pattern else "default")
        args.num_blocks = default_num_blocks(
            args, "default" if run_mode == "all" else run_mode)
    # Store flags given on the command line (contention under --mode all takes its own
    # defaults for the others).
    args.explicit_store = {k for k in CONTENTION_STORE_DEFAULTS if getattr(args, k) is not None}
    if args.mode == "contention":
        for k, v in CONTENTION_STORE_DEFAULTS.items():
            if getattr(args, k) is None:
                setattr(args, k, v)
    if args.store_job_blocks is None:
        args.store_job_blocks = args.job_blocks
    if args.store_inflight is None:
        args.store_inflight = args.inflight
    for name in ("job_blocks", "inflight", "store_job_blocks", "store_inflight"):
        if getattr(args, name) < 1:
            parser.error(f"--{name.replace('_', '-')} must be >= 1")
    if args.mode in (None, "default", "contention", "all") and not args.pattern:
        jobs = -(-args.num_blocks // args.job_blocks)
        if jobs < args.inflight:
            parser.error(f"--num-blocks {args.num_blocks} makes only {jobs} job(s) of --job-blocks "
                         f"{args.job_blocks}, but --inflight {args.inflight} needs at least "
                         f"{args.inflight}: use --num-blocks >= {args.inflight * args.job_blocks}, "
                         f"or a smaller --job-blocks or --inflight")

    if args.gpus:
        gpus = [int(g) for g in args.gpus.split(",")]
        if args.tp not in (1, len(gpus)):
            parser.error(f"--tp {args.tp} conflicts with --gpus {args.gpus}")
    else:
        if args.tp < 1:
            parser.error(f"--tp must be >= 1, got {args.tp}")
        gpus = list(range(args.gpu, args.gpu + args.tp))
    if len(set(gpus)) != len(gpus):
        parser.error(f"--gpus lists a device twice: {args.gpus}")
    if max(gpus) >= torch.cuda.device_count():
        parser.error(f"TP needs CUDA devices {gpus}, but only "
                     f"{torch.cuda.device_count()} are visible")
    args.tp = len(gpus)
    if len(gpus) > 1:
        # vLLM derives a worker's CPU-tier slot from current_device_index() %
        # world_size, so rank i must sit on a device whose index maps to slot i.
        bad = [g for i, g in enumerate(gpus) if g % len(gpus) != i]
        if bad:
            parser.error(f"TP devices {gpus}: rank i needs a device with index % "
                         f"{len(gpus)} == i (vLLM picks the CPU-tier slot that way)")
        args.gpu = gpus[0]

    torch.cuda.set_device(args.gpu)

    mode = args.mode
    if mode is None:
        mode = "pattern" if args.pattern else "default"

    pattern = eval_expr_fn = None
    block_bytes = args.block_bytes
    if mode == "pattern":
        overrides = {}
        for ov in args.override:
            if "=" in ov:
                k, v = ov.split("=", 1)
                overrides[k] = v
        pattern, eval_expr_fn = _load_pattern(args.pattern, overrides)
        sizes = {ks["object_bytes"] for ks in pattern.keyspaces.values()}
        if args.block_bytes == DEFAULT_BLOCK_BYTES and len(sizes) >= 1:
            block_bytes = max(sizes)
        if len(sizes) > 1 or block_bytes not in sizes:
            print(f"  note: pattern object_bytes {sorted(sizes)}; every keyspace "
                  f"uses block_bytes={block_bytes} (CPU-tier geometry is fixed)")
        while block_bytes % args.layers:
            args.layers //= 2

    # fs tier root: a per-run subdirectory, removed at exit unless --keep-fs.
    fs_root = None
    fs_run_dir = None
    if args.spec == "tiering" and not args.no_fs:
        parent = args.fs_root or tempfile.gettempdir()
        os.makedirs(parent, exist_ok=True)
        fs_run_dir = tempfile.mkdtemp(prefix=f"cputier-bench-{os.getpid()}-", dir=parent)
        fs_root = fs_run_dir
        if not args.keep_fs:
            atexit.register(shutil.rmtree, fs_run_dir, True)
            # atexit does not run on a signal: turn SIGTERM (podman stop, the
            # connector-sweep interrupt) into a normal exit so the fs tier's
            # files are removed; they run to hundreds of GB.
            signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))

    shm = shutil.disk_usage("/dev/shm")
    if args.cpu_bytes > shm.free:
        sys.exit(f"error: --cpu-bytes {args.cpu_bytes} exceeds free /dev/shm "
                 f"({shm.free}); in a container raise --shm-size")

    import vllm
    print(f"cputier Connector Lifecycle Benchmark (vLLM {vllm.__version__})")
    print(f"  spec={args.spec}  cpu_bytes={args.cpu_bytes}  "
          f"fs_root={fs_root or '(none)'}  gpus={gpus}  tp={len(gpus)}")
    print(f"  num_blocks={args.num_blocks}  bs={args.bs}  block_bytes={block_bytes}  "
          f"layers={args.layers}  blocks_per_chunk={args.blocks_per_chunk}")
    print(f"  min_duration={args.min_duration}s  "
          f"fs_threads=r{args.fs_read_threads}/w{args.fs_write_threads}  "
          f"eviction_policy={args.eviction_policy}  lookup_order={args.lookup_order}")

    # GPU KV buffers sized by the largest single transfer: every job addresses GPU
    # blocks 0..n-1 of its own batch (_gpu_spec). --working-set only sets how many
    # keys cycle through the CPU tier (in --bs batches), so sizing by it reserved
    # GPU memory no transfer touches -- 32 GiB at --working-set 16384, 2 MiB
    # blocks -- as certus-connector-bench did before 4e612f8a. A scheduler-step
    # request transfers at most its prefix (shared pool) plus its suffix.
    # scheduler-step sizes its requests by its own --num-blocks (128 unless given).
    sched_nb = args.num_blocks_given or SEQUENTIAL_NUM_BLOCKS
    kpr = max(4, sched_nb // args.requests_per_step)
    step_request = min((kpr // 2) * 4, sched_nb * 2) + (kpr - kpr // 2)
    # The in-flight passes: one slot per job in flight, loads then stores (StepLoop),
    # contention both at once; in chunks (CputierStack multiplies by --blocks-per-chunk).
    inflight_slots = (args.inflight * args.job_blocks
                      + max(args.store_inflight * args.store_job_blocks,
                            CONTENTION_STORE_DEFAULTS["store_inflight"]
                            * CONTENTION_STORE_DEFAULTS["store_job_blocks"]))
    # Not --num-blocks: that is how many keys cycle through the tier; every transfer
    # addresses GPU blocks from its own slot (a --bs batch, a scheduler-step step, or
    # one in-flight job), as certus-connector-bench's gpu_blocks() sizes them. With the
    # derived default (4000) sizing by it would reserve 7.8 GiB of unused GPU memory.
    max_blocks = max(args.bs, step_request, inflight_slots)
    if mode == "pattern":
        max_blocks = max(max_blocks, *(ks["cardinality"] for ks in pattern.keyspaces.values()))
    stack = CputierStack(args, block_bytes, max_blocks, fs_root, gpus)
    print(f"  CPU tier: {stack.num_chunks} chunks × {stack.spec.kv_bytes_per_chunk} bytes")
    print(f"  job_blocks={args.job_blocks} inflight={args.inflight} store_job_blocks="
          f"{args.store_job_blocks} store_inflight={args.store_inflight} step_us={args.step_us:g}")
    if mode in ("default", "contention", "all"):
        # A warm load is only warm while its keys stay in the CPU tier, as in
        # certus-connector-bench (which refuses more than 90% of its tier's usable size):
        # a larger set is evicted to the fs tier and promoted back, or misses.
        fit = int(stack.num_chunks * INFLIGHT_TIER_SHARE)
        need = args.num_blocks            # one key is one chunk (--blocks-per-chunk GPU blocks)
        if need > fit:
            stack.shutdown()
            sys.exit(f"error: --num-blocks {args.num_blocks} ({need} chunks) is more than "
                     f"{INFLIGHT_TIER_SHARE:.0%} of the CPU tier's {stack.num_chunks} chunks "
                     f"(--cpu-bytes {args.cpu_bytes}): the warm loads would not be warm. Use "
                     f"--num-blocks <= {fit * args.blocks_per_chunk} or a larger --cpu-bytes")

    try:
        all_results = _run(args, mode, stack, pattern, eval_expr_fn)
    finally:
        stack.shutdown()

    # ── CSV output ──
    if args.csv and all_results:
        extra = {
            "tag": args.tag,
            "mode": mode,
            "backend": "cputier",
            "spec": args.spec,
            "cpu_bytes": args.cpu_bytes,
            "fs_tier": int(fs_root is not None),
            "gpu": args.gpu,
            "gpus": ",".join(map(str, gpus)),
            "tp": len(gpus),
            "bs": args.bs,
            "num_blocks": args.num_blocks,
            "block_bytes": block_bytes,
            "layers": args.layers,
            "blocks_per_chunk": args.blocks_per_chunk,
            "fs_read_threads": args.fs_read_threads,
            "fs_write_threads": args.fs_write_threads,
            "working_set": args.working_set,
            "lookup_order": args.lookup_order,
            "vllm_version": vllm.__version__,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        }
        extra.update(job_blocks=args.job_blocks, inflight=args.inflight, step_us=args.step_us,
                     store_job_blocks=args.store_job_blocks, store_inflight=args.store_inflight)
        if mode == "prefix-miss":
            extra["hit_ratio"] = args.hit_ratio
        elif mode == "scheduler-step":
            extra["requests_per_step"] = args.requests_per_step
            extra["num_sessions"] = args.num_sessions
        elif mode == "pattern":
            extra["pattern"] = args.pattern

        # A result that ran with other job settings (contention's store side) carries them.
        rows = [r.csv_row({**extra, **getattr(r, "params", {})}) for r in all_results]
        write_header = not os.path.exists(args.csv)
        fieldnames = list(rows[0].keys())
        for r in rows[1:]:
            for k in r:
                if k not in fieldnames:
                    fieldnames.append(k)
        with open(args.csv, "a", newline="") as f:
            w = csv.DictWriter(f, fieldnames=fieldnames, extrasaction="ignore")
            if write_header:
                w.writeheader()
            w.writerows(rows)
        print(f"\nResults appended to {args.csv}")

    if fs_run_dir and args.keep_fs:
        print(f"\nfs tier files kept in {fs_run_dir}")
    print(f"\nDone.")


def _run(args, mode, stack: CputierStack, pattern, eval_expr_fn) -> list[BenchResult]:
    if args.verify:
        print("\nVerify round-trip data integrity...")
        n = min(args.bs, args.num_blocks)
        ok = verify_roundtrip(stack, n, cold=False)
        print(f"  warm (GPU→CPU→GPU): {'OK' if ok else 'FAILED'}")
        if stack.has_secondary:
            ok_cold = verify_roundtrip(stack, n, cold=True)
            print(f"  cold (GPU→CPU→fs→CPU→GPU): {'OK' if ok_cold else 'FAILED'}")
            ok = ok and ok_cold
        if not ok:
            sys.exit(1)
        stack.evict_primary()

    # ── warmup (CUDA streams/events, pinned descriptor buffers, fs threads) ──
    print(f"\nWarmup ({args.warmup} store/load cycles)...")
    warmup_keys = make_content_keys(args.warmup, seed=_fresh_seed())
    for wk in warmup_keys:
        store_batch(stack, [wk])
        load_batch(stack, [wk])
    stack.evict_primary()
    stack.take_events()

    def _run_mode(m: str) -> list[BenchResult]:
        results: list[BenchResult] = []
        if args.num_blocks_given is None:
            args.num_blocks = default_num_blocks(args, m)
        keys = make_content_keys(args.num_blocks, seed=_fresh_seed())

        if m == "pattern":
            print(f"\nPattern: {pattern.id} ({pattern.name})")
            pattern.describe()
            results = bench_pattern(pattern, eval_expr_fn, stack, args.bs, args.min_duration)

        elif m == "prefix-miss":
            print(f"\n  Mode: prefix-miss (hit_ratio={args.hit_ratio})")
            results.append(bench_prefix_miss(
                stack, args.bs, args.min_duration,
                hit_ratio=args.hit_ratio, num_blocks=args.num_blocks,
            ))

        elif m == "contention":
            ca = argparse.Namespace(**vars(args))
            for k, v in CONTENTION_STORE_DEFAULTS.items():
                if k not in args.explicit_store:
                    setattr(ca, k, v)
            print(f"\n  Mode: contention ({inflight_label(ca, 'contention')})")
            stack.evict_primary()
            results = bench_contention(stack, keys, ca, args.min_duration)

        elif m == "scheduler-step":
            print(f"\n  Mode: scheduler-step (reqs={args.requests_per_step}, "
                  f"sessions={args.num_sessions})")
            results.append(bench_scheduler_step(
                stack, args.bs, args.min_duration,
                requests_per_step=args.requests_per_step,
                num_sessions=args.num_sessions,
                num_blocks=args.num_blocks,
            ))

        else:  # default
            print(f"\n  Mode: default ({inflight_label(args, 'load')})")
            results.append(bench_store_inflight(stack, keys, args, args.min_duration))

            populate_keys(stack, keys, args.store_job_blocks)
            results.append(bench_load_inflight(stack, keys, args, args.min_duration, cold=False))

            if args.no_cold:
                pass
            elif not stack.has_secondary:
                print("\n  (cold load skipped: no fs tier; use --spec tiering without --no-fs)")
            else:
                results.append(bench_load_inflight(stack, keys, args, args.min_duration, cold=True))

            if not args.no_eviction:
                results.append(bench_mixed_eviction(
                    stack, args.bs, args.min_duration,
                    working_set=args.working_set, load_fraction=0.5,
                ))

        # Clean up between modes.
        stack.evict_primary()
        stack.take_events()
        if stack.failed_jobs:
            print(f"  WARNING: {stack.failed_jobs} transfer jobs reported failure")
        for r in results:   # the CSV records each row's own working set (--mode all)
            r.params = {**getattr(r, "params", {}), "num_blocks": args.num_blocks}
        return results

    ALL_MODES = ["default", "prefix-miss", "contention", "scheduler-step"]
    modes_to_run = ALL_MODES if mode == "all" else [mode]

    all_results: list[BenchResult] = []
    for m in modes_to_run:
        if mode == "all":
            print(f"\n{'='*70}")
            print(f"  Running mode: {m}")
            print(f"{'='*70}")
        all_results.extend(_run_mode(m))
    return all_results


if __name__ == "__main__":
    main()
