#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""End-to-end 3FS connector benchmark: vLLM's HF3FSKVConnector storage engine.

The 3FS counterpart of benchmarks/certus-connector-bench/bench_connector_lifecycle.py,
benchmarks/cputier-connector-bench/bench_cputier_lifecycle.py and
benchmarks/lmcache-connector-bench/bench_lmcache_lifecycle.py. It drives the same
objects vLLM's ``HF3FSKVConnector`` builds when vLLM is started with
``kv_connector=HF3FSKVConnector`` (see benchmarks/vllm-serve/run-serve-h3fs0-cc131k.sh):

  - worker side: ``register_kv_caches`` setup (``_setup_kv_cache_config``,
    ``_setup_storage_clients``: one Hf3fsClient per I/O job over one data file)
    and the ``AsyncOperationManager`` (``submit_save_operation``,
    ``submit_load_operation``, ``get_finished_operations``), which gathers KV
    out of the GPU cache, writes it with the 3FS USRBIO client, and reads and
    scatters it back on a load;
  - the connector's metadata server (``hf3fs_metadata_server``), which hands out
    file pages and answers ``batch_key_exists`` — the lookup vLLM's scheduler
    side makes in ``get_num_new_matched_tokens``.

No vLLM engine, model, or HTTP server is started.

NO MEMORY TIER. HF3FSKVConnector has no host-RAM cache: every store goes
GPU → 3FS and every load comes 3FS → GPU. This bench is therefore a DISK-ONLY
baseline. Its warm and cold load rows are the same disk path; --dram does not
apply, and there is no mixed-eviction phase (there is nothing to evict). The
report says so.

CLIENT. Run it in the vLLM + 3FS image (benchmarks/vllm-serve/build_h3fs_container.sh)
against a live 3FS FUSE mount, and the connector uses the real USRBIO client.
Without the 3FS client (the stock vllm-openai image) vLLM silently falls back
to its file-backed MOCK client, which uses buffered I/O, so loads can be served
from the page cache: such a run is NOT a 3FS measurement. Every result row
records ``hf3fs_client`` (``usrbio`` / ``mock``), the header says it, and
--require-usrbio refuses to run on the mock.

