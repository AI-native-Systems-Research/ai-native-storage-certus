#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""End-to-end connector-path benchmark: full vLLM offloading lifecycle.

Unlike certus_fio and bench_connector_path (which call Ring.lookup() directly),
this benchmark exercises the actual connector classes that vLLM uses:

  - ShmqCertusOffloadingManager (manager.py): touch, lookup, prepare_store,
    complete_store, prepare_load, complete_load
  - CertusShmqWorker (handler.py): submit_store, submit_load, get_finished

The measured path includes every layer the production connector adds on top of
the raw ring transport: key namespacing (ns_key per TP rank), per-block region
construction (_regions_for_block), ThreadPoolExecutor dispatch, BLAKE2b content
hashing, and the lookup cache.

Usage (certus-server must be running):
    python bench_connector_lifecycle.py --shm-path /dev/shm/certus-shmq

    # Larger jobs, longer run
    python bench_connector_lifecycle.py --job-blocks 128 --num-blocks 1024 --min-duration 10

    # Run the SAME pattern as certus_fio but through the connector
    python bench_connector_lifecycle.py --pattern cold_prefill_store
    python bench_connector_lifecycle.py --pattern warm_prefill_load_and_suffix_store

    # TP=2: one worker process per GPU (rank i on device --gpu+i, or the i-th
    # entry of --gpus), one scheduler-side manager, as vLLM runs
    # --tensor-parallel-size 2. --block-bytes is each rank's shard size.
    python bench_connector_lifecycle.py --tp 2
    python bench_connector_lifecycle.py --gpus 2,3

    # CSV output for automated collection
    python bench_connector_lifecycle.py --csv results.csv

    # vLLM <= 0.20 call order (touch before lookup) instead of the default
    python bench_connector_lifecycle.py --mode scheduler-step --lookup-order touch-first

    # Per-layer KV tensors (vLLM >= 0.29 default LBHNC; 0.23+ for the Certus
    # connector): each block is 32 regions of block_bytes/32
    # (Llama-3-8B: --block-bytes 2097152 --pieces-per-block 32 -> 64 KiB per piece)
    python bench_connector_lifecycle.py --block-bytes 2097152 --pieces-per-block 32

    # LongDocQA's cold-load shape (32 pieces, 625-block request loads, 4 in flight,
    # tier full; --tier-fill-blocks is derived from --tier-gib, default 12)
    python bench_connector_lifecycle.py --preset longdocqa

The default mode's store and warm passes and --mode contention drive the
connector as vLLM 0.30's offloading worker does: several jobs outstanding, submitted
without waiting and reaped by one get_finished() per scheduler tick (--step-us), never
a sleep-poll for one job. Job size (--job-blocks, default 64) and jobs in flight
(--inflight, default 4) are independent; --job-blocks 16 --inflight 1 runs one 16-block
job at a time, as the sequential passes these replaced did. The cold pass is the committed
cold pass (plain at --inflight 1, shaped otherwise) with the same two knobs.
    python bench_connector_lifecycle.py --job-blocks 64 --inflight 4 \
        --block-bytes 2097152 --pieces-per-block 32 --num-blocks 4000 --workers 4
    python bench_connector_lifecycle.py --mode contention    # loads 64 x 4, stores 32 x 1
    # LongDocQA's job shape, for checking the benchmark against that workload only
    python bench_connector_lifecycle.py --shape longdocqa
    # --verify compares every loaded byte (timings then include the check)