UNITS. One key is one vLLM GPU block: --block-bytes bytes covering all
--layers layer tensors (``bytes_per_page`` in the connector, one 3FS file page),
the same per-key size as the sibling benches. Keys are independent (the
connector's prefix hash only names them), so labels, report and CSV columns
match the sibling benches and join on ``label``.

    benchmarks/3fs-connector-bench/run-in-container.sh            # default phases
    run-in-container.sh --verify                                  # data round trip first
    run-in-container.sh --mode all --csv bench-results/3fs.csv --tag 3fs
"""

from __future__ import annotations

import argparse
import csv
import os
import statistics
import subprocess
import sys
import time
import urllib.request
import uuid
from dataclasses import dataclass, field

import torch

assert torch.cuda.is_available(), "CUDA GPU required"

try:
    import vllm
    import vllm.distributed.kv_transfer.kv_connector.v1.hf3fs.hf3fs_connector as hc
    from vllm.distributed.kv_transfer.kv_connector.v1.hf3fs.hf3fs_metadata_server import (
        Hf3fsGlobalMetadataClient,
    )
except ImportError as _e:
    sys.exit(
        f"error: this benchmark needs vLLM >= 0.30 with HF3FSKVConnector ({_e}).\n"
        f"       Run it in the vLLM + 3FS image: "
        f"benchmarks/3fs-connector-bench/run-in-container.sh"
    )

# Same per-key size as the sibling benches' default block
# (Llama-3-8B: 16 tokens x 32 layers x 4096 bytes).
DEFAULT_BLOCK_BYTES = 2 * 1024 * 1024
DEFAULT_LAYERS = 32
POLL_S = 0.0001
CLIENT = "usrbio" if hc.HF3FS_AVAILABLE else "mock"


def parse_size(s: str) -> int:
    """Parse '4G', '512M', '1048576' into bytes (binary units)."""
    s = str(s).strip()
    mult = {"K": 1 << 10, "M": 1 << 20, "G": 1 << 30, "T": 1 << 40}
    if s and s[-1].upper() in mult:
        return int(float(s[:-1]) * mult[s[-1].upper()])
    return int(s)


# ── phase timing (same shape as the sibling benches) ────────────────────────

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
    # (GB/s, seconds) including time excluded from wall_seconds; None when
    # nothing was excluded. 3FS stores write straight to disk, so no store
    # drain is excluded and this stays None.
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


# ── metadata server ─────────────────────────────────────────────────────────

class MetadataServer:
    """The connector's page/key index (hf3fs_metadata_server), in a child process.

    vLLM does not start it either: run-serve-h3fs0-cc131k.sh launches it next to
    ``vllm serve``. Its state is in memory, so a fresh server starts empty.
    """

    def __init__(self, port: int):
        self.url = f"http://127.0.0.1:{port}"
        self.proc = subprocess.Popen(
            [sys.executable, "-m",
             "vllm.distributed.kv_transfer.kv_connector.v1.hf3fs.hf3fs_metadata_server",
             "--host", "127.0.0.1", "--port", str(port)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(300):
            if self.proc.poll() is not None:
                sys.exit(f"error: metadata server exited ({self.proc.returncode}); "
                         f"is port {port} in use?")
            try:
                urllib.request.urlopen(self.url + "/docs", timeout=1)
                return
            except Exception:
                time.sleep(0.1)
        sys.exit("error: metadata server did not come up in 30 s")

    def stop(self):
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


# ── the connector's worker side ─────────────────────────────────────────────

class H3fsStack:
    """HF3FSKVConnector's worker-side state, plus a fake GPU KV cache.

    The connector object is built without its vLLM-config constructor and given
    exactly the attributes its worker path reads, then its own
    ``_setup_kv_cache_config`` / ``_setup_storage_clients`` /
    ``AsyncOperationManager`` run unchanged. Only the metadata client is pointed
    at this bench's private metadata server (the stock client always uses
    ``http://localhost:18000``).
    """

    def __init__(self, args, num_gpu_blocks: int, md_url: str):
        torch.cuda.set_device(args.gpu)
        self.block_bytes = args.block_bytes
        nh, cd = kv_geometry(args.block_bytes, args.layers, args.tokens_per_block, args.kv_heads)
        self.num_heads, self.content_dim = nh, cd
        self.num_gpu_blocks = num_gpu_blocks

        # Patch only what the connector resolves from vLLM globals.
        hc.get_tensor_model_parallel_rank = lambda: 0
        _client_cls = Hf3fsGlobalMetadataClient
        hc.Hf3fsMetadataClient = lambda: _client_cls(base_url=md_url)

        c = object.__new__(hc.HF3FSKVConnector)
        # [B, H, N, C] per layer: the standardized view the connector requires.
        c._kv_caches = {
            f"model.layers.{i}.self_attn.attn": torch.randn(
                num_gpu_blocks, nh, args.tokens_per_block, cd,
                dtype=torch.float16, device=f"cuda:{args.gpu}")
            for i in range(args.layers)
        }
        torch.cuda.synchronize()
        c._storage_path = args.storage_path
        c._metadata_server_url = md_url
        c._file_size = args.file_size
        c._numjobs = args.numjobs
        c._max_device_buffer_count = max(args.max_device_buffer_count,
                                         args.numjobs * hc.DEFAULT_MAX_IO_ENTRIES)
        c._setup_kv_cache_config()
        assert c._bytes_per_page == args.block_bytes, (c._bytes_per_page, args.block_bytes)
        c._setup_storage_clients()
        c._async_manager = hc.AsyncOperationManager(c)
        self.c = c
        self.am = c._async_manager
        self.md = c._metadata_client
        self.num_pages = args.file_size // args.block_bytes
        self.data_file = os.path.join(args.storage_path, "hf3fs_vllm_data_file_0")
        self._cursor = 0
        self.failed = 0

    # A store/load is one vLLM request: its block ids in the GPU cache and the
    # prefix hashes that name them in 3FS.
    def alloc_blocks(self, n: int) -> list[int]:
        nb = self.num_gpu_blocks
        out = [(self._cursor + j) % nb for j in range(n)]
        self._cursor = (self._cursor + n) % nb
        return out

    @staticmethod
    def new_keys(n: int) -> list[str]:
        return [uuid.uuid4().hex for _ in range(n)]

    def lookup(self, keys: list[str]) -> int:
        """Matched prefix length, as get_num_new_matched_tokens counts it."""
        ex = self.md.batch_key_exists(keys)
        return next((i for i, e in enumerate(ex) if not e), len(ex))

    def submit_store(self, keys: list[str], blocks: list[int] | None = None):
        rid = f"s-{uuid.uuid4().hex[:12]}"
        blocks = blocks if blocks is not None else self.alloc_blocks(len(keys))
        futs = []
        # wait_for_save splits a request into max_device_buffer_count batches.
        step = self.c._max_device_buffer_count
        for i in range(0, len(keys), step):
            futs.append(self.am.submit_save_operation(rid, blocks[i:i + step], keys[i:i + step]))
        return rid, futs

    def submit_load(self, keys: list[str], blocks: list[int] | None = None):
        blocks = blocks if blocks is not None else self.alloc_blocks(len(keys))
        rid = f"l-{uuid.uuid4().hex[:12]}"
        return rid, [self.am.submit_load_operation(rid, blocks, keys)]

    def wait(self, futs) -> bool:
        """Poll as vLLM's get_finished does; True if every operation succeeded."""
        while not all(f.done() for f in futs):
            time.sleep(POLL_S)
        ok = all(f.result() for f in futs)
        if not ok:
            self.failed += 1
        return ok

    def reset(self):
        """Forget every key and free every page (POST /clear, then re-init rank 0)."""
        self.md._post("clear", {})
        self.md.initialize(0, self.num_pages, role="worker")

    def drop_page_cache(self) -> bool:
        """Ask the kernel to drop this file's cached pages (mock client only).

        The USRBIO client bypasses the page cache, so a 3FS read is always a
        disk read. The mock uses buffered I/O; without this its cold rows would
        be RAM reads. POSIX_FADV_DONTNEED drops clean pages, which is all a
        flushed file has.
        """
        if CLIENT != "mock" or not os.path.exists(self.data_file):
            return False
        fd = os.open(self.data_file, os.O_RDONLY)
        try:
            os.fsync(fd)
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)
        return True

    def shutdown(self):
        try:
            self.am.shutdown()
        finally:
            for cl in self.c._clients:
                try:
                    cl.close()
                except Exception:
                    pass


def kv_geometry(block_bytes: int, layers: int, tokens: int, kv_heads: int | None):
    """(num_heads, content_dim) of one fp16 [B, H, N, C] layer tensor so that one
    block (``tokens`` tokens x ``layers`` layers) is exactly ``block_bytes``."""
    per_layer = block_bytes // layers
    if per_layer * layers != block_bytes or per_layer % (2 * tokens):
        sys.exit(f"error: --block-bytes {block_bytes} must be a multiple of "
                 f"layers x tokens_per_block x 2 = {layers * tokens * 2}")
    elems = per_layer // (2 * tokens)          # fp16 elements per token per layer
    if kv_heads:
        if elems % kv_heads:
            sys.exit(f"error: --kv-heads {kv_heads} does not divide {elems} elements/token/layer")
        return kv_heads, elems // kv_heads
    for nh in (8, 4, 2, 1):                    # largest head count keeping head_dim >= 128
        if elems % nh == 0 and elems // nh >= 128:
            return nh, elems // nh
    return 1, elems


def disk_read_bytes(path: str) -> int | None:
    """Cumulative bytes read from the block device under ``path`` (/sys stat)."""
    try:
        dev = os.stat(path).st_dev
        base = f"/sys/dev/block/{os.major(dev)}:{os.minor(dev)}"
        if not os.path.exists(base + "/stat"):
            return None
        with open(base + "/stat") as f:
            return int(f.read().split()[2]) * 512
    except OSError:
        return None


# ── building blocks ─────────────────────────────────────────────────────────

def store_batch(stack: H3fsStack, keys: list[str], phases=None) -> int:
    """One request: submit the save, poll get_finished until it lands."""
    t0 = time.perf_counter()
    _, futs = stack.submit_store(keys)
    _rec(phases, "submit_store", t0)
    t0 = time.perf_counter()
    ok = stack.wait(futs)
    _rec(phases, "get_finished", t0)
    return len(keys) if ok else 0


def load_batch(stack: H3fsStack, keys: list[str], phases=None,
               blocks: list[int] | None = None) -> tuple[int, int]:
    """One request: lookup the prefix, load the hit part. Returns (loaded, missed)."""
    t0 = time.perf_counter()
    hit = stack.lookup(keys)
    _rec(phases, "lookup", t0)
    if not hit:
        return 0, len(keys)
    t0 = time.perf_counter()
    _, futs = stack.submit_load(keys[:hit], None if blocks is None else blocks[:hit])
    _rec(phases, "submit_load", t0)
    t0 = time.perf_counter()
    ok = stack.wait(futs)
    _rec(phases, "get_finished", t0)
    return (hit, len(keys) - hit) if ok else (0, len(keys))