Cold-load shape (default mode): --cold-tier-state, --store-order, --load-unit and
--preset change how the cold pass is set up and driven, to match serving conditions
(see knowledge/serving/). The result is reported as "Cold Load Lifecycle (...)" with
the settings after the name, so parsers that match "Cold Load Lifecycle" find it.
"""

from __future__ import annotations

import argparse
import collections
import contextlib
import csv
import ctypes
import hashlib
import os
import random
import statistics
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field

_here = os.path.dirname(os.path.abspath(__file__))
_repo = os.path.join(_here, "..", "..")
sys.path.insert(0, os.path.join(_repo, "certus-shmq-connector"))
sys.path.insert(0, os.path.join(_repo, "apps", "python"))

import torch

assert torch.cuda.is_available(), "CUDA GPU required"


# ── vLLM compatibility shim ─────────────────────────────────────────────────
#
# The connector's handler.py imports vLLM symbols (GPULoadStoreSpec,
# TransferResult, worker base class) at module level via compat's __getattr__.
# When vLLM is not installed we must seed compat's symbol cache and stub its
# adapter functions BEFORE importing handler/manager, so the lazy resolution
# never fires.

def _has_vllm() -> bool:
    try:
        from importlib.metadata import version as _dv
        _dv("vllm")
        return True
    except Exception:
        return False


def _patch_compat_for_bench():
    """Seed compat with lightweight stubs so the connector imports without vLLM."""
    if "CERTUS_VLLM_VERSION" not in os.environ:
        os.environ["CERTUS_VLLM_VERSION"] = "0.24"

    from certus_shmq_connector import compat as _c
    from dataclasses import dataclass as _dc

    @_dc
    class _TransferResult:
        job_id: int
        success: bool
        transfer_size: int
        transfer_time: float
        transfer_type: object = None

    @_dc
    class _PrepareStoreOutput:
        keys_to_store: list
        store_spec: object
        evicted_keys: list

    class _FakeLoadStoreSpec:
        pass

    class _FakeGPULoadStoreSpec(_FakeLoadStoreSpec):
        pass

    class _FakeOffloadingManager:
        pass

    class _FakeOffloadingEvent:
        def __init__(self, **kw):
            self.__dict__.update(kw)

    _c._VLLM_SYMBOLS.update({
        "TransferResult": _TransferResult,
        "GPULoadStoreSpec": _FakeGPULoadStoreSpec,
        "LoadStoreSpec": _FakeLoadStoreSpec,
        "OffloadingManager": _FakeOffloadingManager,
        "OffloadingEvent": _FakeOffloadingEvent,
        "OffloadKey": object,
        "PrepareStoreOutput": _PrepareStoreOutput,
        "OffloadingSpec": object,
        "VllmConfig": object,
        "KVCacheConfig": object,
    })

    _orig_wbc = _c.worker_base_class
    def _stub_wbc():
        try:
            return _orig_wbc()
        except (ImportError, TypeError):
            return object
    _c.worker_base_class = _stub_wbc

    def _stub_mtr(job_id, success, transfer_size, transfer_time, transfer_type):
        return _TransferResult(
            job_id=job_id, success=success,
            transfer_size=transfer_size, transfer_time=transfer_time,
            transfer_type=transfer_type,
        )
    _c.make_transfer_result = _stub_mtr
    _c.lookup_result = lambda exists: bool(exists)


if not _has_vllm():
    _patch_compat_for_bench()


from certus_shmq_connector.ring import Ring  # noqa: E402
from certus_shmq_connector.manager import ShmqCertusOffloadingManager  # noqa: E402
from certus_shmq_connector import handler as _handler_mod  # noqa: E402
from certus_shmq_connector.handler import (  # noqa: E402
    _LOAD_TYPE, _STORE_TYPE, _regions_for_block, worker_class,
)
from certus_shmq_connector.gpu import KvCacheIpc  # noqa: E402
from certus_shmq_connector.mediums import BlockLocation, CertusLoadStoreSpec  # noqa: E402

# handler.py imports make_transfer_result from compat at module level; if we
# patched compat, propagate the stub into the already-imported handler module.
if not _has_vllm():
    import certus_shmq_connector.handler as _h
    from certus_shmq_connector.compat import make_transfer_result as _mtr
    _h.make_transfer_result = _mtr

# Default block size: 2 MiB (Llama-3-8B: 16 tokens × 32 layers × 4096 bytes).
DEFAULT_BLOCK_BYTES = 2 * 1024 * 1024

# Order of the lookup() and touch() calls for one request's keys (--lookup-order).
# vLLM's offloading scheduler (get_num_new_matched_tokens) looks up first and
# touches after since 0.21 (vLLM PR #41228, still so in 0.30); vLLM <= 0.20
# touched first.
LOOKUP_FIRST = "lookup-first"
TOUCH_FIRST = "touch-first"
LOOKUP_ORDERS = (LOOKUP_FIRST, TOUCH_FIRST)


# ── content-hashed keys (realistic vLLM pattern) ───────────────────────────

def make_content_keys(n: int, seed: int = 42) -> list[bytes]:
    """Generate n distinct 36-byte keys mimicking vLLM's content-hashed blocks.

    vLLM OffloadKeys are 36 bytes: a 32-byte block hash + 4-byte KV-cache-group
    index. We simulate this with BLAKE2b(seed || index) + a group suffix, giving
    realistic key distribution through the BLAKE2b fold in manager._key_to_u64.
    """
    keys = []
    for i in range(n):
        h = hashlib.blake2b(f"{seed}:{i}".encode(), digest_size=32).digest()
        group_idx = (i % 4).to_bytes(4, "big")
        keys.append(h + group_idx)
    return keys


# ── fake GPU region (CUDA IPC handle from a real allocation) ────────────────

def _get_cuda_ipc_handle(ptr: int) -> bytes:
    handle = (ctypes.c_byte * 64)()
    err = ctypes.CDLL("libcudart.so").cudaIpcGetMemHandle(
        ctypes.byref(handle), ctypes.c_void_p(ptr)
    )
    assert err == 0, f"cudaIpcGetMemHandle failed: {err}"
    return bytes(handle)


def _alloc_base(data_ptr: int) -> int:
    base = ctypes.c_ulonglong(0)
    size = ctypes.c_size_t(0)
    err = ctypes.CDLL("libcuda.so").cuMemGetAddressRange_v2(
        ctypes.byref(base), ctypes.byref(size), ctypes.c_ulonglong(data_ptr)
    )
    assert err == 0, f"cuMemGetAddressRange failed: {err}"
    return base.value


def make_kv_regions(gpu_id: int, block_bytes: int, num_blocks: int = 128,
                    layers: int = 1) -> tuple[list[KvCacheIpc], list[torch.Tensor]]:
    """One region per layer, as vLLM presents per-layer KV tensors (LBHNC).

    Each layer is its own allocation of ``num_blocks`` pages of
    ``block_bytes // layers`` bytes, so a block's bytes are spread over
    ``layers`` separate tensors. ``layers=1`` is one contiguous region.
    """
    if layers < 1 or block_bytes % layers:
        raise ValueError(f"--block-bytes {block_bytes} is not divisible by "
                         f"--pieces-per-block {layers}")
    regions, tensors = [], []
    for _ in range(layers):
        region, tensor = make_kv_region(gpu_id, block_bytes // layers, num_blocks)
        regions.append(region)
        tensors.append(tensor)
    return regions, tensors


def make_kv_region(gpu_id: int, block_bytes: int, num_blocks: int = 128) -> tuple[KvCacheIpc, torch.Tensor]:
    """Allocate a GPU tensor and build the KvCacheIpc the worker uses.

    The tensor is sized to hold ``num_blocks`` blocks. The worker's
    ``_regions_for_block(regions, block_id)`` computes
    ``offset = base_delta + block_id * stride_bytes``, so block_ids up to
    ``num_blocks - 1`` must land within the allocation.
    """
    total_bytes = block_bytes * num_blocks
    tensor = torch.randint(
        0, 256, (total_bytes,), dtype=torch.uint8, device=f"cuda:{gpu_id}"
    )
    data_ptr = tensor.data_ptr()
    alloc = _alloc_base(data_ptr)
    handle = _get_cuda_ipc_handle(data_ptr)
    region = KvCacheIpc(
        handle_bytes=handle,
        gpu_device_id=gpu_id,
        stride_bytes=block_bytes,
        base_delta=data_ptr - alloc,
    )
    return region, tensor


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
    # pass's SSD write-through drain; None when nothing was excluded.
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


# ── structured output (YAML / CSV) ─────────────────────────────────────────

def _yaml_val(v):
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return str(v)
    s = str(v)
    if any(c in s for c in ":{}\n#[],%&*?|>!`@'\""):
        return "'" + s.replace("'", "''") + "'"
    return s


def dump_yaml(data, stream):
    """Minimal YAML emitter for nested dicts/lists of scalars."""
    def _dump(obj, indent):
        prefix = "  " * indent
        if isinstance(obj, dict):
            for k, v in obj.items():
                if isinstance(v, (dict, list)):
                    stream.write(f"{prefix}{k}:\n")
                    _dump(v, indent + 1)
                else:
                    stream.write(f"{prefix}{k}: {_yaml_val(v)}\n")
        elif isinstance(obj, list):
            for item in obj:
                if isinstance(item, dict):
                    first = True
                    for k, v in item.items():
                        mark = "- " if first else "  "
                        first = False
                        if isinstance(v, (dict, list)):
                            stream.write(f"{prefix}{mark}{k}:\n")
                            _dump(v, indent + 2)
                        else:
                            stream.write(f"{prefix}{mark}{k}: {_yaml_val(v)}\n")
                else:
                    stream.write(f"{prefix}- {_yaml_val(item)}\n")
        else:
            stream.write(f"{prefix}{_yaml_val(obj)}\n")
    _dump(data, 0)


def _results_to_structured(results: list, extra: dict) -> dict:
    """Convert BenchResult list + extra params to a structured dict."""
    benchmarks = []
    for r in results:
        phases = {}
        for name, timing in r.phases.items():
            phases[name] = {
                "p50_us": round(timing.p50_us, 1),
                "p99_us": round(timing.p99_us, 1),
                "mean_us": round(timing.mean_us, 1),
                "n": len(timing.samples),
            }
        benchmarks.append({
            "label": r.label,
            "total_blocks": r.total_blocks,
            "total_bytes": r.total_bytes,
            "wall_seconds": round(r.wall_seconds, 6),
            "throughput_gbs": round(r.throughput_gbs, 4),
            "iops": round(r.iops, 1),
            "phases": phases,
        })
    return {"params": extra, "results": benchmarks}


def write_output(path: str, data: dict):
    """Write structured benchmark results to path (YAML or CSV by extension)."""
    ext = os.path.splitext(path)[1].lower()
    with open(path, "w", newline="") as fh:
        if ext in (".yaml", ".yml"):
            dump_yaml(data, fh)
        elif ext == ".csv":
            rows = []
            for r in data.get("results", []):
                row = {k: v for k, v in r.items() if k != "phases"}
                for pname, pvals in r.get("phases", {}).items():
                    for stat, val in pvals.items():
                        row[f"{pname}_{stat}"] = val
                row.update(data.get("params", {}))
                rows.append(row)
            if rows:
                fieldnames = list(rows[0].keys())
                for r in rows[1:]:
                    for k in r:
                        if k not in fieldnames:
                            fieldnames.append(k)
                writer = csv.DictWriter(fh, fieldnames=fieldnames,
                                        extrasaction="ignore")
                writer.writeheader()
                writer.writerows(rows)
        else:
            dump_yaml(data, fh)


# ── cold-load shape (serving conditions) ───────────────────────────────────

# Named cold-load shapes. Each sets flags whose defaults would otherwise apply;
# explicit flags override. Sizes the preset cannot know (the tier fill depends
# on the server's tier size) are derived from --tier-gib unless given.
# A preset's "observation" names the knowledge/serving/observations.yaml entry it
# reproduces; knowledge/serving/check.py fails if the preset's block bytes, pieces
# per block or blocks per load job disagree with it. It is not a flag.
COLD_PRESETS = {
    # LongDocQA, Llama-3-8B, vLLM 0.30 under the default LBHNC layout: 2 MiB blocks
    # in 32 layer pieces, one 625-block load job per request, 4 requests in flight,
    # tier full.
    "longdocqa": {
        "observation": "ldq-llama-3-8b-lbhnc-2026-10-26",
        "block_bytes": 2097152,     # 2 MiB: 16 tokens x 128 KiB/token
        "pieces_per_block": 32,
        "bs": 16,
        "requests": 4,
        "num_blocks": 2500,         # 4 requests x 625 blocks (10,000 tokens / 16)
        "store_order": "interleaved",
        "load_unit": "request",
        "inflight": 4,
        "cold_tier_state": "full",
    },
}

# The cold-shape flags' defaults; the run's params record the shape only when it differs.
COLD_SHAPE_DEFAULTS = {"cold_tier_state": "empty", "store_order": "batch", "load_unit": "batch"}


def cold_shape_is_default(args) -> bool:
    return all(getattr(args, k) == v for k, v in COLD_SHAPE_DEFAULTS.items())


def split_requests(keys: list[bytes], requests: int) -> list[list[bytes]]:
    """Split keys into `requests` consecutive prefixes of equal length."""
    per = len(keys) // requests
    return [keys[r * per:(r + 1) * per] for r in range(requests)]


def _start_load_job(manager, worker, unit, gpu_base, job_id, phases, *,
                    touch_first: bool, prefix_stop: bool) -> list[bytes]:
    """One load job as vLLM's scheduler and worker start it: lookup the unit's keys
    (stopping at the first miss when ``prefix_stop``, as a request's prefix does),
    touch them (before the lookups with ``touch_first``), pin the hits
    (prepare_load) and submit them to GPU blocks ``gpu_base..``. Returns the hits;
    nothing is pinned or submitted when there are none. Phases "touch", "lookup",
    "prepare_load" and "submit_load" are recorded. Used by _StepLoop.start_load."""
    if touch_first:
        t0 = time.perf_counter(); manager.touch(unit)
        phases["touch"].record(time.perf_counter() - t0)
    t0 = time.perf_counter()
    hits = []
    for k in unit:
        if not manager.lookup(k):
            if prefix_stop:
                break
            continue
        hits.append(k)
    phases["lookup"].record(time.perf_counter() - t0)
    if not touch_first:
        t0 = time.perf_counter(); manager.touch(unit)
        phases["touch"].record(time.perf_counter() - t0)
    if hits:
        _submit_load_job(manager, worker, hits, gpu_base, job_id, phases)
    return hits


def _submit_load_job(manager, worker, hits, gpu_base, job_id, phases) -> None:
    """Pin ``hits`` (prepare_load) and submit them to GPU blocks ``gpu_base..``; phases
    "prepare_load" and "submit_load"."""
    t0 = time.perf_counter()
    spec = manager.prepare_load(hits)
    phases["prepare_load"].record(time.perf_counter() - t0)
    t0 = time.perf_counter()
    try:
        worker.submit_load(job_id, spec, _FakeGPUSpec(list(range(gpu_base, gpu_base + len(hits)))))
    except BaseException:
        manager.complete_load(hits)   # not submitted: unpin before the error propagates
        raise
    phases["submit_load"].record(time.perf_counter() - t0)


# ── in-flight passes: vLLM 0.30's step loop (store, warm, contention) ──
#
# vLLM 0.30's offloading worker (offloading/worker.py) never waits for a transfer:
# each engine step it submits the step's new load jobs and the stores deferred from
# the previous step (start_kv_transfers), and reaps whatever has finished with one
# non-blocking get_finished() call. Several requests' jobs are outstanding at once.
# The default mode's store and warm passes and the contention mode drive the
# connector that way, with job size (--job-blocks) and jobs in flight (--inflight)
# as independent knobs: neither is fixed by the pass, because both vary by workload.
# --job-blocks 16 --inflight 1 reproduces the sequential passes these replaced (one
# 16-block job at a time; same server session, 2026-11-05: warm 14.35 vs 14.16 GB/s,
# store 8.74 vs 8.97 against the committed in-flight modes, within run-to-run spread). Each tick: submit into free slots, call get_finished() once, complete what
# finished, then idle (--step-us N, default 100: sleep to the next N-us tick boundary;
# --step-us 0: time.sleep(0), a tight loop that releases the GIL to the pool threads
# without waiting, but leaves some jobs stalled for several GIL switch intervals: warm
# 64 x 1 on dc564aa3, 10 runs each, job p99 6.4-27.1 ms and 14 % spread at 0 against
# 6.2-6.4 ms and 8 % at 100, same median). No call waits for a particular job.
#
# Phases: "get_finished" is each job's time from submit to reap (what the sequential
# passes' get_finished measured: the wait for one job), "reap_call" the duration of
# the one get_finished() call per tick, "*_transfer" the connector's transfer_time.

# Named shapes of real workloads, for checking the benchmark against them, not for
# a target's primary metric (fitting a target to one workload tunes Certus to that
# workload). "observation" names the knowledge/serving/observations.yaml entry;
# knowledge/serving/check.py fails if a shape and its observation disagree.
INFLIGHT_SHAPES = {
    # LongDocQA, Llama-3-8B, vLLM 0.30, LBHNC: one 625-block load job per document
    # (10,000 tokens / 16), 4 requests in flight; stores of ~105 blocks per job
    # (67.5 GB over 306 jobs: one job per request per 2,048-token prefill chunk).
    # Store jobs in flight were not measured: 4 (one per request in flight) is an
    # assumption, listed in "assumed" so check.py does not treat it as checked.
    "longdocqa": {
        "observation": "ldq-llama-3-8b-lbhnc-2026-10-26",
        "block_bytes": 2097152,
        "pieces_per_block": 32,
        "job_blocks": 625,
        "inflight": 4,
        "store_job_blocks": 105,
        "store_inflight": 4,
        "assumed": ("store_inflight",),
    },
}

# The modes that run in-flight passes and take the defaults below.
INFLIGHT_MODES = ("default", "contention")
# What they use when a flag is not given: 32 pieces per block as vLLM 0.30 lays out
# Llama-3-8B (a block is 16 tokens x 128 KiB). The other modes keep the argparse defaults.
INFLIGHT_DEFAULTS = {"pieces_per_block": 32}
# --num-blocks when not given: 90% of what the memory tier holds before evicting, at most this
# many (4,000 blocks is 7.8 GiB at 2 MiB, 63 jobs of 64; it is the whole default on a 12 GiB tier).
INFLIGHT_MAX_BLOCKS = 4000
INFLIGHT_TIER_SHARE = 0.9
# The contention mode's store side when not given: 32-block jobs, one in flight. Loads are the
# larger stream in serving (multi-turn moved 3.3x more bytes loaded than stored); in the
# 2026-11-05 mixed sweep 64 x 4 loads against 32 x 1 stores came closest at 4 loads in flight (2.4x).
CONTENTION_STORE_DEFAULTS = {"store_job_blocks": 32, "store_inflight": 1}
# Before the contention window, the same loads run alone for this long on the same manager,
# worker and resident working set ("Contention: Load Alone"), so the load under the concurrent
# store is compared with a load set up the same way.
CONTENTION_ALONE_SECONDS = 3.0


class _ByteVerifier:
    """--verify for the in-flight passes: every block a store job sends is first
    written with its key's pattern, every load job's GPU blocks are zeroed before it
    is submitted and compared with its keys' patterns when it is reaped. A block's
    pattern is, in each of its pieces, the key's first 8 bytes followed by a fill byte
    of (key[0] + piece index) mod 256, so a block or piece delivered to the wrong
    place, or not at all, fails. Adds GPU work to the timed loop: timings taken with
    --verify are not comparable with timings taken without it."""

    def __init__(self, tensors: list):
        self.tensors = tensors
        self.checked = 0
        self.bad = 0

    def _rows(self, t, base, n, piece):
        return t.view(-1, piece)[base:base + n]

    def _pattern(self, keys, layer, piece, device):
        """Each piece: the whole key, then the layer index (4 bytes), then a fill byte
        from a hash of (key, layer). Needs pieces of at least len(key) + 4 bytes."""
        head = [bytes(k) + int(layer).to_bytes(4, "little") for k in keys]
        fill = torch.tensor([hashlib.blake2b(h, digest_size=1).digest()[0] for h in head],
                            dtype=torch.uint8, device=device)
        pat = fill.unsqueeze(1).expand(len(keys), piece).clone()
        n = len(head[0])
        pat[:, :n] = torch.tensor([list(h) for h in head], dtype=torch.uint8, device=device)
        return pat

    def _piece(self):
        return self.tensors[0].numel() // self._blocks

    def bind(self, num_blocks: int):
        self._blocks = num_blocks
        return self

    def write(self, keys, base):
        piece = self._piece()
        for layer, t in enumerate(self.tensors):
            self._rows(t, base, len(keys), piece).copy_(self._pattern(keys, layer, piece, t.device))
        torch.cuda.synchronize()

    def zero(self, base, n):
        piece = self._piece()
        for t in self.tensors:
            self._rows(t, base, n, piece).zero_()
        torch.cuda.synchronize()

    def check(self, keys, base) -> None:
        torch.cuda.synchronize()
        piece = self._piece()
        bad = torch.zeros(len(keys), dtype=torch.bool, device=self.tensors[0].device)
        for layer, t in enumerate(self.tensors):
            got = self._rows(t, base, len(keys), piece)
            bad |= (got != self._pattern(keys, layer, piece, t.device)).any(dim=1)
        self.checked += len(keys)
        self.bad += int(bad.sum().item())

    def report(self, what: str) -> None:
        print(f"  Verify ({what}): {self.checked} blocks compared, {self.bad} wrong"
              f"  (timings above include the verification work)")
        if self.bad:
            raise RuntimeError(f"--verify: {self.bad} of {self.checked} loaded blocks wrong")


# How long the sequential passes (prefix-miss, scheduler-step, pattern,
# mixed eviction, populate) wait between polls for finished transfers (--step-us; the
# in-flight passes use it as their scheduler tick). They always slept 100 us here,
# which is the default. --step-us 0 polls without sleeping (time.sleep(0), releasing the GIL).
_POLL_S = 100e-6


def set_poll_us(step_us: float) -> None:
    global _POLL_S
    _POLL_S = max(0.0, float(step_us)) / 1e6


def _poll_wait() -> None:
    time.sleep(_POLL_S)


def _step_idle(step_us: float, t_tick: float) -> None:
    """End of one step-loop tick: release the GIL (step_us <= 0) or sleep to the tick
    boundary. time.sleep(0) drops and retakes the GIL, so the pool threads (which run
    the connector's ring I/O in Python) get it every tick, as they do while a vLLM
    engine step runs; os.sched_yield() keeps the GIL and starved them (measured: a
    pure-Python pool thread ran 1.8x slower beside a sched_yield loop, unchanged
    beside a sleep(0) loop)."""
    if step_us <= 0:
        time.sleep(0)
        return
    rest = t_tick + step_us / 1e6 - time.perf_counter()
    if rest > 0:
        time.sleep(rest)


class _InflightStats:
    """Per-direction accounting for the in-flight passes: per-job times (submit to
    reap, and the connector's TransferResult.transfer_time, which is what vLLM logs
    as kv_offload_load_time / kv_offload_store_time), blocks moved and the
    time-weighted number of jobs outstanding."""

    def __init__(self, prefix: str):
        self.job = PhaseTiming(f"{prefix}job")
        self.transfer = PhaseTiming(f"{prefix}transfer")
        self.blocks = 0
        self.jobs = 0
        self.misses = 0      # loads: keys whose lookup missed; stores: keys not reserved
        self._area = 0.0     # integral of jobs outstanding over time
        self._t_last = None

    def sample(self, outstanding: int, now: float) -> None:
        if self._t_last is not None:
            self._area += outstanding * (now - self._t_last)
        self._t_last = now

    def outstanding_mean(self, wall: float) -> float:
        return self._area / wall if wall > 0 else 0.0

    def concurrency(self, wall: float) -> float:
        """Summed transfer time over wall time: the formula the LongDocQA observation
        uses for its measured load concurrency (2.91 with 4 configured)."""
        return sum(self.transfer.samples) / wall if wall > 0 else 0.0


def _print_inflight_summary(stats: _InflightStats, wall: float, what: str) -> None:
    print(f"  {what} jobs={stats.jobs}  per-job submit->reap mean={stats.job.mean_us:.1f}us "
          f"p50={stats.job.p50_us:.1f}us p99={stats.job.p99_us:.1f}us  "
          f"transfer_time mean={stats.transfer.mean_us:.1f}us  "
          f"outstanding mean={stats.outstanding_mean(wall):.2f}  "
          f"concurrency (sum transfer / wall)={stats.concurrency(wall):.2f}")


class _StepLoop:
    """The shared step loop: one worker, load and store jobs outstanding together,
    each in its own GPU block slot, reaped by one get_finished() per tick.

    Loads are started in the tick they are scheduled (lookup -> touch ->
    prepare_load -> submit_load, as vLLM's get_num_new_matched_tokens /
    update_state_after_alloc / start_kv_transfers do). Stores are prepared in one
    tick (prepare_store, then touch, as _build_store_jobs does) and submitted at the
    start of the next (prepare_store_kv defers them past the step's sampling).
    Finished jobs are completed (complete_load / complete_store) when reaped. A
    failed job is completed too, then every outstanding job is drained and
    completed before RuntimeError is raised, so no key stays pinned or reserved."""

    def __init__(self, manager, worker, phases, *, touch_first: bool,
                 load_slots: int = 0, load_job_blocks: int = 0,
                 store_slots: int = 0, store_job_blocks: int = 0,
                 drain_events: bool = False, verifier: "_ByteVerifier | None" = None,
                 prefix_stop: bool = True):
        self.manager, self.worker, self.phases = manager, worker, phases
        self.verifier = verifier
        self.touch_first = touch_first
        self.prefix_stop = prefix_stop        # a load stops at its first lookup miss
        self.load_job_blocks = load_job_blocks
        self.store_job_blocks = store_job_blocks
        # GPU block ranges: load slots first, then store slots, never overlapping.
        self.free_load = list(range(load_slots))
        self.free_store = list(range(store_slots))
        self.store_base = load_slots * load_job_blocks
        self.load_stats = _InflightStats("load_")
        self.store_stats = _InflightStats("store_")
        self.pending: dict[int, tuple] = {}   # job_id -> (kind, keys, slot, t_submit, tag)
        self.deferred: list[tuple] = []       # prepared stores: (keys, spec, slot, tag)
        self.n_load = 0
        self.n_store = 0                      # submitted + deferred stores
        self.job_id = 3_000_000
        self.drain_events = drain_events
        self.events = 0
        self.uncompleted: list[tuple] = []   # reaped jobs whose completion raised

    # ── scheduling ──

    def load_slot_free(self) -> bool:
        return bool(self.free_load)

    def store_slot_free(self) -> bool:
        return bool(self.free_store)

    def start_load(self, unit, tag=None) -> bool:
        """Start one load job for ``unit`` (a request's keys, prefix order). False when
        no key hit (nothing pinned or submitted)."""
        slot = self.free_load[0]
        self.job_id += 1
        if self.verifier is not None:
            self.verifier.zero(slot * self.load_job_blocks, self.load_job_blocks)
        hits = _start_load_job(self.manager, self.worker, unit, slot * self.load_job_blocks,
                               self.job_id, self.phases, touch_first=self.touch_first,
                               prefix_stop=self.prefix_stop)
        self.load_stats.misses += len(unit) - len(hits)
        if not hits:
            return False
        self.sample(time.perf_counter())
        self.free_load.pop(0)
        self.pending[self.job_id] = ("load", hits, slot, time.perf_counter(), tag)
        self.n_load += 1
        return True

    def start_load_hits(self, hits, tag=None) -> None:
        """Start one load job for ``hits``, keys the caller has already looked up and
        touched (scheduler-step does its own prefix scan per request)."""
        if len(hits) > self.load_job_blocks:
            raise RuntimeError(f"load job of {len(hits)} blocks exceeds its GPU slot "
                               f"({self.load_job_blocks})")
        slot = self.free_load[0]
        self.job_id += 1
        _submit_load_job(self.manager, self.worker, hits, slot * self.load_job_blocks,
                         self.job_id, self.phases)
        self.sample(time.perf_counter())
        self.free_load.pop(0)
        self.pending[self.job_id] = ("load", list(hits), slot, time.perf_counter(), tag)
        self.n_load += 1

    def prepare_store(self, unit, tag=None) -> bool:
        """Prepare one store job for ``unit``; it is submitted by the next tick's
        submit_deferred(). False when nothing was reserved."""
        m, ph = self.manager, self.phases
        t0 = time.perf_counter()
        out = m.prepare_store(unit)
        ph["prepare_store"].record(time.perf_counter() - t0)
        stored = list(out.keys_to_store) if out is not None else []
        self.store_stats.misses += len(unit) - len(stored)
        if not stored:
            return False
        t0 = time.perf_counter()
        try:
            m.touch(unit)
        except BaseException:
            m.complete_store(stored, success=False)   # reserved, never submitted
            raise
        ph["store_touch" if "store_touch" in ph else "touch"].record(time.perf_counter() - t0)
        self.sample(time.perf_counter())
        slot = self.free_store.pop(0)
        self.deferred.append((stored, out.store_spec, slot, tag))
        self.n_store += 1
        return True

    def submit_deferred(self) -> None:
        while self.deferred:
            stored, spec, slot, tag = self.deferred[0]
            base = self.store_base + slot * self.store_job_blocks
            self.job_id += 1
            if self.verifier is not None:
                self.verifier.write(stored, base)
            t0 = time.perf_counter()
            self.worker.submit_store(self.job_id, _FakeGPUSpec(list(range(base, base + len(stored)))),
                                     spec)
            self.phases["submit_store"].record(time.perf_counter() - t0)
            self.deferred.pop(0)
            self.pending[self.job_id] = ("store", stored, slot, time.perf_counter(), tag)

    # ── reaping ──

    def reap(self) -> list[tuple]:
        """One get_finished() call (its duration is phase "reap_call"); completes each
        finished job. Returns (kind, keys, tag) per finished job."""
        t0 = time.perf_counter()
        finished = self.worker.get_finished()
        self.phases["reap_call"].record(time.perf_counter() - t0)
        done, failed = [], []
        error = None   # the first exception; every job of the batch is still completed
        now = time.perf_counter()
        if finished:
            self.sample(now)
        for res in finished:
            kind, keys, slot, t_sub, tag = self.pending.pop(res.job_id)
            stats = self.load_stats if kind == "load" else self.store_stats
            stats.job.record(now - t_sub)
            if getattr(res, "transfer_time", None) is not None:
                stats.transfer.record(res.transfer_time)
            try:
                if res.success and kind == "load" and self.verifier is not None:
                    self.verifier.check(keys, slot * self.load_job_blocks)   # before unpin
            except BaseException as e:  # noqa: BLE001 - re-raised after the batch
                error = error or e
            try:
                self._complete(kind, keys, bool(res.success))
            except BaseException as e:  # noqa: BLE001
                error = error or e
                self.uncompleted.append((kind, keys, bool(res.success)))   # _abort retries
            if kind == "load":
                self.free_load.append(slot)
                self.n_load -= 1
            else:
                self.free_store.append(slot)
                self.n_store -= 1
            if res.success:
                stats.blocks += len(keys)
                stats.jobs += 1
                done.append((kind, keys, tag))
            else:
                failed.append(res.job_id)
        if error is not None:
            raise error
        if self.drain_events and any(k == "store" for k, _, _ in done):
            t0 = time.perf_counter()
            for ev in self.manager.take_events():
                self.events += len(getattr(ev, "keys", ()) or ())
            self.phases["take_events"].record(time.perf_counter() - t0)
        if failed:
            raise RuntimeError(f"in-flight transfer job(s) {failed} failed")
        return done

    def _complete(self, kind, keys, success) -> None:
        t0 = time.perf_counter()
        if kind == "load":
            self.manager.complete_load(keys)
            self.phases["complete_load"].record(time.perf_counter() - t0)
        else:
            self.manager.complete_store(keys, success=success)
            self.phases["complete_store"].record(time.perf_counter() - t0)

    def _abort(self) -> None:
        """Abort prepared stores and drain every outstanding job, completing each, so
        no key stays pinned or reserved. Gives up on jobs the worker never reports
        after 120 s (the run is failing anyway)."""
        for stored, _spec, _slot, _tag in self.deferred:
            try:
                self.manager.complete_store(stored, success=False)
            except Exception:  # noqa: BLE001 - best effort while failing
                pass
        self.deferred = []
        for kind, keys, success in self.uncompleted:
            try:
                self._complete(kind, keys, success)
            except Exception:  # noqa: BLE001 - best effort while failing
                pass
        self.uncompleted = []
        deadline = time.monotonic() + 120.0
        while self.pending and time.monotonic() < deadline:
            for res in self.worker.get_finished():
                kind, keys, _slot, _t, _tag = self.pending.pop(res.job_id)
                try:
                    self._complete(kind, keys, bool(res.success))
                except Exception:  # noqa: BLE001
                    pass
            time.sleep(0)
        if self.pending:
            print(f"  WARNING: {len(self.pending)} job(s) never reported finished: "
                  f"{sorted(self.pending)}", file=sys.stderr)

    @contextlib.contextmanager
    def cleanup(self):
        """On any exception inside the block, abort and drain (_abort), then re-raise."""
        try:
            yield
        except BaseException:
            self._abort()
            raise

    def sample(self, now: float) -> None:
        self.load_stats.sample(self.n_load, now)
        self.store_stats.sample(self.n_store, now)


class _LoadScheduler:
    """Which load job to start next: the working set's jobs in a shuffled order per
    pass, skipping a job whose keys are still in flight. ``fill`` starts jobs into the
    free slots, trying each job of the current order at most once per tick, so a tick
    always ends (to reap and idle) even when every lookup misses."""

    def __init__(self, units, seed: int):
        self.units = units
        self.rng = random.Random(seed)
        self.order: list[int] = []
        self.busy: set[int] = set()
        self.passes = 0

    def fill(self, loop: "_StepLoop") -> None:
        tried = 0
        while loop.load_slot_free() and tried < len(self.units):
            if not self.order:
                self.order = list(range(len(self.units)))
                self.rng.shuffle(self.order)
                self.passes += 1
            idx = next((i for i in self.order if i not in self.busy), None)
            if idx is None:
                return
            self.order.remove(idx)
            tried += 1
            if loop.start_load(self.units[idx], tag=idx):
                self.busy.add(idx)

    def done(self, finished) -> None:
        for kind, _keys, idx in finished:
            if kind == "load":
                self.busy.discard(idx)

    def at_pass_boundary(self) -> bool:
        return not self.order


def _inflight_phases(*names) -> dict[str, PhaseTiming]:
    """Phases of an in-flight pass. Every _StepLoop records "reap_call" (the one
    get_finished() call per tick); each pass then replaces "get_finished" with its
    per-job submit-to-reap times (_InflightStats.job), the wait the sequential passes
    recorded under that name."""
    return {n: PhaseTiming(n) for n in names + ("reap_call",)}


def _job_units(keys: list[bytes], job_blocks: int) -> list[list[bytes]]:
    return [keys[i:i + job_blocks] for i in range(0, len(keys), job_blocks)]


def _populate_jobs(manager, worker, keys, job_blocks, verifier=None) -> None:
    """Store ``keys`` as job_blocks-key jobs (outside any timing), so each job's keys
    form one commit group, as one vLLM store job's do. With a verifier each job's
    GPU blocks are first written with its keys' patterns."""
    job_id = 4_000_000
    stored_n = 0
    for i in range(0, len(keys), job_blocks):
        batch = keys[i:i + job_blocks]
        out = manager.prepare_store(batch)
        if out is None or not out.keys_to_store:
            continue
        stored = list(out.keys_to_store)
        ok = False
        try:
            manager.touch(batch)
            if verifier is not None:
                verifier.write(stored, 0)
            job_id += 1
            worker.submit_store(job_id, _FakeGPUSpec(list(range(len(stored)))), out.store_spec)
            res = []
            while not res:
                res = worker.get_finished()
                if not res:
                    _poll_wait()
            ok = all(r.success for r in res)
        finally:
            manager.complete_store(stored, success=ok)
        if not ok:
            raise RuntimeError(f"populate: store job {job_id} failed")
        stored_n += len(stored)
    if stored_n < len(keys):
        # A key already present is not re-stored; any other shortfall means the tier
        # refused reservations and the working set does not fit.
        missing = sum(1 for k in keys if not manager.lookup(k))
        if missing:
            raise RuntimeError(f"populate: {missing} of {len(keys)} working-set keys not "
                               f"resident (memory tier too small for --num-blocks?)")


def _verify_load_back(manager, worker, keys, job_blocks, verifier) -> None:
    """--verify for the store pass: load ``keys`` back one job at a time (outside the
    timing) and compare every block."""
    job_id = 4_500_000
    for unit in _job_units(keys, job_blocks):
        hits = [k for k in unit if manager.lookup(k)]
        if len(hits) != len(unit):
            verifier.bad += len(unit) - len(hits)
            verifier.checked += len(unit) - len(hits)
        if not hits:
            continue
        verifier.zero(0, len(hits))
        spec = manager.prepare_load(hits)
        job_id += 1
        res = []
        try:
            worker.submit_load(job_id, spec, _FakeGPUSpec(list(range(len(hits)))))
            while not res:
                res = worker.get_finished()
                if not res:
                    _poll_wait()
            if all(r.success for r in res):
                verifier.check(hits, 0)
        finally:
            manager.complete_load(hits)
        if not all(r.success for r in res):
            raise RuntimeError(f"--verify load-back job {job_id} failed")


def cold_unit_blocks(args) -> int:
    """Blocks per cold load job: one request's prefix (--load-unit request) or --job-blocks."""
    return args.num_blocks // args.requests if args.load_unit == "request" else args.job_blocks


def inflight_label(args, kind: str) -> str:
    """The settings an in-flight result's label carries after its base name (parsers
    match the base name, e.g. "Warm Load Lifecycle", as a substring)."""
    if kind == "contention":
        s = (f"load {args.job_blocks}x{args.inflight} "
             f"store {args.store_job_blocks}x{args.store_inflight}")
    elif kind == "store":
        s = f"job_blocks={args.store_job_blocks} inflight={args.store_inflight}"
    else:
        s = f"job_blocks={args.job_blocks} inflight={args.inflight}"
    s += f" step_us={args.step_us:g}"
    if args.pieces_per_block > 1:
        s += f" pieces={args.pieces_per_block}"
    if args.shape:
        s += f" shape={args.shape}"
    return s


def _load_window(loop: "_StepLoop", sched: "_LoadScheduler", seconds: float, step_us: float,
                 min_passes: int) -> float:
    """Run load ticks until ``seconds`` have passed and ``min_passes`` passes over the
    working set are done, stopping new jobs only at a pass boundary, then reap what is
    outstanding. Returns the wall time from the first tick to the last reap."""
    t_start = time.perf_counter()
    loop.sample(t_start)
    with loop.cleanup():
        while True:
            t_tick = time.perf_counter()
            running = not (t_tick - t_start >= seconds and sched.passes >= min_passes
                           and sched.at_pass_boundary())
            if running:
                sched.fill(loop)
            sched.done(loop.reap())
            if not running and not loop.pending:
                break
            _step_idle(step_us, t_tick)
    wall = time.perf_counter() - t_start
    loop.sample(t_start + wall)
    return wall


def bench_warm_inflight(manager, worker, keys, block_bytes, min_duration, *, args,
                        lookup_order: str = LOOKUP_FIRST, verifier=None) -> BenchResult:
    """Warm load with jobs in flight: the working set (``keys``, which must fit the
    memory tier) is stored once as --job-blocks-key store jobs, its write-through
    drained (it stays resident; nothing is flushed out or cleared), then loaded as
    --job-blocks-key jobs with up to --inflight outstanding, in a shuffled order per
    pass over the working set, by the step loop. A job whose keys are still in flight
    is skipped for that turn. Timed from the first submit to the last reap; at least
    two passes and --min-duration seconds."""
    touch_first = lookup_order == TOUCH_FIRST
    phases = _inflight_phases("touch", "lookup", "prepare_load", "submit_load",
                              "get_finished", "complete_load")
    units = _job_units(keys, args.job_blocks)
    _populate_jobs(manager, worker, keys, args.job_blocks, verifier)
    manager._ring.flush_to_ssd()   # the write-through must not overlap the timed loads
    loop = _StepLoop(manager, worker, phases, touch_first=touch_first,
                     load_slots=args.inflight, load_job_blocks=args.job_blocks,
                     verifier=verifier)
    sched = _LoadScheduler(units, args.seed)
    wall = _load_window(loop, sched, min_duration, args.step_us, min_passes=2)
    st = loop.load_stats
    if st.misses:
        # The working set was populated and checked resident, and nothing is stored or
        # evicted during the pass: a warm lookup that misses is a fault, not a result.
        raise RuntimeError(f"warm load: {st.misses} working-set blocks missed at lookup "
                           f"({st.blocks} loaded)")
    phases["get_finished"], phases["load_transfer"] = st.job, st.transfer
    result = BenchResult(
        label=f"Warm Load Lifecycle ({inflight_label(args, 'warm')})",
        total_blocks=st.blocks, total_bytes=st.blocks * block_bytes,
        wall_seconds=wall, phases=phases,
    )
    result.counts = {"blocks_not_loaded": st.misses, "load_misses": st.misses}
    result.print_report()
    _print_inflight_summary(st, wall, "load")
    print(f"  Blocks not loaded (prefix stopped at a lookup miss): {st.misses}  "
          f"passes: {sched.passes}")
    if verifier is not None:
        verifier.report("warm loads")
    return result


def bench_store_inflight(manager, worker, ring, keys, block_bytes, min_duration, *, args,
                         world_size: int, verifier=None) -> BenchResult:
    """Store with jobs in flight: per pass every key is stored once as
    --store-job-blocks jobs (default --job-blocks) with up to --store-inflight (default
    --inflight) outstanding (prepared in one tick, submitted the next,
    reaped by the step loop); then, outside the timing, the pass's write-through is
    drained and its keys removed (phase "reset"), as the cputier and lmcache benches
    exclude their disk drain. The headline is the GPU->DRAM store path; the figure
    including the drains is printed below it. Each pass ends by reaping its last
    jobs, so its final jobs run with fewer than --inflight outstanding."""
    phases = _inflight_phases("prepare_store", "touch", "submit_store", "get_finished",
                              "complete_store", "take_events", "reset")
    units = _job_units(keys, args.store_job_blocks)
    loop = _StepLoop(manager, worker, phases, touch_first=False,
                     store_slots=args.store_inflight, store_job_blocks=args.store_job_blocks,
                     drain_events=True, verifier=verifier)
    timed, passes = 0.0, 0
    t_all = time.perf_counter()
    while timed < min_duration or passes < 2:
        todo = list(units)
        t_pass = time.perf_counter()
        loop.sample(t_pass)
        with loop.cleanup():
            while todo or loop.deferred or loop.pending:
                t_tick = time.perf_counter()
                loop.submit_deferred()
                while todo and loop.store_slot_free():
                    loop.prepare_store(todo.pop(0))   # nothing reserved: counted as dropped
                loop.reap()
                if todo or loop.deferred or loop.pending:
                    _step_idle(args.step_us, t_tick)
        now = time.perf_counter()
        loop.sample(now)
        timed += now - t_pass
        passes += 1
        t0 = time.perf_counter()
        if verifier is not None:
            _verify_load_back(manager, worker, keys, args.store_job_blocks, verifier)
        manager._ring.flush_to_ssd()
        _remove_keys(ring, keys, world_size)
        phases["reset"].record(time.perf_counter() - t0)
    elapsed = time.perf_counter() - t_all
    st = loop.store_stats
    if st.blocks == 0:
        raise RuntimeError(f"store: nothing stored ({st.misses} keys not reserved)")
    phases["get_finished"], phases["store_transfer"] = st.job, st.transfer
    result = BenchResult(
        label=f"Store Lifecycle ({inflight_label(args, 'store')})",
        total_blocks=st.blocks, total_bytes=st.blocks * block_bytes,
        wall_seconds=timed, phases=phases,
    )
    result.sustained = (result.total_bytes / elapsed / 1e9, elapsed)
    result.counts = {"store_drops": st.misses}
    result.print_report()
    gbs, secs = result.sustained
    print(f"  Sustained incl. SSD write-through drain: {gbs:.2f} GB/s over {secs:.3f}s")
    # Outstanding/concurrency over the timed passes only (the drains are excluded).
    _print_inflight_summary(st, timed, "store")
    print(f"  Store drops: {st.misses}  passes: {passes}")
    if verifier is not None:
        verifier.report("stores, each pass loaded back")
    return result


# ── cold load: the committed sequential cold passes (restored) ─────────────
#
# The plain cold pass (one --job-blocks job at a time, --inflight 1 under the default cold
# shape) and the shaped cold pass (every other shape or --inflight), as committed before the
# step-loop refactor, which reported 24.34 GB/s cold with tier=empty, above the 4 drives'
# 23.14 GB/s (loads served from DRAM). Batches are --job-blocks keys (was --bs), jobs in
# flight --inflight (was --cold-inflight), polls sleep --step-us. No --verify here.

def bench_load_lifecycle(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    keys: list[bytes],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    cold: bool = False,
    lookup_order: str = LOOKUP_FIRST,
) -> BenchResult:
    """Full load lifecycle: lookup → touch (or touch → lookup with
    ``lookup_order=TOUCH_FIRST``) → prepare_load → submit_load → get_finished
    → complete_load."""
    label_temp = "Cold" if cold else "Warm"
    touch_first = lookup_order == TOUCH_FIRST
    phases = {
        "touch": PhaseTiming("touch"),
        "lookup": PhaseTiming("lookup"),
        "prepare_load": PhaseTiming("prepare_load"),
        "submit_load": PhaseTiming("submit_load"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_load": PhaseTiming("complete_load"),
    }

    if cold:
        phases["demote"] = PhaseTiming("demote")

    total_blocks = 0
    excluded = 0.0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 1000
    io_before = manager._ring.get_io_stats() if cold else None

    while (time.perf_counter() - wall_start - excluded) < min_duration or iteration < 2:
        if cold:
            # Every pass starts with an empty memory tier, so each block is read
            # back from SSD. Drain the write-through first: an entry with no SSD
            # copy is removed, not demoted, and the pass would then miss it. The
            # empty-out is excluded from wall time, as in the cputier and lmcache
            # benches (their ``demote`` phase).
            t0 = time.perf_counter()
            manager._ring.flush_to_ssd()
            manager._ring.clear_memory_tier()
            phases["demote"].record(time.perf_counter() - t0)
            excluded += time.perf_counter() - t0

        for i in range(0, len(keys), batch_size):
            batch_keys = keys[i : i + batch_size]

            # 1. touch first (vLLM <= 0.20 only): its reply answers the lookups
            if touch_first:
                t0 = time.perf_counter()
                manager.touch(batch_keys)
                phases["touch"].record(time.perf_counter() - t0)

            # 2. lookup (per-key, from a recent touch reply or a fallback Check)
            t0 = time.perf_counter()
            hits = []
            for k in batch_keys:
                if manager.lookup(k):
                    hits.append(k)
            phases["lookup"].record(time.perf_counter() - t0)

            # 3. touch after the lookup (vLLM >= 0.21)
            if not touch_first:
                t0 = time.perf_counter()
                manager.touch(batch_keys)
                phases["touch"].record(time.perf_counter() - t0)

            if not hits:
                continue

            # 4. prepare_load (pin)
            t0 = time.perf_counter()
            load_spec = manager.prepare_load(hits)
            phases["prepare_load"].record(time.perf_counter() - t0)

            gpu_block_ids = list(range(len(hits)))

            # 5. submit_load (async Certus→GPU via ThreadPoolExecutor)
            t0 = time.perf_counter()
            job_id += 1
            worker.submit_load(
                job_id,
                load_spec,
                _FakeGPUSpec(gpu_block_ids),
            )
            phases["submit_load"].record(time.perf_counter() - t0)

            # 6. get_finished
            t0 = time.perf_counter()
            while not worker.get_finished():
                _poll_wait()
            phases["get_finished"].record(time.perf_counter() - t0)

            # 7. complete_load (unpin)
            t0 = time.perf_counter()
            manager.complete_load(hits)
            phases["complete_load"].record(time.perf_counter() - t0)

            total_blocks += len(hits)

        iteration += 1

    wall = time.perf_counter() - wall_start - excluded
    if cold:
        io = manager._ring.get_io_stats()
        reads = io["read_bytes"] - io_before["read_bytes"]
        want = total_blocks * block_bytes
        print(f"  Cold pass SSD reads: {reads / 1e6:.0f} MB for {want / 1e6:.0f} MB loaded"
              + ("" if reads else "  (0: certus-server built without rw-telemetry, "
                                  "or the loads were served from DRAM)"))
    result = BenchResult(
        label=(
            f"{label_temp} Load Lifecycle "
            f"({'touch → lookup' if touch_first else 'lookup → touch'} "
            f"→ prepare → submit → finish → complete)"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    if cold:
        result.counts = {"disk_read_bytes": reads}
    return result


def cold_shape_label(args) -> str:
    return (f"tier={args.cold_tier_state} store={args.store_order} "
            f"load={args.load_unit} inflight={args.inflight}"
            + (f" pieces={args.pieces_per_block}" if args.pieces_per_block > 1 else ""))


def populate_shaped(manager, worker, keys, batch_size, store_order, requests):
    """Store keys as batch_size-key calls (batch) or one block per request in turn
    (interleaved: request r stores its i-th block at step i, as vLLM commits per step)."""
    if store_order == "batch":
        _populate_keys(manager, worker, keys, batch_size)
        return
    prefixes = split_requests(keys, requests)
    for step in range(len(prefixes[0])):
        for p in prefixes:
            _populate_keys(manager, worker, [p[step]], 1)


def bench_cold_load_shaped(
    manager: ShmqCertusOffloadingManager,
    worker,
    ring: Ring,
    keys: list[bytes],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    args,
    world_size: int,
    lookup_order: str = LOOKUP_FIRST,
) -> BenchResult:
    """Cold load under a serving shape: per pass, empty the tier (flush + clear),
    refill it to the eviction threshold if --cold-tier-state full, then load the
    keys as --load-unit jobs with up to --inflight outstanding. Fill and
    removal are outside the timed region; the keys must already be stored."""
    touch_first = lookup_order == TOUCH_FIRST
    phases = {name: PhaseTiming(name) for name in
              ("touch", "lookup", "prepare_load", "submit_load", "get_finished", "complete_load")}
    rng = random.Random(args.seed)
    if args.load_unit == "request":
        units_base = split_requests(keys, args.requests)
    else:
        units_base = [keys[i:i + batch_size] for i in range(0, len(keys), batch_size)]
    unit_len = max(len(u) for u in units_base)
    job_id = 2_000_000
    fill_seed = 7_000_000 + args.seed * 1000
    total_blocks, timed, passes, reads = 0, 0.0, 0, 0

    while timed < min_duration or passes < 2:
        manager._ring.flush_to_ssd()
        manager._ring.clear_memory_tier()
        fill = []
        if args.cold_tier_state == "full":
            fill_seed += 1
            fill = make_content_keys(args.tier_fill_blocks, seed=fill_seed)
            _populate_keys(manager, worker, fill, batch_size)
        units = list(units_base)
        if args.load_unit == "request":
            rng.shuffle(units)

        pending: dict[int, tuple[list[bytes], float]] = {}
        slots = list(range(args.inflight))
        slot_of: dict[int, int] = {}
        # SSD reads of the timed loads only (the flush, clear and fill are outside it).
        io_before = manager._ring.get_io_stats()
        t_pass = time.perf_counter()
        while units or pending:
            while units and len(pending) < args.inflight:
                unit = units.pop(0)
                hits = _start_load_job(
                    manager, worker, unit, slots[0] * unit_len, job_id + 1, phases,
                    touch_first=touch_first,
                    prefix_stop=args.load_unit == "request")  # vLLM stops a prefix at the first miss
                if not hits:
                    continue
                slot = slots.pop(0)
                job_id += 1
                pending[job_id] = (hits, time.perf_counter())
                slot_of[job_id] = slot
            finished = worker.get_finished()
            if not finished:
                _poll_wait()
                continue
            failed = []
            for res in finished:
                hits, t_sub = pending.pop(res.job_id)
                phases["get_finished"].record(time.perf_counter() - t_sub)
                slots.append(slot_of.pop(res.job_id))
                # Unpin every finished job, failed or not, before raising.
                t0 = time.perf_counter()
                manager.complete_load(hits)
                phases["complete_load"].record(time.perf_counter() - t0)
                if res.success:
                    total_blocks += len(hits)
                else:
                    failed.append(res.job_id)
            if failed:
                # Drain the jobs still in flight so their keys are unpinned too.
                while pending:
                    for res in worker.get_finished():
                        hits, _ = pending.pop(res.job_id)
                        manager.complete_load(hits)
                    _poll_wait()
                raise RuntimeError(f"cold load job(s) {failed} failed")
        timed += time.perf_counter() - t_pass
        reads += manager._ring.get_io_stats()["read_bytes"] - io_before["read_bytes"]
        passes += 1
        if fill:
            _remove_keys(ring, fill, world_size)

    want = total_blocks * block_bytes
    print(f"  Cold pass SSD reads: {reads / 1e6:.0f} MB for {want / 1e6:.0f} MB loaded"
          + ("" if reads else "  (0: certus-server built without rw-telemetry, "
                              "or the loads were served from DRAM)"))
    result = BenchResult(
        label=f"Cold Load Lifecycle (shaped: {cold_shape_label(args)})",
        total_blocks=total_blocks,
        total_bytes=want,
        wall_seconds=timed,
        phases=phases,
    )
    result.counts = {"disk_read_bytes": reads}
    return result


def bench_contention(manager, worker, ring, keys, block_bytes, min_duration, *, args,
                     world_size: int, lookup_order: str = LOOKUP_FIRST,
                     verifier=None) -> list[BenchResult]:
    """Loads and stores in flight together on one worker, as in serving: the load
    working set (``keys``) is stored (in --store-job-blocks jobs) and written through
    first. Then the loads run alone for CONTENTION_ALONE_SECONDS on the same manager,
    worker and working set ("Contention: Load Alone", the reference). Then, for the
    timed window, --inflight load jobs of --job-blocks keys (the working set, shuffled
    per pass) and --store-inflight store jobs of --store-job-blocks fresh keys run
    together through the step loop. Nothing is excluded: the stores' write-through to
    SSD and any eviction it forces run inside the window, as do take_events drains. A
    load key evicted by then misses (counted). Stored keys are removed after the
    window. One pool serves both directions (vLLM's single worker), so with more jobs
    than --workers threads, jobs queue in the pool: the per-job submit-to-reap time
    (get_finished) includes that wait, transfer_time does not."""
    touch_first = lookup_order == TOUCH_FIRST
    load_names = ("touch", "lookup", "prepare_load", "submit_load", "get_finished",
                  "complete_load")
    phases = _inflight_phases(*load_names, "prepare_store", "store_touch", "submit_store",
                              "complete_store", "take_events")
    units = _job_units(keys, args.job_blocks)
    _populate_jobs(manager, worker, keys, args.store_job_blocks, verifier)
    manager._ring.flush_to_ssd()

    # The reference: the same loads, nothing stored beside them.
    alone_phases = _inflight_phases(*load_names)
    alone = _StepLoop(manager, worker, alone_phases, touch_first=touch_first,
                      load_slots=args.inflight, load_job_blocks=args.job_blocks,
                      verifier=verifier)
    alone_wall = _load_window(alone, _LoadScheduler(units, args.seed + 1),
                              CONTENTION_ALONE_SECONDS, args.step_us, min_passes=1)
    ast_ = alone.load_stats

    io_before = manager._ring.get_io_stats()
    loop = _StepLoop(manager, worker, phases, touch_first=touch_first,
                     load_slots=args.inflight, load_job_blocks=args.job_blocks,
                     store_slots=args.store_inflight, store_job_blocks=args.store_job_blocks,
                     drain_events=True, verifier=verifier)
    sched = _LoadScheduler(units, args.seed)
    stored_all: list[bytes] = []
    gen = 0
    base_seed = 50_000_000 + args.seed * 100_000
    t_start = time.perf_counter()
    loop.sample(t_start)
    try:
        with loop.cleanup():
            while True:
                t_tick = time.perf_counter()
                # Stop starting jobs at a pass boundary once --min-duration and 2 passes
                # are done.
                running = not (t_tick - t_start >= min_duration and sched.passes >= 2
                               and sched.at_pass_boundary())
                loop.submit_deferred()
                if running:
                    sched.fill(loop)
                    # One reservation attempt per free slot per tick; a refused one
                    # (tier full) is counted as dropped and tried again next tick.
                    for _ in range(len(loop.free_store)):
                        gen += 1
                        fresh = make_content_keys(args.store_job_blocks, seed=base_seed + gen)
                        stored_all.extend(fresh)
                        loop.prepare_store(fresh)
                sched.done(loop.reap())
                if not running and not loop.pending and not loop.deferred:
                    break
                _step_idle(args.step_us, t_tick)
    finally:
        _remove_keys(ring, stored_all, world_size)
    wall = time.perf_counter() - t_start
    loop.sample(t_start + wall)
    io = manager._ring.get_io_stats()
    io = {k: io[k] - io_before.get(k, 0) for k in io}
    lst, sst = loop.load_stats, loop.store_stats
    load_phases = {k: phases[k] for k in load_names + ("reap_call",)}
    load_phases["get_finished"], load_phases["load_transfer"] = lst.job, lst.transfer
    # The store side's per-job submit-to-reap time under the name the parsers read.
    store_phases = {"prepare_store": phases["prepare_store"],
                    "store_touch": phases["store_touch"],
                    "submit_store": phases["submit_store"],
                    "get_finished": sst.job,
                    "complete_store": phases["complete_store"],
                    "take_events": phases["take_events"],
                    "store_transfer": sst.transfer}
    alone_phases["get_finished"], alone_phases["load_transfer"] = ast_.job, ast_.transfer
    label = inflight_label(args, "contention")
    store_r = BenchResult(label=f"Contention: Store Thread ({label})", total_blocks=sst.blocks,
                          total_bytes=sst.blocks * block_bytes, wall_seconds=wall,
                          phases=store_phases)
    store_r.counts = {"store_drops": sst.misses}
    load_r = BenchResult(label=f"Contention: Load Thread ({label})", total_blocks=lst.blocks,
                         total_bytes=lst.blocks * block_bytes, wall_seconds=wall,
                         phases=load_phases)
    load_r.counts = {"blocks_not_loaded": lst.misses, "load_misses": lst.misses,
                     "disk_read_bytes": io["read_bytes"]}
    alone_r = BenchResult(label=f"Contention: Load Alone ({label})", total_blocks=ast_.blocks,
                          total_bytes=ast_.blocks * block_bytes, wall_seconds=alone_wall,
                          phases=alone_phases)
    store_r.print_report()
    _print_inflight_summary(sst, wall, "store")
    print(f"  Store drops: {sst.misses}")
    load_r.print_report()
    _print_inflight_summary(lst, wall, "load")
    print(f"  Blocks not loaded: {lst.misses}  Eviction events: {loop.events}  "
          f"passes: {sched.passes}")
    alone_r.print_report()
    _print_inflight_summary(ast_, alone_wall, "load")
    print(f"  Load-alone blocks not loaded: {ast_.misses}  (same manager, worker and working "
          f"set as the load under the store, nothing stored beside it)")
    print(f"  Load service throughput: under the store {load_r.throughput_gbs:.2f} GB/s, alone "
          f"{alone_r.throughput_gbs:.2f} GB/s (both over their own window)")
    print(f"  Contention: load+store {(lst.blocks + sst.blocks) * block_bytes / wall / 1e9:.2f} GB/s  "
          f"SSD writes={io['write_bytes'] / 1e6:.0f} MB reads={io['read_bytes'] / 1e6:.0f} MB"
          + ("" if io["write_bytes"] else "  (SSD 0: certus-server built without rw-telemetry)"))
    if verifier is not None:
        verifier.report("contention loads (alone and under the store)")
    return [store_r, load_r]


# ── mixed store+load under eviction pressure ───────────────────────────────

def bench_mixed_eviction(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    working_set: int = 512,
    load_fraction: float = 0.5,
    lookup_order: str = LOOKUP_FIRST,
) -> BenchResult:
    """Interleaved store + load under memory-tier pressure.

    Simulates the production pattern: new blocks are continuously stored
    (pushing the LRU evictor), while earlier blocks are loaded back.  The
    working set is larger than what the tier can hold without eviction, so
    the server's evictor runs throughout.  ``take_events()`` is drained each
    iteration to surface eviction-event overhead.

    ``load_fraction`` controls what share of each iteration is loads vs stores
    (0.5 = half-and-half). Keys rotate across generations so every store is a
    new key (not a dedup hit). The load side calls lookup() and touch() in
    ``lookup_order``.
    """
    touch_first = lookup_order == TOUCH_FIRST
    phases = {
        "store_touch": PhaseTiming("store_touch"),
        "prepare_store": PhaseTiming("prepare_store"),
        "submit_store": PhaseTiming("submit_store"),
        "store_finish": PhaseTiming("store_finish"),
        "complete_store": PhaseTiming("complete_store"),
        "load_touch": PhaseTiming("load_touch"),
        "lookup": PhaseTiming("lookup"),
        "prepare_load": PhaseTiming("prepare_load"),
        "submit_load": PhaseTiming("submit_load"),
        "load_finish": PhaseTiming("load_finish"),
        "complete_load": PhaseTiming("complete_load"),
        "take_events": PhaseTiming("take_events"),
    }

    total_blocks = 0
    total_evictions = 0
    store_drops = 0
    load_misses = 0
    wall_start = time.perf_counter()
    generation = 0
    job_id = 100_000

    # Split the working set: the first load_fraction are "old" (will be loaded),
    # the rest are "new" (will be stored, pushing evictions).
    load_count = max(1, int(working_set * load_fraction))
    store_count = working_set - load_count

    print(f"\n  Eviction mode: working_set={working_set} "
          f"(store={store_count}/gen, load={load_count}/gen, "
          f"block_bytes={block_bytes})")
    print(f"  The tier fills after ~{(4 * 1024**3) // block_bytes // store_count} "
          f"generations (assuming 4G tier), then eviction kicks in.")

    # Start clean: clear DRAM and use a random base seed so keys from previous
    # runs (still on SSD in the dispatch-map) don't collide and get deduped.
    manager._ring.clear_memory_tier()
    io_before = manager._ring.get_io_stats()
    base_seed = random.randint(10_000_000, 99_000_000)

    # Seed the tier with an initial generation of loadable keys.
    seed_keys = make_content_keys(load_count, seed=base_seed)
    _populate_keys(manager, worker, seed_keys, batch_size)
    loadable_keys = list(seed_keys)

    while (time.perf_counter() - wall_start) < min_duration or generation < 3:
        generation += 1

        # ── store phase: push new keys, forcing eviction ──
        new_keys = make_content_keys(store_count, seed=base_seed + 1_000_000 * generation)
        for i in range(0, len(new_keys), batch_size):
            batch = new_keys[i : i + batch_size]

            t0 = time.perf_counter()
            manager.touch(batch)
            phases["store_touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            result = manager.prepare_store(batch)
            phases["prepare_store"].record(time.perf_counter() - t0)

            if result is None or not result.keys_to_store:
                store_drops += len(batch)
                continue

            stored = result.keys_to_store
            if len(stored) < len(batch):
                store_drops += len(batch) - len(stored)

            gpu_ids = list(range(len(stored)))
            job_id += 1

            t0 = time.perf_counter()
            worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
            phases["submit_store"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker.get_finished():
                _poll_wait()
            phases["store_finish"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager.complete_store(stored, success=True)
            phases["complete_store"].record(time.perf_counter() - t0)

            total_blocks += len(stored)

        # ── drain eviction events (the real scheduler does this) ──
        t0 = time.perf_counter()
        for ev in manager.take_events():
            total_evictions += len(ev.keys) if hasattr(ev, "keys") else 0
        phases["take_events"].record(time.perf_counter() - t0)

        # ── load phase: load from the previous generation's keys ──
        for i in range(0, len(loadable_keys), batch_size):
            batch = loadable_keys[i : i + batch_size]

            if touch_first:
                t0 = time.perf_counter()
                manager.touch(batch)
                phases["load_touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            hits = [k for k in batch if manager.lookup(k)]
            phases["lookup"].record(time.perf_counter() - t0)

            if not touch_first:
                t0 = time.perf_counter()
                manager.touch(batch)
                phases["load_touch"].record(time.perf_counter() - t0)

            if not hits:
                load_misses += len(batch)
                continue
            load_misses += len(batch) - len(hits)

            t0 = time.perf_counter()
            load_spec = manager.prepare_load(hits)
            phases["prepare_load"].record(time.perf_counter() - t0)

            gpu_ids = list(range(len(hits)))
            job_id += 1

            t0 = time.perf_counter()
            worker.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
            phases["submit_load"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker.get_finished():
                _poll_wait()
            phases["load_finish"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager.complete_load(hits)
            phases["complete_load"].record(time.perf_counter() - t0)

            total_blocks += len(hits)

        # Rotate: this generation's stored keys become next generation's load targets.
        loadable_keys = list(new_keys)

        # Give the background writer time to flush write-throughs so entries
        # become evictable. Without this, the store loop outruns the bg writer
        # and every Reserve fails (entries never get ssd_offset set → can't be
        # demoted → AllocationFailed → store drops). 200ms is enough for the
        # bg writer to flush ~1024 entries across 4 NVMe drives.
        if store_drops > 0 and generation >= 3:
            time.sleep(0.2)

    wall = time.perf_counter() - wall_start
    # Collect SSD I/O stats delta to show write-through (demotion) and promote activity.
    io_after = manager._ring.get_io_stats()
    io_after = {k: io_after[k] - io_before.get(k, 0) for k in io_after}

    result = BenchResult(
        label=(
            f"Mixed Store+Load Under Eviction Pressure "
            f"(ws={working_set}, load_frac={load_fraction:.0%})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.counts = {"store_drops": store_drops, "load_misses": load_misses,
                     "disk_read_bytes": io_after["read_bytes"]}
    result.print_report()
    print(f"  Eviction events (REMOVED): {total_evictions}  "
          f"Store drops (tier full): {store_drops}  "
          f"Load misses (evicted): {load_misses}")
    print(f"  SSD I/O: writes={io_after['write_ops']} ({io_after['write_bytes']/1e6:.0f} MB)  "
          f"reads={io_after['read_ops']} ({io_after['read_bytes']/1e6:.0f} MB)")
    if store_drops > 0 and io_after['write_ops'] == 0:
        print(f"  NOTE: Store drops with zero SSD writes means background DRAM→SSD")
        print(f"  write-through is not completing. Entries never become evictable.")
        print(f"  Check server --memory-tier-eviction-threshold and drive health.")
    return result


# ── mode: prefix-miss ────────────────────────────────────────────────────────

def bench_prefix_miss(
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    hit_ratio: float = 0.75,
    num_blocks: int = 128,
    lookup_order: str = LOOKUP_FIRST,
) -> BenchResult:
    """Measure the cost of vLLM's _maximal_prefix_lookup at varying hit ratios.

    Pre-stores ``hit_ratio`` fraction of keys, then repeatedly runs a per-key
    lookup breaking at the first miss, followed by touch(all) (or touch(all)
    first with ``lookup_order=TOUCH_FIRST``).  Tracks the latency of lookups
    answering hit and miss separately.
    """
    touch_first = lookup_order == TOUCH_FIRST
    all_keys = make_content_keys(num_blocks)
    present_count = max(0, min(num_blocks, int(num_blocks * hit_ratio)))
    present_keys = all_keys[:present_count]
    absent_keys = all_keys[present_count:]

    # Pre-store the "prefix" keys.
    if present_keys:
        _populate_keys(manager, worker, present_keys, batch_size)

    phases = {
        "touch": PhaseTiming("touch"),
        "lookup_hit": PhaseTiming("lookup_hit"),
        "lookup_miss": PhaseTiming("lookup_miss"),
        "prefix_scan": PhaseTiming("prefix_scan"),
        "load": PhaseTiming("load"),
        "store": PhaseTiming("store"),
    }

    total_scans = 0
    total_prefix_hits = 0
    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 700_000

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for batch_start in range(0, num_blocks, batch_size):
            batch = all_keys[batch_start : batch_start + batch_size]

            # vLLM <= 0.20: touch the full batch first (its reply answers the
            # scan's lookups).
            if touch_first:
                t0 = time.perf_counter()
                manager.touch(batch)
                phases["touch"].record(time.perf_counter() - t0)

            # Maximal prefix scan: per-key lookup, break at first miss.
            t_scan = time.perf_counter()
            prefix_hits = []
            for k in batch:
                t_lk = time.perf_counter()
                hit = manager.lookup(k)
                elapsed = time.perf_counter() - t_lk
                if hit:
                    phases["lookup_hit"].record(elapsed)
                    prefix_hits.append(k)
                else:
                    phases["lookup_miss"].record(elapsed)
                    break
            phases["prefix_scan"].record(time.perf_counter() - t_scan)

            # vLLM >= 0.21: touch the full batch after the scan.
            if not touch_first:
                t0 = time.perf_counter()
                manager.touch(batch)
                phases["touch"].record(time.perf_counter() - t0)

            total_scans += 1
            total_prefix_hits += len(prefix_hits)

            # Complete the lifecycle: load hits, store the first miss.
            if prefix_hits:
                t0 = time.perf_counter()
                load_spec = manager.prepare_load(prefix_hits)
                gpu_ids = list(range(len(prefix_hits)))
                job_id += 1
                worker.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
                while not worker.get_finished():
                    _poll_wait()
                manager.complete_load(prefix_hits)
                phases["load"].record(time.perf_counter() - t0)
                total_blocks += len(prefix_hits)

        iteration += 1

    wall = time.perf_counter() - wall_start
    avg_prefix = total_prefix_hits / total_scans if total_scans else 0
    result = BenchResult(
        label=(
            f"Prefix-Miss Scan (hit_ratio={hit_ratio:.0%}, "
            f"avg_prefix={avg_prefix:.1f}/{batch_size})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.print_report()
    print(f"  Scans={total_scans}  avg_prefix_len={avg_prefix:.1f}  "
          f"lookup_cache_hits={len(phases['lookup_hit'].samples)}  "
          f"lookup_cache_misses={len(phases['lookup_miss'].samples)}")

    # Clean up stored keys.
    if present_keys:
        _remove_keys(manager._ring, present_keys, manager._world_size)

    return result


# ── mode: scheduler-step ─────────────────────────────────────────────────────

def bench_scheduler_step(
    manager: ShmqCertusOffloadingManager,
    worker,
    ring: Ring,
    batch_size: int,
    block_bytes: int,
    min_duration: float,
    *,
    requests_per_step: int = 8,
    inflight: int = 4,
    step_us: float = 100.0,
    num_sessions: int = 32,
    num_blocks: int = 128,
    lookup_order: str = LOOKUP_FIRST,
    prefix_sharing: str = "shared",
) -> BenchResult:
    """Continuous-batching scheduler steps, driven as vLLM 0.30's offloading connector
    is (offloading/scheduler.py and worker.py), on the step loop.

    Each step admits up to ``requests_per_step`` new requests from the session pool.
    Each request does a maximal prefix scan (per-key lookup, stopping at the first
    miss) and then touches its keys (touch first with ``lookup_order=TOUCH_FIRST``,
    vLLM <= 0.20). Its prefix hits become ONE load job of its own (vLLM:
    update_state_after_alloc, one job per request), submitted that step without
    waiting; at most ``inflight`` load jobs are outstanding, and a request that would
    exceed that stays queued for a later step (vLLM's scheduler bounds its running
    requests the same way). Hit keys another outstanding job is already loading are
    not loaded again: the request waits for that job (vLLM delays a request whose hit
    chunks are being loaded, then finds them in the GPU prefix cache). Once a
    request's loads are reaped it computes its suffix: its store is prepared in that
    step (prepare_store, then touch, as _build_store_jobs does) and submitted at the
    start of the next (prepare_store_kv). Finished jobs are reaped by one non-blocking
    get_finished() per tick; each tick then sleeps to the next ``step_us`` boundary.
    Nothing waits for a particular job.

    ``prefix_sharing="shared"`` gives every session a slice of one shared prefix pool
    (like a common system prompt); ``"private"`` gives each session its own prefix
    blocks, so no prefix block is shared between sessions. All prefix blocks are
    stored up front. Keys are never removed between steps: the cache fills and
    evicts naturally.

    Throughput counts blocks loaded plus blocks stored over the timed window, which
    ends when the last outstanding job is reaped.
    """
    touch_first = lookup_order == TOUCH_FIRST
    keys_per_request = max(4, num_blocks // requests_per_step)
    prefix_len = keys_per_request // 2
    suffix_len = keys_per_request - prefix_len

    phases = _inflight_phases("touch", "lookup", "prepare_load", "submit_load",
                              "load_job", "complete_load", "prepare_store",
                              "store_touch", "submit_store", "store_job",
                              "complete_store", "take_events", "step")

    # Session prefixes: slices of one shared pool, geometric lengths (short prefixes
    # miss more), or private blocks of the same lengths.
    max_prefix_pool = min(prefix_len * 4, num_blocks * 2)
    shared_prefix_pool = make_content_keys(max_prefix_pool, seed=77777)
    session_prefixes: list[list[bytes]] = []
    rng = random.Random(42)
    for s in range(num_sessions):
        geo_len = max(2, min(len(shared_prefix_pool), int(rng.expovariate(1.0 / prefix_len))))
        if prefix_sharing == "private":
            # Seeds far above the suffix seeds below, so no key is reused.
            session_prefixes.append(make_content_keys(geo_len, seed=1_000_000_000 + s))
        else:
            session_prefixes.append(shared_prefix_pool[:geo_len])
    prefix_keys = (
        [k for p in session_prefixes for k in p]
        if prefix_sharing == "private" else shared_prefix_pool
    )
    _populate_keys(manager, worker, prefix_keys, batch_size)
    manager._ring.flush_to_ssd()   # the populate's write-through must not overlap the steps

    # One GPU slot per job, as large as a request (its longest prefix plus suffix).
    # A request whose store finds no free slot keeps its place and stores in a
    # later step; admission stops while requests_per_step requests are queued.
    slot_blocks, load_slots, store_slots = sched_step_slots(
        num_blocks, requests_per_step, inflight)
    loop = _StepLoop(manager, worker, phases, touch_first=touch_first,
                     load_slots=load_slots, load_job_blocks=slot_blocks,
                     store_slots=store_slots, store_job_blocks=slot_blocks,
                     drain_events=True)

    next_req = 0            # requests are numbered; request r is session r % num_sessions
    loading: dict[bytes, int] = {}      # key -> outstanding load job loading it
    job_of: dict[int, int] = {}         # request -> its outstanding load job
    blocked: dict[int, list[bytes]] = {}  # request loading its hits -> blocks it then stores
    delayed: dict[int, tuple[set[int], set[bytes]]] = {}  # request -> (jobs it waits on,
                                        # keys those jobs loaded: now in the GPU prefix cache)
    computable: list[tuple[int, list[bytes]]] = []        # loads done: blocks to store
    total_steps = 0
    total_prefix_hits = 0
    total_suffix_stores = 0
    total_requests = 0
    total_delays = 0

    def request_keys(r: int) -> list[bytes]:
        # A seed of its own per request (above the prefix seeds: 77777 and
        # 1_000_000_000 + session). The old seed, session x 10,000 + 5,000 + step x 100
        # + index, collided once the step passed 100 (a few seconds in): requests got
        # other requests' suffix keys, which then hit at lookup or were not reserved
        # again for store.
        suffix = make_content_keys(suffix_len, seed=2_000_000_000 + r)
        return session_prefixes[r % num_sessions] + suffix

    def admit(r: int, gpu_cached: set[bytes]) -> str:
        """One scheduling attempt for request r, as get_num_new_matched_tokens and
        update_state_after_alloc make it: skip its leading blocks already in the GPU
        prefix cache (``gpu_cached``: loaded by the jobs it waited on), look up the
        rest (maximal prefix), touch (also on a delayed attempt, as vLLM does), then
          "delayed": a hit is being loaded by another job; retried when those end;
          "no_slot": it has hits to load and --inflight jobs are outstanding;
          "loading": its hits are one load job of its own;
          "ready":   nothing to load: it computes (stores) this step."""
        nonlocal total_prefix_hits, total_suffix_stores, total_requests, total_delays
        req_keys = request_keys(r)
        start = 0
        while start < len(req_keys) and req_keys[start] in gpu_cached:
            start += 1
        rest = req_keys[start:]
        t0 = time.perf_counter()
        if touch_first:
            manager.touch(req_keys)
            phases["touch"].record(time.perf_counter() - t0)
            t0 = time.perf_counter()
        hits = []
        for k in rest:
            if not manager.lookup(k):
                break
            hits.append(k)
        phases["lookup"].record(time.perf_counter() - t0)
        if not touch_first:
            t0 = time.perf_counter()
            manager.touch(req_keys)
            phases["touch"].record(time.perf_counter() - t0)
        busy = {loading[k] for k in hits if k in loading}
        if busy:
            total_delays += 1
            delayed[r] = (busy, set(gpu_cached))
            return "delayed"
        if hits and not loop.load_slot_free():
            return "no_slot"
        missing = rest[len(hits):]
        total_prefix_hits += start + len(hits)
        total_suffix_stores += len(missing)
        total_requests += 1
        if hits:
            loop.start_load_hits(hits, tag=r)
            jid = job_of[r] = loop.job_id
            for k in hits:
                loading[k] = jid
            blocked[r] = missing
            return "loading"
        computable.append((r, missing))
        return "ready"

    stored_keys: list[bytes] = []              # every suffix block stored, for the cleanup
    retry: list[tuple[int, set[bytes]]] = []   # delayed requests whose blockers ended

    wall_start = time.perf_counter()
    loop.sample(wall_start)
    with loop.cleanup():
        while True:
            t_tick = time.perf_counter()
            running = (t_tick - wall_start) < min_duration or total_steps < 3
            # Start of the step: stores deferred from the last step, then schedule:
            # requests already waiting first (vLLM's waiting queue is FCFS), then new
            # ones, up to --requests-per-step per step and a bounded backlog.
            loop.submit_deferred()
            for i, (r, cached) in enumerate(retry):
                if admit(r, cached) == "no_slot":
                    retry = retry[i:]
                    break
            else:
                retry = []
            admitted = 0
            backlog = len(computable) + len(delayed) + len(blocked)
            while (running and not retry and admitted < requests_per_step
                   and backlog < requests_per_step + inflight):
                if admit(next_req, set()) == "no_slot":
                    break             # next_req is tried again next step
                next_req += 1
                admitted += 1
                backlog += 1
            # Requests whose loads are done compute; their stores go out next step.
            while computable and loop.store_slot_free():
                r, missing = computable.pop(0)
                if missing:
                    stored_keys.extend(missing)   # removed when the run ends
                    loop.prepare_store(missing, tag=r)
            phases["step"].record(time.perf_counter() - t_tick)
            # One non-blocking reap per tick.
            for kind, keys, tag in loop.reap():
                if kind != "load":
                    continue
                jid = job_of.pop(tag)
                for k in keys:
                    if loading.get(k) == jid:
                        del loading[k]
                computable.append((tag, blocked.pop(tag)))
                for r in [r for r, (aw, _c) in delayed.items() if jid in aw]:
                    aw, cached = delayed[r]
                    aw.discard(jid)
                    cached.update(keys)
                    if not aw:
                        del delayed[r]
                        retry.append((r, cached))
            total_steps += 1
            if not running and not (delayed or retry or blocked or computable
                                    or loop.deferred or loop.pending):
                break
            _step_idle(step_us, t_tick)
    wall = time.perf_counter() - wall_start
    loop.sample(wall_start + wall)

    ls, ss = loop.load_stats, loop.store_stats
    phases["load_job"], phases["store_job"] = ls.job, ss.job
    phases["load_transfer"], phases["store_transfer"] = ls.transfer, ss.transfer
    total_blocks = ls.blocks + ss.blocks
    result = BenchResult(
        label=(
            f"Scheduler Step (reqs={requests_per_step}, "
            f"sessions={num_sessions}, keys/req={keys_per_request}, "
            f"inflight={inflight}, step_us={step_us:g})"
        ),
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )
    result.counts = {"store_drops": ss.misses}
    result.print_report()
    print(f"  Steps={total_steps}  steps/s={total_steps/wall:.1f}  "
          f"requests={total_requests}  requests/s={total_requests/wall:.1f}  "
          f"prefix_hits={total_prefix_hits}  suffix_stores={total_suffix_stores}  "
          f"delays (hit being loaded by another job)={total_delays}  "
          f"loaded={ls.blocks}  stored={ss.blocks}  store_drops={ss.misses}  "
          f"lookup_order={lookup_order}  prefix_sharing={prefix_sharing}")
    _print_inflight_summary(ls, wall, "load")
    _print_inflight_summary(ss, wall, "store")

    # Remove everything this run stored (the prefix pool and the suffixes), so a later
    # run in the same server session starts from the same state.
    _remove_keys(ring, prefix_keys + stored_keys, manager._world_size)

    return result


def sched_step_slots(num_blocks: int, requests_per_step: int, inflight: int):
    """scheduler-step's GPU slots: (blocks per slot, load slots, store slots). A slot
    holds one request: its longest prefix (the shared pool, 4 x half a request, at
    most 2 x --num-blocks) plus its suffix. Store slots: one step's requests plus
    the loads that can finish in one reap."""
    kpr = max(4, num_blocks // requests_per_step)
    prefix_len = kpr // 2
    slot = min(prefix_len * 4, num_blocks * 2) + (kpr - prefix_len)
    return slot, inflight, requests_per_step + inflight


# ── pattern-driven mode (same YAML patterns as certus_fio) ──────────────────

def _load_pattern(name: str, overrides: dict | None = None):
    """Load a workload pattern YAML, reusing certus_fio's WorkloadPattern."""
    sys.path.insert(0, os.path.join(_repo, "tools", "certus-fio"))
    from certus_fio import WorkloadPattern, eval_expr
    patterns_dir = os.path.join(_repo, "knowledge", "workload_patterns")
    path = os.path.join(patterns_dir, f"{name}.yaml")
    if not os.path.exists(path):
        candidates = [f[:-5] for f in os.listdir(patterns_dir) if f.endswith(".yaml")]
        print(f"Pattern '{name}' not found. Available:", file=sys.stderr)
        for c in sorted(candidates):
            print(f"  {c}", file=sys.stderr)
        sys.exit(1)
    return WorkloadPattern(path, overrides), eval_expr


def bench_pattern(
    pattern,
    eval_expr_fn,
    manager: ShmqCertusOffloadingManager,
    worker,
    kv_regions: list[KvCacheIpc],
    ring: Ring,
    batch_size: int,
    block_bytes_override: int | None,
    min_duration: float,
    tp: int,
    lookup_order: str = LOOKUP_FIRST,
) -> list[BenchResult]:
    """Run a certus_fio YAML pattern through the connector lifecycle.

    Translates each pattern phase's store/load/store_3phase/load_3phase ops
    into the full manager+worker lifecycle, using the pattern's keyspace
    sizing and parameters. Integer keys are used (same as certus_fio) so the
    numbers are directly comparable — the only difference is the code path.
    """
    results = []
    key_base = 10_000_000

    for phase_def in pattern.phases:
        phase_id = phase_def["id"]
        ops = phase_def.get("operations", [])

        for op_def in ops:
            op = op_def["op"]
            ks_name = op_def.get("keys", list(pattern.keyspaces.keys())[0])
            ks = pattern.keyspaces[ks_name]
            object_bytes = block_bytes_override or ks["object_bytes"]
            cardinality = ks["cardinality"]
            repeat = eval_expr_fn(op_def.get("repeat", cardinality), pattern.params)
            bs = eval_expr_fn(op_def.get("batch_size", batch_size), pattern.params)
            if bs > repeat:
                bs = repeat

            ks_offset = list(pattern.keyspaces.keys()).index(ks_name) * 10_000_000
            all_keys = list(range(key_base + ks_offset, key_base + ks_offset + cardinality))
            # Cycle keys to reach repeat count (same as certus_fio).
            import itertools
            keys_int = list(itertools.islice(itertools.cycle(all_keys), repeat))

            # Wrap ints as the manager expects (it calls _key_to_u64, which
            # passes ints through unchanged — same path as certus_fio's
            # integer keys, no BLAKE2b hashing overhead).
            keys = keys_int

            # Update manager block size to match pattern's object_bytes.
            manager.set_block_size_bytes(object_bytes)

            if op in ("store", "store_3phase"):
                # Precondition: keys should not exist. Clean up first.
                _remove_int_keys(ring, all_keys, tp)

                result = _bench_pattern_store(
                    manager, worker, keys, bs, object_bytes, min_duration, ring, tp,
                )
                result.label = f"[{pattern.id}] {phase_id}/{op} (connector lifecycle)"
                result.print_report()
                results.append(result)

            elif op in ("load", "load_3phase"):
                # Precondition check: if pattern says present_in_store, populate.
                for pc in pattern.preconditions:
                    if pc["subject"] == ks_name and pc["state"] == "present_in_store":
                        _populate_int_keys(manager, worker, all_keys, bs, ring, tp)
                        break

                # Check if this is a cold-load pattern (keys absent from local cache).
                is_cold = any(
                    pc["subject"] == ks_name and pc["state"] == "absent_from_local_cache"
                    for pc in pattern.preconditions
                )

                result = _bench_pattern_load(
                    manager, worker, keys, bs, object_bytes, min_duration,
                    cold=is_cold, lookup_order=lookup_order,
                )
                result.label = f"[{pattern.id}] {phase_id}/{op} {'cold' if is_cold else 'warm'} (connector lifecycle)"
                result.print_report()
                results.append(result)

        # Clean up between phases.
        for ks_name, ks in pattern.keyspaces.items():
            ks_offset = list(pattern.keyspaces.keys()).index(ks_name) * 10_000_000
            ks_keys = list(range(key_base + ks_offset, key_base + ks_offset + ks["cardinality"]))
            _remove_int_keys(ring, ks_keys, tp)

    return results


def _bench_pattern_store(manager, worker, keys, batch_size, block_bytes, min_duration, ring, tp):
    """Store keys through the full connector lifecycle, with certus_fio-compatible timing."""
    phases = {
        "touch": PhaseTiming("touch"),
        "prepare_store": PhaseTiming("prepare_store"),
        "submit_store": PhaseTiming("submit_store"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_store": PhaseTiming("complete_store"),
    }
    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 200_000

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for i in range(0, len(keys), batch_size):
            batch = keys[i : i + batch_size]

            t0 = time.perf_counter()
            manager.touch(batch)
            phases["touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            result = manager.prepare_store(batch)
            phases["prepare_store"].record(time.perf_counter() - t0)

            if result is None or not result.keys_to_store:
                continue

            stored = result.keys_to_store
            gpu_ids = list(range(len(stored)))
            job_id += 1

            t0 = time.perf_counter()
            worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
            phases["submit_store"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker.get_finished():
                _poll_wait()
            phases["get_finished"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager.complete_store(stored, success=True)
            phases["complete_store"].record(time.perf_counter() - t0)

            total_blocks += len(stored)

        iteration += 1
        if (time.perf_counter() - wall_start) < min_duration:
            _remove_int_keys(ring, keys, tp)

    wall = time.perf_counter() - wall_start
    return BenchResult(
        label="pattern store",
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )


def _bench_pattern_load(manager, worker, keys, batch_size, block_bytes, min_duration, cold=False,
                        lookup_order=LOOKUP_FIRST):
    """Load keys through the full connector lifecycle (lookup/touch in ``lookup_order``)."""
    touch_first = lookup_order == TOUCH_FIRST
    phases = {
        "touch": PhaseTiming("touch"),
        "lookup": PhaseTiming("lookup"),
        "prepare_load": PhaseTiming("prepare_load"),
        "submit_load": PhaseTiming("submit_load"),
        "get_finished": PhaseTiming("get_finished"),
        "complete_load": PhaseTiming("complete_load"),
    }
    total_blocks = 0
    wall_start = time.perf_counter()
    iteration = 0
    job_id = 300_000

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        if cold and iteration > 0:
            try:
                manager._ring.flush_to_ssd()
                manager._ring.clear_memory_tier()
            except Exception:
                pass

        for i in range(0, len(keys), batch_size):
            batch = keys[i : i + batch_size]

            if touch_first:
                t0 = time.perf_counter()
                manager.touch(batch)
                phases["touch"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            hits = [k for k in batch if manager.lookup(k)]
            phases["lookup"].record(time.perf_counter() - t0)

            if not touch_first:
                t0 = time.perf_counter()
                manager.touch(batch)
                phases["touch"].record(time.perf_counter() - t0)

            if not hits:
                continue

            t0 = time.perf_counter()
            load_spec = manager.prepare_load(hits)
            phases["prepare_load"].record(time.perf_counter() - t0)

            gpu_ids = list(range(len(hits)))
            job_id += 1

            t0 = time.perf_counter()
            worker.submit_load(job_id, load_spec, _FakeGPUSpec(gpu_ids))
            phases["submit_load"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            while not worker.get_finished():
                _poll_wait()
            phases["get_finished"].record(time.perf_counter() - t0)

            t0 = time.perf_counter()
            manager.complete_load(hits)
            phases["complete_load"].record(time.perf_counter() - t0)

            total_blocks += len(hits)

        iteration += 1

    wall = time.perf_counter() - wall_start
    return BenchResult(
        label="pattern load",
        total_blocks=total_blocks,
        total_bytes=total_blocks * block_bytes,
        wall_seconds=wall,
        phases=phases,
    )


def _remove_int_keys(ring: Ring, keys: list[int], world_size: int):
    """Remove integer keys (no BLAKE2b hashing — same keyspace as certus_fio)."""
    from certus_shmq_connector.mediums import ns_key
    expanded = []
    for k in keys:
        for r in range(world_size):
            expanded.append(ns_key(k, r, world_size))
    if expanded:
        try:
            ring.remove(expanded)
        except Exception:
            pass


def _populate_int_keys(manager, worker, keys, batch_size, ring, tp):
    """Store integer keys so they're available for load benchmarks."""
    _remove_int_keys(ring, keys, tp)
    job_id = 400_000
    for i in range(0, len(keys), batch_size):
        batch = keys[i : i + batch_size]
        manager.touch(batch)
        result = manager.prepare_store(batch)
        if result is None or not result.keys_to_store:
            continue
        stored = result.keys_to_store
        gpu_ids = list(range(len(stored)))
        job_id += 1
        worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
        while not worker.get_finished():
            _poll_wait()
        manager.complete_store(stored, success=True)


# ── helpers ─────────────────────────────────────────────────────────────────

class _FakeGPUSpec:
    """Minimal stand-in for GPULoadStoreSpec. The worker only reads .block_ids."""
    def __init__(self, block_ids: list[int]):
        self.block_ids = block_ids


# ── real tensor parallelism (--gpus) ────────────────────────────────────────
#
# vLLM under TP=W runs one scheduler-side manager and W workers, one per GPU.
# The scheduler hands every worker the SAME transfer spec (logical keys + GPU
# block ids); each worker folds its own rank into the keys and moves its own
# shard, and a job is finished only once every rank reports it. _TPWorkerGroup
# reproduces that: rank 0 runs in this process, ranks 1..W-1 in spawned
# processes on their own GPUs, behind the same submit_store / submit_load /
# get_finished surface the benchmarks already drive.


def _tp_rank_main(conn, rank, world_size, gpu, shm_path, block_bytes,
                  max_blocks, workers, layers=1):
    """Entry point of one non-zero TP rank (spawned process)."""
    torch.cuda.set_device(gpu)
    # Each process claims channels only from its own slot, as the production
    # connector does: claims are not atomic across processes.
    ring = Ring(shm_path, ready_timeout=10.0, log=lambda msg: None,
                claim_slot=rank, claim_slots=world_size)
    # This thread only relays messages; the pool threads do the ring I/O.
    ring.release_channel()
    regions, _tensors = make_kv_regions(gpu, block_bytes, max_blocks, layers)
    executor = ThreadPoolExecutor(max_workers=workers,
                                  thread_name_prefix=f"bench-shmq-r{rank}")
    worker = worker_class()(ring, regions, block_bytes, executor,
                            rank=rank, world_size=world_size)
    conn.send(("ready", rank))
    inflight = 0
    try:
        while True:
            # Poll briefly while jobs are in flight, otherwise park on the pipe.
            if conn.poll(0.0001 if inflight else 0.05):
                msg = conn.recv()
                if msg[0] == "stop":
                    break
                op, job_id, block_ids, keys = msg
                spec = CertusLoadStoreSpec([BlockLocation(key=k) for k in keys])
                if op == "store":
                    worker.submit_store(job_id, _FakeGPUSpec(block_ids), spec)
                else:
                    worker.submit_load(job_id, spec, _FakeGPUSpec(block_ids))
                inflight += 1
            for res in worker.get_finished():
                conn.send((res.job_id, bool(res.success)))
                inflight -= 1
    finally:
        executor.shutdown(wait=True)
        ring.close()


class _TPWorkerGroup:
    """W CertusShmqWorkers, one per GPU, behind the single-worker interface.

    ``submit_*`` fans the job out to every rank; ``get_finished`` reports a job
    once all ranks have finished it, successful only if every rank succeeded,
    with ``transfer_size`` counting all W shards."""

    _READY_TIMEOUT_S = 60.0

    def __init__(self, ring, regions, block_bytes, executor, *, gpus,
                 shm_path, max_blocks, workers, layers=1):
        self._world_size = len(gpus)
        self._block_bytes = block_bytes
        self._rank0 = worker_class()(ring, regions, block_bytes, executor,
                                     rank=0, world_size=self._world_size)
        self._pending: dict[int, list] = {}  # job_id -> [remaining, ok, t0, n, type]
        self._done: collections.deque = collections.deque()
        self._conns = []
        self._procs = []
        ctx = __import__("multiprocessing").get_context("spawn")
        for rank in range(1, self._world_size):
            parent, child = ctx.Pipe()
            proc = ctx.Process(
                target=_tp_rank_main,
                args=(child, rank, self._world_size, gpus[rank], shm_path,
                      block_bytes, max_blocks, workers, layers),
                daemon=True,
            )
            proc.start()
            self._conns.append(parent)
            self._procs.append(proc)
        for rank, (conn, proc) in enumerate(zip(self._conns, self._procs), start=1):
            if not conn.poll(self._READY_TIMEOUT_S):
                self.close()
                raise RuntimeError(f"TP rank {rank} did not start "
                                   f"(exitcode={proc.exitcode})")
            conn.recv()

    def _submit(self, op, job_id, gpu_spec, certus_spec):
        block_ids = list(gpu_spec.block_ids)
        keys = list(certus_spec.keys)
        self._pending[job_id] = [
            self._world_size, True, time.monotonic(), len(block_ids),
            _STORE_TYPE if op == "store" else _LOAD_TYPE,
        ]
        for conn in self._conns:
            conn.send((op, job_id, block_ids, keys))
        if op == "store":
            return self._rank0.submit_store(job_id, gpu_spec, certus_spec)
        return self._rank0.submit_load(job_id, certus_spec, gpu_spec)

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
            _, ok, t0, n, ttype = entry
            # Resolved per call: the no-vLLM shim patches it in after import.
            self._done.append(_handler_mod.make_transfer_result(
                job_id=job_id, success=ok,
                transfer_size=n * self._block_bytes * self._world_size,
                transfer_time=time.monotonic() - t0, transfer_type=ttype,
            ))

    def get_finished(self):
        for res in self._rank0.get_finished():
            self._rank_done(res.job_id, bool(res.success))
        for conn in self._conns:
            while conn.poll():
                job_id, success = conn.recv()
                self._rank_done(job_id, success)
        out = list(self._done)
        self._done.clear()
        return out

    def close(self):
        for conn in self._conns:
            try:
                conn.send(("stop",))
            except (BrokenPipeError, OSError):
                pass
        for proc in self._procs:
            proc.join(timeout=10)
            if proc.is_alive():
                proc.terminate()
        self._conns.clear()
        self._procs.clear()


def _remove_keys(ring: Ring, keys: list[bytes], world_size: int):
    """Remove all server entries for these keys (all TP ranks)."""
    from certus_shmq_connector.manager import _key_to_u64
    from certus_shmq_connector.mediums import ns_key

    int_keys = [_key_to_u64(k) for k in keys]
    expanded = []
    for k in int_keys:
        for r in range(world_size):
            expanded.append(ns_key(k, r, world_size))
    if expanded:
        try:
            ring.remove(expanded)
        except Exception:
            pass


def _populate_keys(
    manager: ShmqCertusOffloadingManager,
    worker,
    keys: list[bytes],
    batch_size: int,
):
    """Store all keys so they're available for load benchmarks."""
    job_id = 500_000
    for i in range(0, len(keys), batch_size):
        batch = keys[i : i + batch_size]
        manager.touch(batch)
        result = manager.prepare_store(batch)
        if result is None or not result.keys_to_store:
            continue
        stored = result.keys_to_store
        gpu_ids = list(range(len(stored)))
        job_id += 1
        worker.submit_store(job_id, _FakeGPUSpec(gpu_ids), result.store_spec)
        res = []
        while not res:
            res = worker.get_finished()
            if not res:
                _poll_wait()
        ok = all(r.success for r in res)
        manager.complete_store(stored, success=ok)
        if not ok:
            raise RuntimeError(f"populate: store job {job_id} failed")


# ── main ────────────────────────────────────────────────────────────────────

def _explicit_flags(parser, argv=None) -> dict:
    """The flags given on the command line (a --preset or --shape fills only the
    others). Re-parses with every default suppressed: comparing against the default
    would miss a flag set to its default value."""
    given = argparse.ArgumentParser(add_help=False)
    for action in parser._actions:
        if action.option_strings and action.dest != "help":
            kw = {"dest": action.dest, "default": argparse.SUPPRESS}
            if isinstance(action, argparse._StoreTrueAction):
                kw["action"] = "store_true"
            elif action.nargs is not None:
                kw["nargs"] = action.nargs
            given.add_argument(*action.option_strings, **kw)
    return vars(given.parse_known_args(argv)[0])


# Every mode --mode all runs, in order.
ALL_MODES = ["default", "prefix-miss", "contention", "scheduler-step"]


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="End-to-end connector lifecycle benchmark",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Modes (--mode):\n"
            "\n"
            "  default        Store, warm load and cold load with --inflight jobs of --job-blocks\n"
            "                 outstanding (vLLM 0.30's step loop), then mixed eviction\n"
            "  prefix-miss    Vary prefix hit ratio to measure _maximal_prefix_lookup cost\n"
            "  contention     Loads and stores in flight together on one worker, write-through\n"
            "                 live, after a load-alone reference\n"
            "  scheduler-step Full scheduler step simulation with multi-request prefix sharing\n"
            "\n"
            "Without --mode, runs the default mode (or --pattern if given).\n"
            "--job-blocks 16 --inflight 1 runs default mode's passes one 16-block job at a time.\n"
            "\n"
            "Unlike certus_fio (which calls Ring.lookup directly), this exercises\n"
            "key namespacing, region construction, ThreadPoolExecutor dispatch,\n"
            "BLAKE2b key hashing, the lookup cache, and eviction-event draining."
        ),
    )
    parser.add_argument("--pattern", type=str, default=None,
                        help="Run a certus_fio YAML pattern through the connector "
                        "(e.g. cold_prefill_store, warm_prefill_load_and_suffix_store)")
    parser.add_argument("--override", nargs="*", default=[],
                        help="Pattern parameter overrides (e.g. requested_blocks=256)")
    parser.add_argument("--shm-path", default="/dev/shm/certus-shmq",
                        help="certus-server shmq mailbox path")
    parser.add_argument("--bs", type=int, default=16,
                        help="Batch size (blocks per batch) of the prefix-miss, "
                             "scheduler-step and pattern modes and of mixed eviction; the "
                             "default mode's store, warm and cold passes and contention use "
                             "--job-blocks instead")
    parser.add_argument("--num-blocks", type=int, default=128,
                        help="Total blocks per iteration: the size of the working set, the "
                             "distinct blocks (keys) a pass cycles through. The default and "
                             "contention modes cut it into jobs of --job-blocks (the last job "
                             "takes the remainder, so it need not divide evenly) and need at "
                             "least --inflight jobs; default there: 90%% of what the tier holds "
                             "before evicting (--tier-gib, --eviction-threshold), at most 4000 "
                             "(4000 on a 12 GiB tier). Default 128 in the other modes")
    parser.add_argument("--block-bytes", type=int, default=DEFAULT_BLOCK_BYTES,
                        help=f"Per-block size in bytes (default: {DEFAULT_BLOCK_BYTES})")
    parser.add_argument("--pieces-per-block", "--layers", dest="pieces_per_block",
                        type=int, default=None,
                        help="KV regions per block: N GPU tensors of block_bytes/N "
                             "each, one per distinct layer view as vLLM lays out KV "
                             "under its default LBHNC layout (default 32 in the default and "
                             "contention modes, Llama-3-8B under vLLM 0.30; 1 in the others: "
                             "one contiguous region, as under BLHNC or vLLM 0.20/0.22). "
                             "--layers is the same flag, the name the cputier, lmcache and 3fs "
                             "benches use for it")
    # ── cold-load shape (default mode's cold pass) ──
    parser.add_argument("--cold-tier-state", choices=("empty", "full"), default="empty",
                        help="memory tier at the start of each cold pass: empty (cleared, "
                             "the default) or full (refilled with --tier-fill-blocks "
                             "other blocks, outside the timing, so cold reads must evict)")
    parser.add_argument("--tier-gib", type=float, default=12.0,
                        help="the server's --memory-tier-size in GiB (default 12, what the targets "
                             "start it with). The benchmark cannot read it from the server, so a "
                             "wrong value is not detected: it is used to refuse a working set that "
                             "cannot stay resident and to size --tier-fill-blocks")
    parser.add_argument("--eviction-threshold", type=float, default=0.8,
                        help="the server's --memory-tier-eviction-threshold (default 0.8)")
    parser.add_argument("--tier-fill-blocks", type=int, default=0,
                        help="blocks that fill the tier to its eviction threshold; default with "
                             "--cold-tier-state full: --tier-gib x --eviction-threshold / block "
                             "bytes")
    parser.add_argument("--store-order", choices=("batch", "interleaved"), default="batch",
                        help="how the cold pass stores its keys: batch (--job-blocks per job "
                             "in key order, the default) or interleaved (--requests requests "
                             "store one block per call in turn, as concurrent vLLM requests "
                             "commit per step)")
    parser.add_argument("--load-unit", choices=("batch", "request"), default="batch",
                        help="cold load job: batch (--job-blocks keys per job in key order, "
                             "the default) or request (one request's whole prefix per job, "
                             "requests in random order, stopping at the first miss)")
    parser.add_argument("--requests", type=int, default=16,
                        help="requests --num-blocks is split into for --store-order "
                             "interleaved and --load-unit request (default 16)")
    parser.add_argument("--seed", type=int, default=0,
                        help="seed for the in-flight passes' job order")
    parser.add_argument("--preset", choices=sorted(COLD_PRESETS), default=None,
                        help="named cold-load shape (default mode); explicit flags override "
                             "its values")
    parser.add_argument("--gpu", type=int, default=0,
                        help="CUDA device index")
    parser.add_argument("--tp", type=int, default=1,
                        help="Tensor-parallel world size: one worker per GPU "
                             "(rank i on device --gpu+i) and one scheduler-side "
                             "manager, as vLLM's --tensor-parallel-size. "
                             "--block-bytes is each rank's shard size.")
    parser.add_argument("--gpus", type=str, default=None,
                        help="Comma-separated CUDA devices for the TP ranks "
                             "(e.g. 2,3) instead of --gpu..--gpu+tp-1; implies "
                             "--tp = the device count.")
    parser.add_argument("--min-duration", type=float, default=5.0,
                        help="Minimum seconds per benchmark phase")
    parser.add_argument("--warmup", type=int, default=4,
                        help="Warmup iterations before measurement")
    parser.add_argument("--workers", type=int, default=4,
                        help="ThreadPoolExecutor worker count")
    parser.add_argument("--working-set", type=int, default=512,
                        help="Working set size for eviction-pressure mode (blocks)")
    parser.add_argument("--csv", type=str, default=None,
                        help="Append results to CSV file")
    parser.add_argument("--output", action="append", default=[],
                        metavar="FILE",
                        help="Write results to FILE (YAML for .yaml/.yml, CSV "
                             "for .csv; repeatable)")
    parser.add_argument("--tag", type=str, default="",
                        help="Tag column for CSV (e.g. branch name)")
    parser.add_argument("--target-ssd", type=str, default="",
                        help="Comma-separated PCI BDFs of NVMe drives under test "
                             "(recorded in params, not used by the bench itself)")
    parser.add_argument("--no-cold", action="store_true",
                        help="Skip the cold-load benchmark (SSD→DRAM→GPU)")
    parser.add_argument("--no-warm", action="store_true",
                        help="(default) Skip the warm-load pass. Only then may --num-blocks exceed "
                             "what the tier holds (a store-only run over more than the tier: the "
                             "stores then wait for the write-through)")
    parser.add_argument("--no-eviction", action="store_true",
                        help="Skip the eviction-pressure benchmark")
    parser.add_argument("--mode", type=str, default=None,
                        choices=["default", *ALL_MODES[1:], "all"],
                        help="Benchmark mode (omit for default or --pattern; "
                             "'all' runs every mode sequentially)")
    parser.add_argument("--hit-ratio", type=float, default=0.75,
                        help="(prefix-miss) Fraction of keys pre-stored (0.0-1.0)")
    parser.add_argument("--requests-per-step", type=int, default=8,
                        help="(scheduler-step) Requests per scheduler step")
    parser.add_argument("--num-sessions", type=int, default=32,
                        help="(scheduler-step) Distinct conversations in the pool")
    parser.add_argument("--lookup-order", choices=LOOKUP_ORDERS, default=LOOKUP_FIRST,
                        help="Order of lookup() and touch() for the same keys, in every "
                             "mode that does both: lookup-first (default) is vLLM >= 0.21 "
                             "(vLLM PR #41228, still so in 0.30); touch-first is vLLM <= 0.20")
    # ── in-flight passes (default mode's store, warm and cold; contention) ──
    parser.add_argument("--job-blocks", type=int, default=64,
                        help="(default, contention) blocks per load or store job (default 64, "
                             "128 MiB at 2 MiB blocks). Throughput at 2+ jobs in flight is the "
                             "same from 16 to 128 blocks (warm 22.7 GB/s, 2026-11-05 sweeps); 64 "
                             "keeps the per-job host cost small. vLLM's job is one request's "
                             "prefix hit for a load (625 blocks in LongDocQA), one prefill chunk "
                             "for a store (about 105). --load-unit request sizes cold jobs by "
                             "request instead")
    parser.add_argument("--inflight", type=int, default=4,
                        help="(default, contention, scheduler-step) jobs kept outstanding by the "
                             "store, warm and cold passes, contention's loads and scheduler-step's "
                             "loads (one job per request) (default 4): store and cold "
                             "loads reach their plateau only at 4 (store 14.6 vs 13.1 GB/s at 2), "
                             "and LongDocQA's client runs 4 (about 2 measured); needs server "
                             "channels > the jobs in flight")
    parser.add_argument("--store-job-blocks", type=int, default=None,
                        help="(default, contention) blocks per store job: the default mode's "
                             "store pass (default --job-blocks) and contention's stores "
                             "(default 32)")
    parser.add_argument("--store-inflight", type=int, default=None,
                        help="(default, contention) store jobs outstanding: the default mode's "
                             "store pass (default --inflight) and contention's stores (default "
                             "1: loads are the larger stream in serving)")
    parser.add_argument("--step-us", type=float, default=100.0,
                        help="poll interval for finished transfers, every mode: N (default 100) "
                             "sleeps N us between polls, and is the in-flight passes' scheduler "
                             "tick; 0 polls without sleeping, a tight loop that yields the CPU "
                             "each tick (it stalls some jobs on the GIL: p99 up to 27 ms vs "
                             "6.4 ms at 100, warm 64x1, 2026-11-05)")
    parser.add_argument("--verify", action="store_true",
                        help="(default, contention) stamp every stored block and compare every "
                             "loaded block byte for byte (adds GPU work: timings are not "
                             "comparable)")
    parser.add_argument("--shape", choices=sorted(INFLIGHT_SHAPES), default=None,
                        help="(default, contention) named validation shape (sets block bytes, "
                             "pieces, job sizes and in flight; explicit flags override). For "
                             "checking the benchmark against a workload, not for a target's metric")
    parser.add_argument("--prefix-sharing", choices=("shared", "private"), default="shared",
                        help="(scheduler-step) shared: sessions take slices of one shared "
                             "prefix pool (default); private: each session has its own "
                             "prefix blocks, none shared between sessions")
    return parser


def _fail(parser, mode_all: bool, mode: str, msg: str):
    parser.error(f"--mode all, {mode}: {msg}" if mode_all else msg)


def resolve_args(parser, argv=None, mode: str | None = None) -> argparse.Namespace:
    """Parse ``argv`` and resolve it for one mode (``mode``; default: --mode, or
    "pattern"/"default" without it): --preset and --shape fill the flags not given,
    then the in-flight defaults, then every check that can fail before the server is
    touched (parser.error, exit 2). An explicit flag always wins over a preset, a
    shape and a mode default."""
    args = parser.parse_args(argv)
    mode_all = args.mode == "all"
    if mode is None:
        mode = args.mode or ("pattern" if args.pattern else "default")
    args.run_mode = mode
    explicit = _explicit_flags(parser, argv)
    if args.preset:
        # Applied in every mode, as before: its block shape and working set apply to all.
        for key, val in COLD_PRESETS[args.preset].items():
            if key != "observation" and key not in explicit:
                setattr(args, key, val)
    if args.shape:
        if mode not in INFLIGHT_MODES:
            if not mode_all:
                parser.error(f"--shape applies to --mode {'/'.join(INFLIGHT_MODES)}")
        else:
            for key, val in INFLIGHT_SHAPES[args.shape].items():
                if key not in ("observation", "assumed") and key not in explicit:
                    setattr(args, key, val)
    if args.pieces_per_block is None:
        args.pieces_per_block = (INFLIGHT_DEFAULTS["pieces_per_block"]
                                 if mode in INFLIGHT_MODES else 1)
    if args.step_us < 0:
        parser.error("--step-us must be >= 0")
    if mode == "scheduler-step" and (args.inflight < 1 or args.requests_per_step < 1):
        _fail(parser, mode_all, mode, "--inflight and --requests-per-step must be >= 1")
    if args.pieces_per_block < 1:
        parser.error(f"--pieces-per-block must be >= 1, got {args.pieces_per_block}")
    if args.block_bytes % args.pieces_per_block:
        parser.error(f"--block-bytes {args.block_bytes} is not divisible by "
                     f"--pieces-per-block {args.pieces_per_block}")
    if not (0.0 < args.eviction_threshold <= 1.0) or args.tier_gib <= 0:
        parser.error("--tier-gib must be > 0 and --eviction-threshold in (0, 1]")
    tier_usable_blocks = int(args.tier_gib * 2**30 * args.eviction_threshold // args.block_bytes)
    if args.cold_tier_state == "full" and args.tier_fill_blocks <= 0:
        args.tier_fill_blocks = tier_usable_blocks   # tier size x threshold / block bytes

    if mode in INFLIGHT_MODES:
        # A working set that fits the tier, unless given.
        if "num_blocks" not in explicit and not (args.preset and "num_blocks" in COLD_PRESETS[args.preset]):
            args.num_blocks = max(1, min(INFLIGHT_MAX_BLOCKS,
                                         int(tier_usable_blocks * INFLIGHT_TIER_SHARE)))
        if mode == "contention":
            for key, val in CONTENTION_STORE_DEFAULTS.items():
                if getattr(args, key) is None:
                    setattr(args, key, val)
        if args.store_job_blocks is None:
            args.store_job_blocks = args.job_blocks
        if args.store_inflight is None:
            args.store_inflight = args.inflight
        for name in ("job_blocks", "inflight", "store_job_blocks", "store_inflight"):
            if getattr(args, name) < 1:
                _fail(parser, mode_all, mode, f"--{name.replace('_', '-')} must be >= 1")
        if args.verify and args.block_bytes // args.pieces_per_block < 64:
            _fail(parser, mode_all, mode, "--verify needs pieces of at least 64 bytes "
                  "(--block-bytes / --pieces-per-block)")
        if args.verify and (args.tp != 1 or (args.gpus and len(args.gpus.split(",")) > 1)):
            # The verifier reads rank 0's GPU tensors only.
            _fail(parser, mode_all, mode, "--verify supports TP=1 only")
        # The working set (--num-blocks) is cut into jobs of --job-blocks; --inflight of them
        # must exist at once (the warm pass and contention's loads).
        jobs = -(-args.num_blocks // args.job_blocks)
        loads = mode == "contention" or not (args.no_warm and args.no_cold)
        if loads and jobs < args.inflight:
            _fail(parser, mode_all, mode,
                  f"--num-blocks {args.num_blocks} makes only {jobs} job(s) of --job-blocks "
                  f"{args.job_blocks}, but --inflight {args.inflight} needs at least "
                  f"{args.inflight}: use --num-blocks >= {args.inflight * args.job_blocks}, "
                  f"or a smaller --job-blocks or --inflight")
        if args.num_blocks > tier_usable_blocks and (mode == "contention" or not args.no_warm):
            # A warm load is only warm while its keys stay in the memory tier. A larger set is
            # evicted to the drives and promoted back, and nothing in the pass's own output
            # shows it (4,000 blocks in a 4 GiB tier: 24.45 GB/s, 0 blocks not loaded, 2026-11-05).
            _fail(parser, mode_all, mode,
                  f"--num-blocks {args.num_blocks} is {args.num_blocks * args.block_bytes / 2**30:.1f} GiB, "
                  f"more than the {args.tier_gib * args.eviction_threshold:.1f} GiB the tier holds before "
                  f"evicting (--tier-gib {args.tier_gib:g} x --eviction-threshold {args.eviction_threshold:g}): "
                  f"loads would come from the drives. Use --num-blocks <= {tier_usable_blocks}, or set "
                  f"--tier-gib to the server's --memory-tier-size")
        if mode == "default" and not args.no_cold:
            if (args.store_order == "interleaved" or args.load_unit == "request") and (
                    args.requests < 1 or args.num_blocks % args.requests):
                _fail(parser, mode_all, mode,
                      f"--num-blocks {args.num_blocks} must split evenly into "
                      f"--requests {args.requests}")
    return args


def gpu_blocks(args) -> int:
    """GPU blocks the run's regions hold (store and load share them for the in-flight
    modes). The in-flight passes address one slot range per job in flight (loads
    first, then stores) and the populate jobs' ids; mixed eviction and the tier fill
    address 0..--bs-1 and 0..--job-blocks-1. Not --num-blocks, which is the tier
    working set. The other modes size by --num-blocks (and scheduler-step by a step's
    keys)."""
    mode = args.run_mode
    if mode == "default":
        blocks = max(args.bs, args.inflight * args.job_blocks,
                     args.store_inflight * args.store_job_blocks)
        if not args.no_cold:
            blocks = max(blocks, args.inflight * cold_unit_blocks(args))
        return blocks
    if mode == "contention":
        return max(args.bs, args.job_blocks, args.store_job_blocks,
                   args.inflight * args.job_blocks + args.store_inflight * args.store_job_blocks)
    blocks = max(args.num_blocks, args.bs)
    if mode == "scheduler-step":
        # One slot per load job and per store job (sched_step_slots).
        slot, loads, stores = sched_step_slots(args.num_blocks, args.requests_per_step,
                                               args.inflight)
        blocks = max(blocks, slot * (loads + stores))
    return blocks


def run_params(args, gpus) -> dict:
    """The params recorded with a mode's results (CSV columns, YAML params)."""
    mode = args.run_mode
    extra = {
        "tag": args.tag,
        "mode": mode,
        "lookup_order": args.lookup_order,
        "shm_path": args.shm_path,
        "gpu": args.gpu,
        "gpus": ",".join(map(str, gpus)),
        "tp": args.tp,
        "bs": args.bs,
        "num_blocks": args.num_blocks,
        "block_bytes": args.block_bytes,
        "workers": args.workers,
        "working_set": args.working_set,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
    }
    if args.target_ssd:
        extra["target_ssd"] = [b.strip() for b in args.target_ssd.split(",")]
    if args.pieces_per_block > 1:
        extra["pieces_per_block"] = args.pieces_per_block
    if mode in INFLIGHT_MODES:
        extra.update({"shape": args.shape or "", "job_blocks": args.job_blocks,
                      "inflight": args.inflight, "step_us": args.step_us, "seed": args.seed,
                      "verify": args.verify})
        extra.update({"store_job_blocks": args.store_job_blocks,
                      "store_inflight": args.store_inflight})
    if mode == "default" and (not cold_shape_is_default(args) or args.preset):
        extra.update({
            "preset": args.preset or "",
            "cold_tier_state": args.cold_tier_state,
            "tier_fill_blocks": args.tier_fill_blocks,
            "store_order": args.store_order,
            "load_unit": args.load_unit,
            "requests": args.requests,
        })
    if mode == "prefix-miss":
        extra["hit_ratio"] = args.hit_ratio
    elif mode == "scheduler-step":
        extra["inflight"] = args.inflight
        extra["step_us"] = args.step_us
        extra["requests_per_step"] = args.requests_per_step
        extra["num_sessions"] = args.num_sessions
        extra["prefix_sharing"] = args.prefix_sharing
    is_private = (mode == "scheduler-step" and args.prefix_sharing == "private")
    extra["prefix_shared_pools"] = 0 if is_private else 1
    if mode == "pattern":
        extra["pattern"] = args.pattern
    return extra


def main():
    parser = build_parser()
    first = parser.parse_args()
    top_mode = first.mode or ("pattern" if first.pattern else "default")
    modes = ALL_MODES if top_mode == "all" else [top_mode]
    # Resolve and check every mode's arguments before anything is connected.
    resolved = {m: resolve_args(parser, None, m) for m in modes}
    args = resolved[modes[0]]

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
    real_tp = len(gpus) > 1
    if real_tp and args.pattern:
        parser.error("--pattern does not support tensor parallelism")
    tp = len(gpus)
    for a in resolved.values():
        a.tp = tp
        a.gpu = gpus[0]
    # Throughput accounting counts every rank's shard (each moves block_bytes).
    acct_block_bytes = args.block_bytes * tp if real_tp else args.block_bytes

    torch.cuda.set_device(args.gpu)
    set_poll_us(args.step_us)   # every mode's poll for finished transfers

    print(f"Connector Lifecycle Benchmark")
    print(f"  shm_path={args.shm_path}  gpus={gpus}  tp={tp}")
    print(f"  num_blocks={args.num_blocks}  bs={args.bs}  block_bytes={args.block_bytes}"
          + (f"  pieces_per_block={args.pieces_per_block} "
             f"({args.block_bytes // args.pieces_per_block} B per piece)"
             if args.pieces_per_block > 1 else "")
          + (f" per rank ({acct_block_bytes} per logical block)" if real_tp else ""))
    print(f"  min_duration={args.min_duration}s  workers={args.workers}  "
          f"lookup_order={args.lookup_order}")

    # ── set up ring + connector objects ──
    # Under real TP each process claims from its own channel slot (rank 0 here).
    ring = Ring(args.shm_path, ready_timeout=10.0, log=lambda msg: None,
                claim_slot=0, claim_slots=tp if real_tp else 1)

    # Clear leaked channel owner words from crashed/exited benchmark processes.
    # The server never touches owner words, so a non-zero owner from a dead
    # process permanently blocks that channel.  Safe because we are the only
    # live client right now (the benchmark is single-process).
    from certus_shmq_connector.ring import OFF_OWNER
    cleared = 0
    for ch in range(ring._num_channels):
        owner_off = ring._channel_base(ch) + OFF_OWNER
        if ring._rd_u32(owner_off) != 0:
            ring._wr_u32(owner_off, 0)
            cleared += 1
    if cleared:
        print(f"  Cleared {cleared} leaked channel claims from prior runs")
    # Re-claim channel 0 for the main thread (the constructor already did this,
    # but we just zeroed the owner word above).
    ring._tls.channel = None  # force re-claim on next use

    if real_tp:
        # Main thread + the pool must fit this process's slot; each other rank needs one
        # pool per worker group.
        per_slot = ring._num_channels // tp
        need = 1 + args.workers
        if per_slot < need:
            sys.exit(f"error: --tp {tp} with --workers {args.workers} needs "
                     f"{need} shmq channels per rank but the server has "
                     f"{ring._num_channels} ({per_slot} per rank); start "
                     f"certus-server with --channels {need * tp} or more")

    for m, a in resolved.items():
        if m in INFLIGHT_MODES or m == "scheduler-step":
            # Jobs beyond the pool's threads queue in the executor (as in vLLM, whose
            # pool has 4 threads); each running job holds a channel, the main thread one.
            if m == "scheduler-step":
                jobs = sum(sched_step_slots(a.num_blocks, a.requests_per_step, a.inflight)[1:])
            elif m == "contention":
                jobs = a.inflight + a.store_inflight
            else:
                jobs = max(a.inflight, a.store_inflight)
            need = min(a.workers, jobs) + 1
            if ring._num_channels < need:
                sys.exit(f"error: --mode {m} needs {need} shmq channels, the server "
                         f"has {ring._num_channels}; start certus-server with --channels {need}")

    manager = ShmqCertusOffloadingManager(
        ring, block_size_bytes=args.block_bytes, world_size=tp,
    )
    executor = ThreadPoolExecutor(max_workers=args.workers, thread_name_prefix="bench-shmq")

    def make_worker(a):
        """GPU regions and the one worker (as in vLLM) for one mode's resolved args.
        Every transfer addresses GPU blocks from 0, so the regions are sized by the jobs
        (or batches) in flight (gpu_blocks), not by --working-set: sizing them from
        --working-set allocated 64 GiB for a 16 GiB working set at 2 MiB blocks. Each
        mode of --mode all gets its own, at its own --pieces-per-block."""
        blocks = gpu_blocks(a)
        regions, tensors = make_kv_regions(a.gpu, a.block_bytes, blocks, a.pieces_per_block)
        if real_tp:
            w = _TPWorkerGroup(
                ring, regions, a.block_bytes, executor, gpus=gpus,
                shm_path=a.shm_path, max_blocks=blocks,
                workers=a.workers, layers=a.pieces_per_block,
            )
        else:
            w = worker_class()(ring, regions, a.block_bytes, executor, rank=0, world_size=tp)
        return regions, tensors, blocks, w

    regions, tensors, max_blocks, worker = make_worker(args)
    retired: list = []   # --mode all: earlier modes' regions and workers

    # ── warmup ──
    print(f"\nWarmup ({args.warmup} store/remove cycles)...")
    warmup_keys = make_content_keys(args.warmup, seed=99999)
    for wk in warmup_keys:
        try:
            _populate_keys(manager, worker, [wk], 1)
        except Exception:
            pass
    _remove_keys(ring, warmup_keys, tp)

    # ── mode dispatch ──

    def _run_mode(a) -> list[BenchResult]:
        """Run a single benchmark mode with its resolved arguments, return its results."""
        nonlocal regions, tensors, max_blocks, worker
        mode = a.run_mode
        if a is not args:
            # The previous mode's regions stay allocated: the server may still hold
            # their CUDA IPC mappings.
            retired.append((regions, tensors, worker))
            regions, tensors, max_blocks, worker = make_worker(a)
        results: list[BenchResult] = []
        keys = make_content_keys(a.num_blocks)
        verifier = (_ByteVerifier(tensors).bind(max_blocks)
                    if mode in INFLIGHT_MODES and a.verify else None)

        if mode == "pattern":
            overrides = {}
            for ov in a.override:
                if "=" in ov:
                    k, v = ov.split("=", 1)
                    overrides[k] = v
            pattern, eval_expr_fn = _load_pattern(a.pattern, overrides)
            print(f"\nPattern: {pattern.id} ({pattern.name})")
            pattern.describe()
            return bench_pattern(
                pattern, eval_expr_fn, manager, worker, regions,
                ring, a.bs,
                a.block_bytes if a.block_bytes != DEFAULT_BLOCK_BYTES else None,
                a.min_duration, tp,
                lookup_order=a.lookup_order,
            )

        if mode == "prefix-miss":
            print(f"\n  Mode: prefix-miss (hit_ratio={a.hit_ratio})")
            r = bench_prefix_miss(
                manager, worker, regions,
                a.bs, acct_block_bytes, a.min_duration,
                hit_ratio=a.hit_ratio,
                num_blocks=a.num_blocks,
                lookup_order=a.lookup_order,
            )
            results.append(r)

        elif mode == "scheduler-step":
            print(f"\n  Mode: scheduler-step (reqs={a.requests_per_step}, "
                  f"sessions={a.num_sessions})")
            r = bench_scheduler_step(
                manager, worker, ring, a.bs, acct_block_bytes, a.min_duration,
                requests_per_step=a.requests_per_step,
                inflight=a.inflight,
                step_us=a.step_us,
                num_sessions=a.num_sessions,
                num_blocks=a.num_blocks,
                lookup_order=a.lookup_order,
                prefix_sharing=a.prefix_sharing,
            )
            results.append(r)

        else:  # default, contention: the in-flight passes
            set_gib = a.num_blocks * a.block_bytes / 2**30
            print(f"\n  Tier assumed: {a.tier_gib:g} GiB x {a.eviction_threshold:g} = "
                  f"{a.tier_gib * a.eviction_threshold:.1f} GiB usable; working set "
                  f"{a.num_blocks} blocks = {set_gib:.1f} GiB"
                  + ("" if a.num_blocks <= int(a.tier_gib * 2**30 * a.eviction_threshold // a.block_bytes)
                     else "  (larger than the tier: the stores wait for the write-through)"))
            # Each pass leaves its working set on the server; remove it even when the
            # pass fails, so a next process on the same server (a grid's next cell, same
            # key seed) does not find it resident and skip its stores.
            try:
                if mode == "contention":
                    print(f"\n  Mode: contention ({inflight_label(a, 'contention')})")
                    _remove_keys(ring, keys, tp)
                    results.extend(bench_contention(
                        manager, worker, ring, keys, acct_block_bytes, a.min_duration,
                        args=a, world_size=tp, lookup_order=a.lookup_order,
                        verifier=verifier))
                else:
                    print(f"\n  Mode: default ({inflight_label(a, 'store')})")
                    _remove_keys(ring, keys, tp)
                    results.append(bench_store_inflight(
                        manager, worker, ring, keys, acct_block_bytes, a.min_duration,
                        args=a, world_size=tp, verifier=verifier))
                    _remove_keys(ring, keys, tp)
                    if not a.no_warm:
                        results.append(bench_warm_inflight(
                            manager, worker, keys, acct_block_bytes, a.min_duration,
                            args=a, lookup_order=a.lookup_order, verifier=verifier))
                    if not a.no_cold and cold_shape_is_default(a) and a.inflight == 1:
                        # The plain cold pass, on the keys the warm pass stored in
                        # --job-blocks jobs (as the committed pass reused the warm set);
                        # with --no-warm, stored here the same way.
                        if a.no_warm:
                            _populate_jobs(manager, worker, keys, a.job_blocks)
                        r = bench_load_lifecycle(
                            manager, worker, regions, keys,
                            a.job_blocks, acct_block_bytes, a.min_duration,
                            cold=True, lookup_order=a.lookup_order,
                        )
                        r.print_report()
                        results.append(r)
                    elif not a.no_cold:
                        # Shaped cold pass: re-store in --store-order, then load under the
                        # tier state, job unit and --inflight.
                        _remove_keys(ring, keys, tp)
                        populate_shaped(manager, worker, keys, a.job_blocks,
                                        a.store_order, a.requests)
                        r = bench_cold_load_shaped(
                            manager, worker, ring, keys,
                            a.job_blocks, acct_block_bytes, a.min_duration,
                            args=a, world_size=tp, lookup_order=a.lookup_order,
                        )
                        r.print_report()
                        results.append(r)
                    if verifier is not None and not a.no_cold:
                        print("  Verify: the cold pass is not verified (the committed cold "
                              "passes have no --verify)")
            finally:
                _remove_keys(ring, keys, tp)
            if mode == "default" and not a.no_eviction:
                ring.clear_memory_tier()
                results.append(bench_mixed_eviction(
                    manager, worker, regions,
                    a.bs, acct_block_bytes, a.min_duration,
                    working_set=a.working_set,
                    load_fraction=0.5,
                    lookup_order=a.lookup_order,
                ))

        # Clean up between modes.
        try:
            _remove_keys(ring, keys, tp)
        except Exception:
            pass

        return results

    all_results: list[BenchResult] = []
    rows: list[dict] = []
    yaml_params: dict = {}
    for m in modes:
        if top_mode == "all":
            print(f"\n{'='*70}")
            print(f"  Running mode: {m}")
            print(f"{'='*70}")
        res = _run_mode(resolved[m])
        extra = run_params(resolved[m], gpus)
        if top_mode == "all":
            extra["mode"] = "all"
        all_results.extend(res)
        rows.extend(r.csv_row(extra) for r in res)
        yaml_params = yaml_params or extra

    # ── CSV output ──
    if args.csv and rows:
        write_header = not os.path.exists(args.csv) or os.path.getsize(args.csv) == 0
        fieldnames = list(rows[0].keys())
        for r in rows[1:]:
            for k in r:
                if k not in fieldnames:
                    fieldnames.append(k)
        if not write_header:
            with open(args.csv, newline="") as f:
                existing = next(csv.reader(f), [])
            missing = [k for k in fieldnames if k not in existing]
            if missing:
                # Shaped runs add columns (pieces_per_block, cold_*...); appending
                # them under an older header would silently drop them.
                sys.exit(f"error: {args.csv} has no columns {missing}; write these "
                         f"results to a new --csv file")
            fieldnames = existing
        with open(args.csv, "a", newline="") as f:
            w = csv.DictWriter(f, fieldnames=fieldnames, extrasaction="ignore")
            if write_header:
                w.writeheader()
            w.writerows(rows)
        print(f"\nResults appended to {args.csv}")

    # ── structured output (--output) ──
    if args.output and all_results:
        params = {k: v for k, v in yaml_params.items() if k not in ("gpu", "gpus")}
        out_data = _results_to_structured(all_results, params)
        for output_path in args.output:
            write_output(output_path, out_data)
            print(f"Results written to {output_path}")

    # ── cleanup ──
    if real_tp:
        for _r, _t, w in retired:
            w.close()
        worker.close()
    executor.shutdown(wait=False)
    ring.close()

    print(f"\nDone.")


if __name__ == "__main__":
    main()