def batches(keys: list[str], bs: int) -> list[list[str]]:
    return [keys[i:i + bs] for i in range(0, len(keys), bs)]


# ── default mode phases ─────────────────────────────────────────────────────

def bench_store_lifecycle(stack, num_blocks, batch_size, min_duration) -> BenchResult:
    """submit_store → get_finished, one request per batch.

    Every pass stores fresh keys (the metadata server would otherwise reuse the
    page). A write lands on disk before get_finished reports it, so there is no
    background write-through to drain; the per-pass reset (metadata clear) is
    excluded from wall time like the sibling benches' resets.
    """
    phases = _phases("submit_store", "get_finished", "reset")
    total = drops = 0
    excluded = 0.0
    wall_start = time.perf_counter()
    iteration = 0
    while (time.perf_counter() - wall_start - excluded) < min_duration or iteration < 2:
        for b in batches(stack.new_keys(num_blocks), batch_size):
            n = store_batch(stack, b, phases)
            total += n
            drops += len(b) - n
        iteration += 1
        t0 = time.perf_counter()
        stack.reset()
        _rec(phases, "reset", t0)
        excluded += time.perf_counter() - t0
    result = BenchResult(
        label="Store Lifecycle (touch → prepare_store → submit → finish → complete)",
        total_blocks=total, total_bytes=total * stack.block_bytes,
        wall_seconds=time.perf_counter() - wall_start - excluded, phases=phases)
    result.counts = {"store_drops": drops}
    result.print_report()
    if drops:
        print(f"  Store failures (page or buffer allocation, write): {drops}")
    return result


def bench_load_lifecycle(stack, key_batches, min_duration, *, cold: bool) -> BenchResult:
    """lookup → submit_load → get_finished.

    3FS has no memory tier, so warm and cold run the same disk path. Cold drops
    the data file's page cache before every pass (outside the timed window),
    which only matters for the buffered mock client; USRBIO never uses it.
    """
    phases = _phases("lookup", "submit_load", "get_finished")
    if cold:
        phases["demote"] = PhaseTiming("demote")
    total = misses = 0
    excluded = 0.0
    rd0 = disk_read_bytes(stack.data_file)
    wall_start = time.perf_counter()
    iteration = 0
    while (time.perf_counter() - wall_start - excluded) < min_duration or iteration < 2:
        if cold:
            t0 = time.perf_counter()
            stack.drop_page_cache()
            _rec(phases, "demote", t0)
            excluded += time.perf_counter() - t0
        for b in key_batches:
            loaded, m = load_batch(stack, b, phases)
            total += loaded
            misses += m
        iteration += 1
    wall = time.perf_counter() - wall_start - excluded
    rd1 = disk_read_bytes(stack.data_file)
    temp = "Cold" if cold else "Warm"
    result = BenchResult(
        label=f"{temp} Load Lifecycle (lookup → touch → prepare → submit → finish → complete)",
        total_blocks=total, total_bytes=total * stack.block_bytes,
        wall_seconds=wall, phases=phases)
    result.counts = {"load_misses": misses}
    if cold and rd0 is not None and rd1 is not None and CLIENT == "mock":
        # Block-device reads under the data file; USRBIO data moves over RDMA
        # inside 3FS, not through this host's view of the device, so it is only
        # meaningful (and only recorded) for the mock.
        result.counts["disk_read_bytes"] = rd1 - rd0
    result.print_report()
    if misses:
        print(f"  Load misses: {misses}")
    if cold and CLIENT == "mock":
        rd = (rd1 - rd0) if rd0 is not None and rd1 is not None else None
        print(f"  Cold pass device reads: "
              f"{'n/a' if rd is None else f'{rd / 1e6:.0f} MB'} for "
              f"{total * stack.block_bytes / 1e6:.0f} MB loaded (mock client, buffered I/O)")
    return result


# ── mode: pipelined ─────────────────────────────────────────────────────────

def bench_pipelined(stack, num_blocks, batch_size, min_duration, *,
                    pipeline_depth=4, direction="store") -> BenchResult:
    """Submit pipeline_depth requests before reaping; stores use fresh keys."""
    is_store = direction == "store"
    phases = _phases("prepare", "submit", "pipeline_drain", "complete")
    keys = stack.new_keys(num_blocks)
    if not is_store:
        for b in batches(keys, batch_size):
            store_batch(stack, b)
    total = 0
    excluded = 0.0
    wall_start = time.perf_counter()
    iteration = 0

    def drain(in_flight):
        nonlocal total
        t0 = time.perf_counter()
        for n, futs in in_flight:
            if stack.wait(futs):
                total += n
        _rec(phases, "pipeline_drain", t0)
        in_flight.clear()

    while (time.perf_counter() - wall_start - excluded) < min_duration or iteration < 2:
        pass_keys = stack.new_keys(num_blocks) if is_store else keys
        in_flight = []
        for b in batches(pass_keys, batch_size):
            t0 = time.perf_counter()
            hit = len(b) if is_store else stack.lookup(b)
            _rec(phases, "prepare", t0)
            if not hit:
                continue
            t0 = time.perf_counter()
            _, futs = stack.submit_store(b) if is_store else stack.submit_load(b[:hit])
            _rec(phases, "submit", t0)
            in_flight.append((hit, futs))
            if len(in_flight) >= pipeline_depth:
                drain(in_flight)
        if in_flight:
            drain(in_flight)
        iteration += 1
        if is_store:
            t0 = time.perf_counter()
            stack.reset()
            excluded += time.perf_counter() - t0
    result = BenchResult(
        label=f"Pipelined {'Store' if is_store else 'Load'} (depth={pipeline_depth})",
        total_blocks=total, total_bytes=total * stack.block_bytes,
        wall_seconds=time.perf_counter() - wall_start - excluded, phases=phases)
    result.print_report()
    return result


# ── mode: prefix-miss ───────────────────────────────────────────────────────

def bench_prefix_miss(stack, num_blocks, batch_size, min_duration, *, hit_ratio=0.75) -> BenchResult:
    """Maximal-prefix lookup cost at a given hit ratio.

    As in the sibling benches, the first hit_ratio x num_blocks keys are
    present, so each batch is fully present, absent, or (one batch) partial.
    One batch_key_exists call answers a request's whole prefix.
    """
    keys = stack.new_keys(num_blocks)
    present = max(0, min(num_blocks, int(num_blocks * hit_ratio)))
    for b in batches(keys[:present], batch_size):
        store_batch(stack, b)
    phases = _phases("lookup_hit", "lookup_miss", "prefix_scan", "load")
    scans = prefix_hits = total = 0
    wall_start = time.perf_counter()
    iteration = 0
    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for b in batches(keys, batch_size):
            t0 = time.perf_counter()
            hit = stack.lookup(b)
            _rec(phases, "prefix_scan", t0)
            _rec(phases, "lookup_hit" if hit else "lookup_miss", t0)
            scans += 1
            prefix_hits += hit
            if hit:
                t0 = time.perf_counter()
                _, futs = stack.submit_load(b[:hit])
                if stack.wait(futs):
                    total += hit
                _rec(phases, "load", t0)
        iteration += 1
    wall = time.perf_counter() - wall_start
    avg = prefix_hits / scans if scans else 0
    result = BenchResult(
        label=f"Prefix-Miss Scan (hit_ratio={hit_ratio:.0%}, avg_prefix={avg:.1f}/{batch_size})",
        total_blocks=total, total_bytes=total * stack.block_bytes,
        wall_seconds=wall, phases=phases)
    result.print_report()
    print(f"  Scans={scans}  avg_prefix_len={avg:.1f}  "
          f"lookup_hits={len(phases['lookup_hit'].samples)}  "
          f"lookup_misses={len(phases['lookup_miss'].samples)}")
    return result


# ── mode: scheduler-step ────────────────────────────────────────────────────

def bench_scheduler_step(stack, batch_size, min_duration, *, requests_per_step=8,
                         num_sessions=32, num_blocks=128) -> BenchResult:
    """Continuous-batching scheduler steps over shared session prefixes.

    Sessions share one prefix pool (stored up front) and diverge at geometric
    points; each request is session prefix + fresh suffix keys. Per step: look
    every request up (``touch_lookup``), load the hit prefixes (``submit``,
    ``drain_workers``), then save the suffixes; the saves are drained at the
    start of the next step, untimed, as vLLM defers freeing a request until its
    save finishes.
    """
    import random
    kpr = max(4, num_blocks // requests_per_step)
    prefix_len = kpr // 2
    suffix_len = kpr - prefix_len
    phases = _phases("touch_lookup", "prepare_load", "prepare_store", "submit",
                     "drain_workers", "complete")
    pool_n = min(prefix_len * 4, num_blocks * 2)
    pool = stack.new_keys(pool_n)
    for b in batches(pool, batch_size):
        store_batch(stack, b)
    rng = random.Random(42)
    sessions = [max(2, min(pool_n, int(rng.expovariate(1.0 / prefix_len))))
                for _ in range(num_sessions)]

    total = steps = prefix_hit_keys = suffix_store_keys = 0
    prev: list = []
    wall_start = time.perf_counter()
    while (time.perf_counter() - wall_start) < min_duration or steps < 3:
        for futs in prev:
            stack.wait(futs)
        reqs = []
        for _ in range(requests_per_step):
            p = sessions[rng.randrange(num_sessions)]
            reqs.append(pool[:p] + stack.new_keys(suffix_len))

        t0 = time.perf_counter()
        hits = [stack.lookup(r) for r in reqs]
        _rec(phases, "touch_lookup", t0)
        t0 = time.perf_counter()
        loads = [r[:h] for r, h in zip(reqs, hits) if h]
        _rec(phases, "prepare_load", t0)
        t0 = time.perf_counter()
        futs = [stack.submit_load(k)[1] for k in loads]
        _rec(phases, "submit", t0)
        t0 = time.perf_counter()
        for f in futs:
            stack.wait(f)
        _rec(phases, "drain_workers", t0)
        hk = sum(hits)
        prefix_hit_keys += hk
        total += hk

        t0 = time.perf_counter()
        stores = [r[h:] for r, h in zip(reqs, hits) if h < len(r)]
        _rec(phases, "prepare_store", t0)
        t0 = time.perf_counter()
        prev = [stack.submit_store(k)[1] for k in stores]
        _rec(phases, "complete", t0)
        n_store = sum(len(k) for k in stores)
        suffix_store_keys += n_store
        total += n_store
        steps += 1
    for futs in prev:
        stack.wait(futs)
    wall = time.perf_counter() - wall_start
    result = BenchResult(
        label=(f"Scheduler Step (reqs={requests_per_step}, "
               f"sessions={num_sessions}, keys/req={kpr})"),
        total_blocks=total, total_bytes=total * stack.block_bytes,
        wall_seconds=wall, phases=phases)
    result.print_report()
    print(f"  Steps={steps}  steps/s={steps / wall:.1f}  "
          f"prefix_hits={prefix_hit_keys}  suffix_stores={suffix_store_keys}")
    return result


# ── verify ──────────────────────────────────────────────────────────────────

def verify_roundtrip(stack: H3fsStack, n: int) -> bool:
    """Store random GPU blocks, zero them, load them back, compare bytes."""
    blocks = list(range(n))
    caches = list(stack.c._kv_caches.values())
    for t in caches:
        t[:n].normal_()
    expected = [t[:n].clone() for t in caches]
    torch.cuda.synchronize()
    keys = stack.new_keys(n)
    _, futs = stack.submit_store(keys, blocks)
    if not stack.wait(futs):
        print("  verify: save failed")
        return False
    for t in caches:
        t[:n].zero_()
    torch.cuda.synchronize()
    stack.drop_page_cache()
    loaded, missed = load_batch(stack, keys, blocks=blocks)
    torch.cuda.synchronize()
    if loaded != n:
        print(f"  verify: loaded {loaded}/{n} blocks (missed {missed})")
        return False
    bad = sum(0 if torch.equal(t[:n], e) else 1 for t, e in zip(caches, expected))
    if bad:
        print(f"  verify: {bad}/{len(expected)} layer tensors differ")
    return bad == 0


# ── main ────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="End-to-end 3FS (HF3FSKVConnector) connector lifecycle benchmark",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Modes (--mode):\n"
            "\n"
            "  default        store, warm load, cold load (no mixed eviction: no memory tier)\n"
            "  pipelined      Submit N requests before reaping\n"
            "  prefix-miss    Vary prefix hit ratio to measure the prefix lookup cost\n"
            "  scheduler-step Scheduler steps with multi-request prefix sharing\n"
            "  all            All of the above\n"
            "\n"
            "Drives HF3FSKVConnector's worker side (Hf3fsClient + AsyncOperationManager)\n"
            "and its metadata server, as run-serve-h3fs0-cc131k.sh's vLLM does."
        ),
    )
    parser.add_argument("--storage-path", default="/hf3fs/vllm",
                        help="directory for the connector's data file (a 3FS mount for "
                             "real results; run-in-container.sh binds it)")
    parser.add_argument("--file-size", type=parse_size, default=parse_size("64G"),
                        help="data file size = 3FS capacity in use (hf3fs_file_size)")
    parser.add_argument("--numjobs", type=int, default=16,
                        help="I/O jobs: Hf3fsClient instances (hf3fs_client_numjobs)")
    parser.add_argument("--max-device-buffer-count", type=int, default=128,
                        help="GPU staging buffers per direction (hf3fs_max_device_buffer_count)")
    parser.add_argument("--metadata-port", type=int, default=18010,
                        help="port for the bench's private metadata server")
    parser.add_argument("--require-usrbio", action="store_true",
                        help="exit instead of running on vLLM's file-backed mock client")
    parser.add_argument("--layers", type=int, default=DEFAULT_LAYERS,
                        help="per-layer KV tensors (one gather/scatter descriptor each)")
    parser.add_argument("--tokens-per-block", type=int, default=16)
    parser.add_argument("--kv-heads", type=int, default=None,
                        help="KV heads per layer tensor (default: derived from --block-bytes)")
    parser.add_argument("--verify", action="store_true",
                        help="store random GPU data, zero it, load it back and compare "
                             "bytes before benchmarking")
    parser.add_argument("--bs", type=int, default=16, help="keys (blocks) per request")
    parser.add_argument("--num-blocks", type=int, default=128, help="keys per pass")
    parser.add_argument("--block-bytes", type=parse_size, default=DEFAULT_BLOCK_BYTES,
                        help="bytes per key = one GPU block of all layers (one 3FS page)")
    parser.add_argument("--gpu", type=int, default=0)
    parser.add_argument("--tp", type=int, default=1, help="tensor parallel size (only 1)")
    parser.add_argument("--min-duration", type=float, default=5.0)
    parser.add_argument("--warmup", type=int, default=4)
    parser.add_argument("--working-set", type=int, default=512,
                        help="accepted for flag parity; 3FS has no memory tier to evict")
    parser.add_argument("--csv", type=str, default=None, help="append results to CSV")
    parser.add_argument("--tag", type=str, default="", help="CSV tag column")
    parser.add_argument("--no-cold", action="store_true")
    parser.add_argument("--no-eviction", action="store_true",
                        help="accepted for flag parity (there is no eviction phase)")
    parser.add_argument("--mode", default="default",
                        choices=["default", "pipelined", "prefix-miss", "scheduler-step", "all"])
    parser.add_argument("--pipeline-depth", type=int, default=4)
    parser.add_argument("--direction", default="both", choices=["store", "load", "both"])
    parser.add_argument("--hit-ratio", type=float, default=0.75)
    parser.add_argument("--requests-per-step", type=int, default=8)
    parser.add_argument("--num-sessions", type=int, default=32)
    parser.add_argument("--lookup-order", default="lookup-first",
                        choices=["lookup-first", "touch-first"],
                        help="accepted for flag parity; the connector has no touch call")
    args = parser.parse_args()
    if args.tp != 1:
        parser.error("--tp > 1 is not supported (one worker, one GPU)")
    if CLIENT == "mock":
        msg = ("vLLM's 3FS USRBIO client is not importable, so HF3FSKVConnector falls "
               "back to its file-backed MOCK client (buffered I/O): results are NOT 3FS.")
        if args.require_usrbio:
            sys.exit(f"error: {msg} Use the vLLM + 3FS image (build_h3fs_container.sh).")
        print(f"WARNING: {msg}")
    os.makedirs(args.storage_path, exist_ok=True)

    kpr = max(4, args.num_blocks // args.requests_per_step)
    num_gpu_blocks = max(args.bs * max(1, args.pipeline_depth),
                         2 * args.requests_per_step * kpr, args.bs) + args.bs

    print(f"3FS Connector Lifecycle Benchmark (vLLM {vllm.__version__}, torch {torch.__version__}, "
          f"client={CLIENT})")
    md = MetadataServer(args.metadata_port)
    try:
        stack = H3fsStack(args, num_gpu_blocks, md.url)
        print(f"  storage={args.storage_path}  file={args.file_size / 2**30:.0f} GiB "
              f"({stack.num_pages} pages)  numjobs={args.numjobs}  gpu={args.gpu}")
        print(f"  num_blocks={args.num_blocks}  bs={args.bs}  block_bytes={args.block_bytes}  "
              f"layers={args.layers}  kv=[{num_gpu_blocks}, {stack.num_heads}, "
              f"{args.tokens_per_block}, {stack.content_dim}] fp16  memory tier: none")
        try:
            results = _run(args, stack)
        finally:
            stack.shutdown()
    finally:
        md.stop()

    if stack.failed:
        print(f"  WARNING: {stack.failed} save/load operations failed (see connector log lines)")
    if args.csv and results:
        extra = {
            "tag": args.tag, "mode": args.mode, "backend": "3fs", "hf3fs_client": CLIENT,
            "storage_path": args.storage_path, "file_size": args.file_size,
            "numjobs": args.numjobs, "gpu": args.gpu, "tp": 1,
            "bs": args.bs, "num_blocks": args.num_blocks, "block_bytes": args.block_bytes,
            "layers": args.layers, "working_set": args.working_set,
            "lookup_order": args.lookup_order, "vllm_version": vllm.__version__,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        }
        if args.mode == "pipelined":
            extra.update(pipeline_depth=args.pipeline_depth, direction=args.direction)
        elif args.mode == "prefix-miss":
            extra["hit_ratio"] = args.hit_ratio
        elif args.mode == "scheduler-step":
            extra.update(requests_per_step=args.requests_per_step,
                         num_sessions=args.num_sessions)
        rows = [r.csv_row(extra) for r in results]
        fieldnames = list(rows[0].keys())
        for r in rows[1:]:
            fieldnames += [k for k in r if k not in fieldnames]
        write_header = not os.path.exists(args.csv)
        os.makedirs(os.path.dirname(os.path.abspath(args.csv)), exist_ok=True)
        with open(args.csv, "a", newline="") as f:
            w = csv.DictWriter(f, fieldnames=fieldnames, extrasaction="ignore")
            if write_header:
                w.writeheader()
            w.writerows(rows)
        print(f"\nResults appended to {args.csv}")
    print("\nDone.")


def _run(args, stack: H3fsStack) -> list[BenchResult]:
    if args.verify:
        print("\nVerify round-trip data integrity...")
        ok = verify_roundtrip(stack, max(1, min(args.bs, args.num_blocks)))
        print(f"  GPU→3FS→GPU: {'OK' if ok else 'FAILED'}")
        if not ok:
            sys.exit(1)

    print(f"\nWarmup ({args.warmup} store/load cycles)...")
    for _ in range(args.warmup):
        k = stack.new_keys(1)
        store_batch(stack, k)
        load_batch(stack, k)
    stack.reset()

    def run_mode(m: str) -> list[BenchResult]:
        out: list[BenchResult] = []
        if m == "pipelined":
            print(f"\n  Mode: pipelined (depth={args.pipeline_depth}, direction={args.direction})")
            for d in ("store", "load"):
                if args.direction in (d, "both"):
                    out.append(bench_pipelined(stack, args.num_blocks, args.bs, args.min_duration,
                                               pipeline_depth=args.pipeline_depth, direction=d))
                    stack.reset()
        elif m == "prefix-miss":
            print(f"\n  Mode: prefix-miss (hit_ratio={args.hit_ratio})")
            out.append(bench_prefix_miss(stack, args.num_blocks, args.bs, args.min_duration,
                                         hit_ratio=args.hit_ratio))
        elif m == "scheduler-step":
            print(f"\n  Mode: scheduler-step (reqs={args.requests_per_step}, "
                  f"sessions={args.num_sessions})")
            out.append(bench_scheduler_step(stack, args.bs, args.min_duration,
                                            requests_per_step=args.requests_per_step,
                                            num_sessions=args.num_sessions,
                                            num_blocks=args.num_blocks))
        else:
            out.append(bench_store_lifecycle(stack, args.num_blocks, args.bs, args.min_duration))
            keys = stack.new_keys(args.num_blocks)
            kb = batches(keys, args.bs)
            for b in kb:
                store_batch(stack, b)
            out.append(bench_load_lifecycle(stack, kb, args.min_duration, cold=False))
            if not args.no_cold:
                out.append(bench_load_lifecycle(stack, kb, args.min_duration, cold=True))
            print("\n  (mixed eviction skipped: HF3FSKVConnector has no memory tier to evict)")
        stack.reset()
        return out

    modes = (["default", "pipelined", "prefix-miss", "scheduler-step"]
             if args.mode == "all" else [args.mode])
    results: list[BenchResult] = []
    for m in modes:
        if args.mode == "all":
            print(f"\n{'='*70}\n  Running mode: {m}\n{'='*70}")
        results.extend(run_mode(m))
    return results


if __name__ == "__main__":
    main()
