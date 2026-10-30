#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""End-to-end LMCache connector benchmark: the LMCache MP adapter lifecycle.

The LMCache counterpart of benchmarks/certus-connector-bench/bench_connector_lifecycle.py
and benchmarks/cputier-connector-bench/bench_cputier_lifecycle.py. It drives the
same classes vLLM's ``LMCacheMPConnector`` builds when vLLM is started with
``kv_connector=LMCacheMPConnector`` (see deploy/lmcache/run.sh):

  - LMCacheMPSchedulerAdapter (scheduler side): maybe_submit_lookup_request,
    check_lookup_result, cleanup_lookup_result, free_lookup_locks, end_session
  - LMCacheMPWorkerAdapter (worker side): register_kv_caches,
    batched_submit_store_requests, batched_submit_retrieve_requests,
    get_finished, get_block_ids_with_load_errors

against a running ``lmcache server`` (MP mode: pinned-RAM L1 + optional L2 such
as fs_native on NVMe). KV moves GPU<->L1 by CUDA IPC: the server maps this
process's fake GPU KV cache, exactly as it maps vLLM's.

No vLLM engine, model, or HTTP server is started. The benchmark allocates a fake
per-layer fused-K/V GPU KV cache (vLLM 0.30's NHD layout, [NB, BS, NH, 2*HS]
fp16) and plays the vLLM connector: one request id per batch, a lookup that is
polled until the server answers (L2 prefetch included), a retrieve of the hit
prefix, and end_session when the request finishes.

UNITS. One offloaded "block" (a key) here is one LMCache CHUNK: --block-bytes
bytes of KV covering chunk_size tokens (the server's --chunk-size, default
256). A batch of --bs keys is ONE request whose token sequence is
--bs x chunk_size tokens. LMCache keys are prefix-chained (chunk i's key hashes
all earlier tokens), so a batch is a prefix, not --bs independent keys. Labels,
report and CSV columns match the sibling benches so they join on ``label``.

Run it with run-in-container.sh, which starts a private LMCache server and runs
this script in the vLLM + LMCache image (deploy/lmcache/build.sh):

    benchmarks/lmcache-connector-bench/run-in-container.sh            # default 4 phases
    run-in-container.sh --verify                                      # data round trip first
    run-in-container.sh --mode all --csv bench-results/lmcache.csv --tag lmcache
"""

from __future__ import annotations

import argparse
import collections
import csv
import itertools
import json
import os
import random
import statistics
import sys
import time
import urllib.request
import uuid
from dataclasses import dataclass, field

os.environ.setdefault("LMCACHE_LOG_LEVEL", "WARNING")

import torch

assert torch.cuda.is_available(), "CUDA GPU required"

try:
    import zmq
    from lmcache.integration.vllm.vllm_multi_process_adapter import (
        LMCacheMPSchedulerAdapter,
        LMCacheMPWorkerAdapter,
        LoadStoreOp,
        ParallelStrategy,
    )
except ImportError as _e:
    sys.exit(
        f"error: this benchmark needs LMCache with the MP adapter ({_e}).\n"
        f"       Run it in the vLLM + LMCache image: "
        f"benchmarks/lmcache-connector-bench/run-in-container.sh"
    )

# Default chunk size: 2 MiB, the same per-key size as the sibling benches'
# default block (Llama-3-8B: 16 tokens x 32 layers x 4096 bytes).
DEFAULT_BLOCK_BYTES = 2 * 1024 * 1024
DEFAULT_LAYERS = 32
POLL_S = 0.0001
# Stand-in for one engine step between scheduler retries.
STEP_BACKOFF_S = 0.001


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


# ── LMCache server HTTP side channel (status, metrics, L1 clear) ────────────

def _http(url: str, body: dict | None = None, timeout: float = 30):
    req = urllib.request.Request(
        url, data=None if body is None else json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method="GET" if body is None else "POST")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        raw = r.read().decode()
    return json.loads(raw) if raw[:1] in "{[" else raw


def server_counters(http_url: str) -> collections.Counter:
    """Sum of every lmcache_mp_* sample on the server's /metrics, by name."""
    c: collections.Counter = collections.Counter()
    try:
        text = _http(http_url + "/metrics")
    except Exception:
        return c
    for line in text.splitlines():
        if not line.startswith("lmcache_mp_"):
            continue
        name = line.split("{", 1)[0].split(" ", 1)[0]
        try:
            c[name] += float(line.rsplit(" ", 1)[1])
        except ValueError:
            pass
    return c


# ── fake GPU KV cache ───────────────────────────────────────────────────────

def kv_geometry(block_bytes: int, layers: int, chunk_tokens: int, kv_heads: int | None):
    """(num_heads, content_size) of one fused-K/V fp16 layer tensor so that one
    chunk (chunk_tokens tokens x layers) is exactly block_bytes."""
    per_tok = block_bytes // (layers * chunk_tokens)
    if per_tok * layers * chunk_tokens != block_bytes or per_tok % 4:
        sys.exit(f"error: --block-bytes {block_bytes} must be a multiple of "
                 f"layers x chunk_size x 4 = {layers * chunk_tokens * 4}")
    elems = per_tok // 2                      # fp16 elements per token per layer
    if kv_heads:
        if elems % (2 * kv_heads):
            sys.exit(f"error: --kv-heads {kv_heads} does not divide {elems} elements/token/layer")
        return kv_heads, elems // kv_heads
    for nh in (8, 4, 2, 1):                   # largest head count keeping head_dim >= 64
        if elems % (2 * nh) == 0 and elems // nh >= 128:
            return nh, elems // nh
    return 1, elems


class LMCacheStack:
    """The LMCacheMPConnector's two adapters plus a fake KV cache, playing vLLM.

    A rolling allocator hands out GPU block ids; the pool is sized by the
    caller to exceed the chunks any mode keeps in flight.
    """

    def __init__(self, args, pool_chunks: int):
        torch.cuda.set_device(args.gpu)
        torch.cuda.init()
        self.http_url = args.http_url.rstrip("/")
        self.tokens_per_block = args.tokens_per_block
        ctx = zmq.Context.instance()
        ps = ParallelStrategy(mla_only=False, vllm_world_size=1, vllm_worker_id=0,
                              tp_size=1, pp_size=1, n_servers=1)
        extra = {"lmcache.mp.mq_timeout": args.mq_timeout}
        self.sch = LMCacheMPSchedulerAdapter(
            [args.server_url], ctx, args.model_name, self.tokens_per_block, ps,
            extra_config=extra)
        self.wrk = LMCacheMPWorkerAdapter(
            args.server_url, ctx, args.model_name, self.tokens_per_block, ps,
            extra_config=extra)
        self.chunk_tokens = self.sch.lmcache_tokens_per_chunk
        self.bpc = self.sch.num_blocks_per_chunk()
        self.chunk_bytes = args.block_bytes

        nh, cs = kv_geometry(args.block_bytes, args.layers, self.chunk_tokens, args.kv_heads)
        self.num_heads, self.content_size = nh, cs
        self.num_gpu_blocks = pool_chunks * self.bpc
        self.kv = {
            f"model.layers.{i}.self_attn.attn": torch.randn(
                self.num_gpu_blocks, self.tokens_per_block, nh, cs,
                dtype=torch.float16, device=f"cuda:{args.gpu}")
            for i in range(args.layers)
        }
        torch.cuda.synchronize()
        self.wrk.register_kv_caches(self.kv, layout_hints={"kv_layout": "NHD"})

        self._cursor = 0
        self._seq = itertools.count(1)
        self.pending_stores: set[str] = set()
        self.done_stores: set[str] = set()
        self.done_retrieves: set[str] = set()
        self.load_error_blocks = 0
        self._events: list = []

    # ── ids, tokens, blocks ─────────────────────────────────────────────────
    def new_rid(self, kind: str) -> str:
        return f"{kind}-{next(self._seq)}-{uuid.uuid4().hex[:8]}"

    def make_tokens(self, num_chunks: int) -> list[int]:
        """A fresh random token sequence of num_chunks whole chunks."""
        return torch.randint(1000, 30000, (num_chunks * self.chunk_tokens,)).tolist()

    def alloc_blocks(self, num_tokens: int) -> list[int]:
        n = num_tokens // self.tokens_per_block
        nb = self.num_gpu_blocks
        out = [(self._cursor + j) % nb for j in range(n)]
        self._cursor = (self._cursor + n) % nb
        return out

    def _event(self):
        ev = torch.cuda.Event(interprocess=True)
        ev.record()
        return ev

    # ── worker side ─────────────────────────────────────────────────────────
    def submit_store(self, reqs) -> list[str]:
        """reqs: [(rid, tokens, start_tok, end_tok, blocks|None)]; one batched call."""
        ids, ops = [], []
        for rid, toks, s, e, blocks in reqs:
            if blocks is None:
                blocks = self.alloc_blocks(e - s)
            ops.append(LoadStoreOp(token_ids=toks, block_ids=[blocks], start=s, end=e))
            ids.append(rid)
        if ids:
            self.wrk.batched_submit_store_requests(ids, ops, self._event())
            self.pending_stores.update(ids)
        return ids

    def submit_retrieve(self, reqs) -> list[str]:
        """reqs: [(rid, tokens, start_tok, end_tok, blocks|None)]; one batched call."""
        ids, ops = [], []
        for rid, toks, s, e, blocks in reqs:
            if blocks is None:
                blocks = self.alloc_blocks(e - s)
            ops.append(LoadStoreOp(token_ids=toks, block_ids=[blocks], start=s, end=e))
            ids.append(rid)
        if ids:
            self.wrk.batched_submit_retrieve_requests(ids, ops, self._event())
        return ids

    def poll(self):
        """One worker get_finished() call, as vLLM makes every step."""
        s, r = self.wrk.get_finished(set(self.pending_stores))
        if s:
            self.done_stores |= s
            self.pending_stores -= s
        if r:
            self.done_retrieves |= r
            self.load_error_blocks += len(self.wrk.get_block_ids_with_load_errors())

    def wait(self, stores=(), retrieves=()):
        stores, retrieves = set(stores), set(retrieves)
        while not (stores <= self.done_stores and retrieves <= self.done_retrieves):
            self.poll()
            if not (stores <= self.done_stores and retrieves <= self.done_retrieves):
                time.sleep(POLL_S)
        self.done_stores -= stores
        self.done_retrieves -= retrieves

    # ── scheduler side ──────────────────────────────────────────────────────
    def lookup(self, rid: str, tokens: list[int], phases=None,
               names=("lookup", "lookup_wait")) -> int:
        """Prefix lookup; returns hit tokens. The first check is ``lookup``; the
        polling that follows (server ack, L2 prefetch) is ``lookup_wait``."""
        t0 = time.perf_counter()
        self.sch.maybe_submit_lookup_request(rid, tokens)
        hit = self.sch.check_lookup_result(rid)
        _rec(phases, names[0], t0)
        if hit is None:
            t1 = time.perf_counter()
            while (hit := self.sch.check_lookup_result(rid)) is None:
                time.sleep(POLL_S)
            _rec(phases, names[1], t1)
        self.sch.cleanup_lookup_result(rid)
        return hit

    def submit_lookup(self, rid: str, tokens: list[int]):
        self.sch.maybe_submit_lookup_request(rid, tokens)

    def check_lookup(self, rid: str) -> int | None:
        hit = self.sch.check_lookup_result(rid)
        if hit is not None:
            self.sch.cleanup_lookup_result(rid)
        return hit

    def end_request(self, rid: str):
        self.sch.end_session(rid)

    # ── server-side tier control ────────────────────────────────────────────
    def status(self) -> dict:
        return _http(self.http_url + "/status")

    def has_l2(self) -> bool:
        sc = self.status().get("storage_manager", {}).get("store_controller", {})
        return int(sc.get("num_l2_adapters", 0) or 0) > 0

    def drain_l2(self, timeout: float = 300.0):
        """Wait until the L1->L2 write-through has caught up with L1 writes.

        /status's pending/in-flight counts read zero before a fresh store has
        even been queued to L2, so they cannot be trusted alone. Instead wait
        for the completed-L2-store chunk count to stop moving while nothing is
        pending: write-through skips chunks L2 already holds, so it need not
        reach the L1 write count.
        """
        if not self.has_l2():
            return
        t_end = time.time() + timeout
        prev, stable = None, 0
        while time.time() < t_end:
            c = server_counters(self.http_url)
            done = c["lmcache_mp_l2_store_completed_objects_chunks_total"]
            sub = c["lmcache_mp_l2_store_submitted_objects_chunks_total"]
            sc = self.status().get("storage_manager", {}).get("store_controller", {})
            idle = (not sc.get("pending_keys_count") and not sc.get("in_flight_task_count")
                    and done >= sub)
            stable = stable + 1 if (idle and done == prev) else 0
            if stable >= 3:          # ~30 ms with no new L2 work
                return
            prev = done
            time.sleep(0.01)
        print("  WARNING: L2 write-through still busy after drain timeout")

    def evict_l1(self):
        """Empty L1 after its L2 write-through drained (L2 data kept)."""
        self.drain_l2()
        _http(self.http_url + "/cache/clear", {"tier": "l1", "force": True})

    def shutdown(self):
        try:
            self.wrk.shutdown()
        finally:
            self.sch.shutdown()


# ── one request = one batch of chunks ───────────────────────────────────────

def store_batch(stack: LMCacheStack, tokens: list[int], phases=None, names=None) -> int:
    """Store a whole token sequence as one request; returns chunks stored."""
    nm = names or {}
    rid = stack.new_rid("s")
    t0 = time.perf_counter()
    stack.submit_store([(rid, tokens, 0, len(tokens), None)])
    _rec(phases, nm.get("submit_store", "submit_store"), t0)
    t0 = time.perf_counter()
    stack.wait(stores=[rid])
    _rec(phases, nm.get("get_finished", "get_finished"), t0)
    t0 = time.perf_counter()
    stack.end_request(rid)
    _rec(phases, nm.get("end_session", "end_session"), t0)
    return len(tokens) // stack.chunk_tokens


def load_batch(stack: LMCacheStack, tokens: list[int], phases=None, names=None,
               blocks: list[int] | None = None) -> tuple[int, int]:
    """Lookup + retrieve the hit prefix as one request; returns (loaded, missed) chunks."""
    nm = names or {}
    rid = stack.new_rid("l")
    want = len(tokens) // stack.chunk_tokens
    hit = stack.lookup(rid, tokens, phases,
                       (nm.get("lookup", "lookup"), nm.get("lookup_wait", "lookup_wait")))
    loaded = hit // stack.chunk_tokens
    if hit:
        t0 = time.perf_counter()
        stack.submit_retrieve([(rid, tokens, 0, hit,
                                None if blocks is None else blocks[: hit // stack.tokens_per_block])])
        _rec(phases, nm.get("submit_load", "submit_load"), t0)
        t0 = time.perf_counter()
        stack.wait(retrieves=[rid])
        _rec(phases, nm.get("get_finished", "get_finished"), t0)
    t0 = time.perf_counter()
    stack.end_request(rid)
    _rec(phases, nm.get("end_session", "end_session"), t0)
    return loaded, want - loaded


def make_sequences(stack: LMCacheStack, num_chunks: int, batch_size: int) -> list[list[int]]:
    """num_chunks keys as sequences of batch_size chunks (the last may be shorter)."""
    out, left = [], num_chunks
    while left > 0:
        n = min(batch_size, left)
        out.append(stack.make_tokens(n))
        left -= n
    return out


def populate(stack: LMCacheStack, seqs: list[list[int]]) -> int:
    return sum(store_batch(stack, s) for s in seqs)


# ── default mode phases ─────────────────────────────────────────────────────

def bench_store_lifecycle(stack, num_blocks, batch_size, min_duration) -> BenchResult:
    """submit_store → get_finished → end_session, one request per batch.

    Every pass stores fresh sequences (LMCache dedups chunks it already holds).
    After a pass L1 is emptied (``reset``): that first waits for the pass's
    L1→L2 write-through, then clears L1. The reset is excluded from wall time,
    so the headline is the GPU→L1 store path; the sustained figure including
    the L2 drain is printed below it.
    """
    phases = _phases("submit_store", "get_finished", "end_session", "reset")
    total = 0
    excluded = 0.0
    has_l2 = stack.has_l2()
    wall_start = time.perf_counter()
    iteration = 0
    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for seq in make_sequences(stack, num_blocks, batch_size):
            total += store_batch(stack, seq, phases)
        iteration += 1
        t0 = time.perf_counter()
        stack.evict_l1()
        _rec(phases, "reset", t0)
        excluded += time.perf_counter() - t0
    elapsed = time.perf_counter() - wall_start
    result = BenchResult(
        label="Store Lifecycle (touch → prepare_store → submit → finish → complete)",
        total_blocks=total, total_bytes=total * stack.chunk_bytes,
        wall_seconds=elapsed - excluded, phases=phases)
    result.sustained = (result.total_bytes / elapsed / 1e9, elapsed)
    result.print_report()
    if has_l2:
        print(f"  Sustained incl. L2 write-through drain: "
              f"{result.total_bytes / elapsed / 1e9:.2f} GB/s over {elapsed:.3f}s")
    return result


def bench_load_lifecycle(stack, seqs, min_duration, *, cold: bool) -> BenchResult:
    """lookup (→ lookup_wait) → submit_load → get_finished → end_session.

    Warm: the chunks are in L1 (pinned RAM → GPU).
    Cold: L1 is emptied before every pass, so each lookup prefetches the
    chunks L2 → L1 (``lookup_wait``) before the retrieve copies them to the
    GPU. The empty-out (``demote``) is excluded from wall time.
    """
    phases = _phases("lookup", "lookup_wait", "submit_load", "get_finished", "end_session")
    if cold:
        phases["demote"] = PhaseTiming("demote")
    total = misses = 0
    excluded = 0.0
    c0 = server_counters(stack.http_url)
    wall_start = time.perf_counter()
    iteration = 0
    while (time.perf_counter() - wall_start - excluded) < min_duration or iteration < 2:
        if cold:
            t0 = time.perf_counter()
            stack.evict_l1()
            _rec(phases, "demote", t0)
            excluded += time.perf_counter() - t0
        for seq in seqs:
            loaded, m = load_batch(stack, seq, phases)
            total += loaded
            misses += m
        iteration += 1
    wall = time.perf_counter() - wall_start - excluded
    d = server_counters(stack.http_url) - c0
    temp = "Cold" if cold else "Warm"
    result = BenchResult(
        label=f"{temp} Load Lifecycle (lookup → touch → prepare → submit → finish → complete)",
        total_blocks=total, total_bytes=total * stack.chunk_bytes,
        wall_seconds=wall, phases=phases)
    result.counts = {"load_misses": misses}
    if cold:
        result.counts["disk_read_bytes"] = (
            int(d["lmcache_mp_l2_prefetch_load_completed_chunks_total"]) * stack.chunk_bytes)
    result.print_report()
    if misses or cold:
        print(f"  Load misses: {misses}  "
              f"Prefetched from L2: {int(d['lmcache_mp_l2_prefetch_load_completed_chunks_total'])}")
    return result


def bench_mixed_eviction(stack, batch_size, min_duration, *, working_set=512,
                         load_fraction=0.5) -> BenchResult:
    """Interleaved store + load under L1 pressure.

    Each generation stores new sequences and loads the previous generation's.
    Once L1 is past its eviction watermark, LRU evicts; evicted chunks come
    back from L2 (prefetch) or miss (no L2).

    L1 eviction is asynchronous: a server thread wakes once a second and, if
    L1 is above --eviction-trigger-watermark (0.8), evicts --eviction-ratio
    (0.2) of it. A store into a full L1 does not wait for it; the server
    drops the store (the request still completes, no chunk is written).
    That is LMCache's counterpart of cputier's store drops, and at this
    bench's store rate it dominates: L1 refills between eviction ticks. Each
    store's L1 write count is read from the server's metrics (the scrape is
    excluded from wall time); a dropped batch backs off one engine step and
    is not loaded by the next generation, as in the sibling benches.
    """
    names_store = {"get_finished": "store_finish", "end_session": "store_end_session"}
    names_load = {"get_finished": "load_finish", "end_session": "load_end_session"}
    phases = _phases("submit_store", "store_finish", "store_end_session",
                     "lookup", "lookup_wait", "submit_load", "load_finish", "load_end_session")
    load_count = max(1, int(working_set * load_fraction))
    store_count = working_set - load_count
    l1 = stack.status()["storage_manager"]["l1_manager"]["memory_total_bytes"]
    l1_chunks = l1 // stack.chunk_bytes
    print(f"\n  Eviction mode: working_set={working_set} "
          f"(store={store_count}/gen, load={load_count}/gen, chunk_bytes={stack.chunk_bytes})")
    print(f"  L1 ({l1_chunks} chunks) fills after ~{l1_chunks // max(1, store_count)} "
          f"generations, then eviction kicks in.")

    stack.evict_l1()
    c0 = server_counters(stack.http_url)
    loadable = make_sequences(stack, load_count, batch_size)
    populate(stack, loadable)

    total = load_misses = store_drops = 0
    excluded = 0.0
    wall_start = time.perf_counter()
    generation = 0
    l1_writes = server_counters(stack.http_url)["lmcache_mp_l1_write_chunks_total"]
    while (time.perf_counter() - wall_start - excluded) < min_duration or generation < 3:
        generation += 1
        stored: list[list[int]] = []
        for seq in make_sequences(stack, store_count, batch_size):
            want = store_batch(stack, seq, phases, names_store)
            t0 = time.perf_counter()
            now = server_counters(stack.http_url)["lmcache_mp_l1_write_chunks_total"]
            excluded += time.perf_counter() - t0
            got, l1_writes = int(now - l1_writes), now
            total += got
            if got >= want:
                stored.append(seq)
            else:
                store_drops += want - got
                time.sleep(STEP_BACKOFF_S)
        for seq in loadable:
            loaded, m = load_batch(stack, seq, phases, names_load)
            total += loaded
            load_misses += m
        loadable = stored
    wall = time.perf_counter() - wall_start - excluded
    stack.drain_l2()
    d = server_counters(stack.http_url) - c0
    result = BenchResult(
        label=(f"Mixed Store+Load Under Eviction Pressure "
               f"(ws={working_set}, load_frac={load_fraction:.0%})"),
        total_blocks=total, total_bytes=total * stack.chunk_bytes,
        wall_seconds=wall, phases=phases)
    result.counts = {"store_drops": store_drops, "load_misses": load_misses,
                     "disk_read_bytes": int(d["lmcache_mp_l2_prefetch_load_completed_chunks_total"])
                                        * stack.chunk_bytes}
    result.print_report()
    print(f"  L1 evictions: {int(d['lmcache_mp_l1_evicted_chunks_total'])}  "
          f"Store drops (L1 full): {store_drops}  Load misses: {load_misses}")
    if stack.has_l2():
        w = int(d["lmcache_mp_l2_store_completed_objects_chunks_total"])
        p = int(d["lmcache_mp_l2_prefetch_load_completed_chunks_total"])
        print(f"  L2: writes={w} ({w * stack.chunk_bytes / 1e6:.0f} MB)  "
              f"prefetches={p} ({p * stack.chunk_bytes / 1e6:.0f} MB)")
    return result


# ── mode: pipelined ─────────────────────────────────────────────────────────

def bench_pipelined(stack, num_blocks, batch_size, min_duration, *,
                    pipeline_depth=4, direction="store") -> BenchResult:
    """Submit pipeline_depth requests before reaping.

    Stores: fresh sequences each pass; the per-pass L1 reset (L2 drain) is
    excluded. Loads: the lookup is part of ``prepare``; the retrieve is
    submitted without waiting, then the depth is drained.
    """
    is_store = direction == "store"
    phases = _phases("prepare", "submit", "pipeline_drain", "complete")
    seqs = None if is_store else make_sequences(stack, num_blocks, batch_size)
    if seqs:
        populate(stack, seqs)
    total = 0
    excluded = 0.0
    wall_start = time.perf_counter()
    iteration = 0

    def _drain(in_flight):
        t0 = time.perf_counter()
        if is_store:
            stack.wait(stores=[r for r, _ in in_flight])
        else:
            stack.wait(retrieves=[r for r, _ in in_flight])
        for rid, _ in in_flight:
            t_c = time.perf_counter()
            stack.end_request(rid)
            _rec(phases, "complete", t_c)
        in_flight.clear()
        _rec(phases, "pipeline_drain", t0)

    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        in_flight: list = []
        pass_seqs = make_sequences(stack, num_blocks, batch_size) if is_store else seqs
        for seq in pass_seqs:
            rid = stack.new_rid("p")
            t0 = time.perf_counter()
            end = len(seq)
            if not is_store:
                end = stack.lookup(rid, seq)
                if not end:
                    stack.end_request(rid)
                    continue
            _rec(phases, "prepare", t0)
            t0 = time.perf_counter()
            if is_store:
                stack.submit_store([(rid, seq, 0, end, None)])
            else:
                stack.submit_retrieve([(rid, seq, 0, end, None)])
            _rec(phases, "submit", t0)
            in_flight.append((rid, end))
            total += end // stack.chunk_tokens
            if len(in_flight) >= pipeline_depth:
                _drain(in_flight)
        if in_flight:
            _drain(in_flight)
        iteration += 1
        if is_store:
            t0 = time.perf_counter()
            stack.evict_l1()
            excluded += time.perf_counter() - t0
    elapsed = time.perf_counter() - wall_start
    result = BenchResult(
        label=f"Pipelined {'Store' if is_store else 'Load'} (depth={pipeline_depth})",
        total_blocks=total, total_bytes=total * stack.chunk_bytes,
        wall_seconds=elapsed - excluded, phases=phases)
    if is_store:
        result.sustained = (result.total_bytes / elapsed / 1e9, elapsed)
    result.print_report()
    if is_store and stack.has_l2():
        print(f"  Sustained incl. L2 write-through drain: "
              f"{result.total_bytes / elapsed / 1e9:.2f} GB/s over {elapsed:.3f}s")
    return result


# ── mode: prefix-miss ───────────────────────────────────────────────────────

def bench_prefix_miss(stack, num_blocks, batch_size, min_duration, *, hit_ratio=0.75) -> BenchResult:
    """Cost of a maximal-prefix lookup at a given hit ratio.

    As in the sibling benches, the first hit_ratio x num_blocks keys are
    present, so each batch is fully present, absent, or (one batch) partial.
    LMCache answers the whole prefix in ONE lookup round trip per request
    (there is no per-key scan); ``lookup_hit`` / ``lookup_miss`` are those
    per-request lookups split by whether any prefix hit, ``load`` retrieves it.
    """
    seqs = make_sequences(stack, num_blocks, batch_size)
    present = max(0, min(num_blocks, int(num_blocks * hit_ratio)))
    left = present
    for seq in seqs:
        n = min(left, len(seq) // stack.chunk_tokens)
        if n:
            rid = stack.new_rid("s")
            stack.submit_store([(rid, seq, 0, n * stack.chunk_tokens, None)])
            stack.wait(stores=[rid])
            stack.end_request(rid)
        left -= n

    phases = _phases("lookup_hit", "lookup_miss", "prefix_scan", "load")
    scans = prefix_hits = total = 0
    wall_start = time.perf_counter()
    iteration = 0
    while (time.perf_counter() - wall_start) < min_duration or iteration < 2:
        for seq in seqs:
            rid = stack.new_rid("q")
            t0 = time.perf_counter()
            hit = stack.lookup(rid, seq)
            _rec(phases, "prefix_scan", t0)
            _rec(phases, "lookup_hit" if hit else "lookup_miss", t0)
            scans += 1
            n = hit // stack.chunk_tokens
            prefix_hits += n
            if hit:
                t0 = time.perf_counter()
                stack.submit_retrieve([(rid, seq, 0, hit, None)])
                stack.wait(retrieves=[rid])
                _rec(phases, "load", t0)
                total += n
            stack.end_request(rid)
        iteration += 1
    wall = time.perf_counter() - wall_start
    avg = prefix_hits / scans if scans else 0
    result = BenchResult(
        label=f"Prefix-Miss Scan (hit_ratio={hit_ratio:.0%}, avg_prefix={avg:.1f}/{batch_size})",
        total_blocks=total, total_bytes=total * stack.chunk_bytes,
        wall_seconds=wall, phases=phases)
    result.print_report()
    print(f"  Scans={scans}  avg_prefix_len={avg:.1f}  "
          f"lookup_hits={len(phases['lookup_hit'].samples)}  "
          f"lookup_misses={len(phases['lookup_miss'].samples)}")
    return result


# ── mode: scheduler-step ────────────────────────────────────────────────────

def bench_scheduler_step(stack, batch_size, min_duration, *, requests_per_step=8,
                         num_sessions=32, num_blocks=128) -> BenchResult:
    """Continuous-batching scheduler steps, as LMCacheMPConnector drives them.

    Sessions share one prefix pool (a token sequence stored up front) and
    diverge at geometric points; each request is session prefix + a fresh
    suffix. Per step: submit every request's lookup, poll them all until
    answered (``touch_lookup``; vLLM retries an unanswered lookup on the next
    step, here we poll), retrieve the hit prefixes in one batched call
    (``prepare_load``/``submit``) and drain them (``drain_workers``). The
    requests' stores (from the hit end to the sequence end, as the connector
    does) are submitted after the load drain and drained, untimed, at the
    start of the next step; end_session follows the store.
    """
    kpr = max(4, num_blocks // requests_per_step)
    prefix_len = kpr // 2
    suffix_len = kpr - prefix_len
    ct = stack.chunk_tokens
    phases = _phases("touch_lookup", "prepare_load", "prepare_store", "submit",
                     "drain_workers", "complete")

    pool_chunks = min(prefix_len * 4, num_blocks * 2)
    pool = stack.make_tokens(pool_chunks)
    for s in range(0, pool_chunks, batch_size):
        e = min(pool_chunks, s + batch_size)
        rid = stack.new_rid("s")
        stack.submit_store([(rid, pool, s * ct, e * ct, None)])
        stack.wait(stores=[rid])
        stack.end_request(rid)
    rng = random.Random(42)
    sessions = [max(2, min(pool_chunks, int(rng.expovariate(1.0 / prefix_len))))
                for _ in range(num_sessions)]

    total = steps = prefix_hit_chunks = suffix_store_chunks = 0
    prev: list[str] = []
    wall_start = time.perf_counter()

    def drain_prev():
        if prev:
            stack.wait(stores=prev)
            for rid in prev:
                stack.end_request(rid)

    while (time.perf_counter() - wall_start) < min_duration or steps < 3:
        drain_prev()
        reqs = []
        for _ in range(requests_per_step):
            p = sessions[rng.randrange(num_sessions)]
            toks = pool[: p * ct] + stack.make_tokens(suffix_len)
            reqs.append([stack.new_rid("r"), toks, None])

        t0 = time.perf_counter()
        for r in reqs:
            stack.submit_lookup(r[0], r[1])
        pending = list(reqs)
        while pending:
            still = []
            for r in pending:
                h = stack.check_lookup(r[0])
                if h is None:
                    still.append(r)
                else:
                    r[2] = h
            pending = still
            if pending:
                time.sleep(POLL_S)
        _rec(phases, "touch_lookup", t0)

        t0 = time.perf_counter()
        loads = [(rid, toks, 0, h, None) for rid, toks, h in reqs if h]
        _rec(phases, "prepare_load", t0)
        t0 = time.perf_counter()
        ids = stack.submit_retrieve(loads)
        _rec(phases, "submit", t0)
        t0 = time.perf_counter()
        stack.wait(retrieves=ids)
        _rec(phases, "drain_workers", t0)
        hit_chunks = sum(h // ct for _, _, h in reqs)
        prefix_hit_chunks += hit_chunks
        total += hit_chunks

        t0 = time.perf_counter()
        stores = [(rid, toks, h, len(toks), None) for rid, toks, h in reqs if h < len(toks)]
        _rec(phases, "prepare_store", t0)
        t0 = time.perf_counter()
        prev = stack.submit_store(stores)
        _rec(phases, "complete", t0)
        n_store = sum((e - s) // ct for _, _, s, e, _ in stores)
        suffix_store_chunks += n_store
        total += n_store
        # A request with nothing to store finishes now; the others finish
        # once their store drains (next step), as vLLM delays freeing them.
        for rid, toks, h in reqs:
            if h >= len(toks):
                stack.end_request(rid)
        steps += 1
    drain_prev()
    wall = time.perf_counter() - wall_start
    result = BenchResult(
        label=(f"Scheduler Step (reqs={requests_per_step}, "
               f"sessions={num_sessions}, keys/req={kpr})"),
        total_blocks=total, total_bytes=total * stack.chunk_bytes,
        wall_seconds=wall, phases=phases)
    result.print_report()
    print(f"  Steps={steps}  steps/s={steps / wall:.1f}  "
          f"prefix_hits={prefix_hit_chunks}  suffix_stores={suffix_store_chunks}")
    return result


# ── verify ──────────────────────────────────────────────────────────────────

def verify_roundtrip(stack: LMCacheStack, num_chunks: int, *, cold: bool) -> bool:
    """Store random GPU blocks, zero them, retrieve them back, compare bytes."""
    n_blocks = num_chunks * stack.bpc
    blocks = list(range(n_blocks))
    for t in stack.kv.values():
        t[:n_blocks].normal_()
    expected = [t[:n_blocks].clone() for t in stack.kv.values()]
    torch.cuda.synchronize()

    toks = stack.make_tokens(num_chunks)
    rid = stack.new_rid("vs")
    stack.submit_store([(rid, toks, 0, len(toks), blocks)])
    stack.wait(stores=[rid])
    stack.end_request(rid)
    for t in stack.kv.values():
        t[:n_blocks].zero_()
    torch.cuda.synchronize()
    if cold:
        stack.evict_l1()

    err0 = stack.load_error_blocks
    loaded, missed = load_batch(stack, toks, blocks=blocks)
    torch.cuda.synchronize()
    if loaded != num_chunks:
        print(f"  verify: loaded {loaded}/{num_chunks} chunks (missed {missed})")
        return False
    bad = sum(0 if torch.equal(t[:n_blocks], e) else 1 for t, e in zip(stack.kv.values(), expected))
    if bad or stack.load_error_blocks != err0:
        print(f"  verify: {bad}/{len(expected)} layer tensors differ, "
              f"{stack.load_error_blocks - err0} blocks reported load errors")
        return False
    return True


# ── main ────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="End-to-end LMCache (MP adapter) connector lifecycle benchmark",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Modes (--mode):\n"
            "\n"
            "  default        Four serial phases: store, warm load, cold load, mixed eviction\n"
            "  pipelined      Submit N requests before reaping\n"
            "  prefix-miss    Vary prefix hit ratio to measure the prefix lookup cost\n"
            "  scheduler-step Scheduler steps with multi-request prefix sharing\n"
            "  all            All of the above\n"
            "\n"
            "Exercises LMCacheMPSchedulerAdapter + LMCacheMPWorkerAdapter against a\n"
            "running `lmcache server` (run-in-container.sh starts one)."
        ),
    )
    parser.add_argument("--server-url", default="tcp://127.0.0.1:5575",
                        help="LMCache server ZMQ URL")
    parser.add_argument("--http-url", default="http://127.0.0.1:8095",
                        help="LMCache server HTTP URL (/status, /metrics, /cache/clear)")
    parser.add_argument("--mq-timeout", type=float, default=30.0,
                        help="lmcache.mp.mq_timeout for registration and blocking calls (s)")
    parser.add_argument("--model-name", default="lmcache-connector-bench",
                        help="model name in the LMCache keys")
    parser.add_argument("--layers", type=int, default=DEFAULT_LAYERS,
                        help="per-layer KV tensors (copy descriptors per chunk)")
    parser.add_argument("--tokens-per-block", type=int, default=16,
                        help="vLLM GPU block size in tokens")
    parser.add_argument("--kv-heads", type=int, default=None,
                        help="KV heads per layer tensor (default: derived from --block-bytes)")
    parser.add_argument("--verify", action="store_true",
                        help="store random GPU data, zero it, retrieve it (warm, and cold "
                             "through L2) and compare bytes before benchmarking")
    parser.add_argument("--bs", type=int, default=16,
                        help="keys (chunks) per request")
    parser.add_argument("--num-blocks", type=int, default=128,
                        help="keys per pass")
    parser.add_argument("--block-bytes", type=parse_size, default=DEFAULT_BLOCK_BYTES,
                        help="bytes per key = one LMCache chunk of all layers")
    parser.add_argument("--gpu", type=int, default=0)
    parser.add_argument("--tp", type=int, default=1,
                        help="tensor parallel size (only 1 is supported)")
    parser.add_argument("--min-duration", type=float, default=5.0)
    parser.add_argument("--warmup", type=int, default=4)
    parser.add_argument("--working-set", type=int, default=512,
                        help="mixed-eviction keys per generation (store + load)")
    parser.add_argument("--csv", type=str, default=None, help="append results to CSV")
    parser.add_argument("--tag", type=str, default="", help="CSV tag column")
    parser.add_argument("--no-cold", action="store_true")
    parser.add_argument("--no-eviction", action="store_true")
    parser.add_argument("--mode", default="default",
                        choices=["default", "pipelined", "prefix-miss", "scheduler-step", "all"])
    parser.add_argument("--pipeline-depth", type=int, default=4)
    parser.add_argument("--direction", default="both", choices=["store", "load", "both"])
    parser.add_argument("--hit-ratio", type=float, default=0.75)
    parser.add_argument("--requests-per-step", type=int, default=8)
    parser.add_argument("--num-sessions", type=int, default=32)
    parser.add_argument("--lookup-order", default="lookup-first",
                        choices=["lookup-first", "touch-first"],
                        help="accepted for flag parity with the sibling benches; "
                             "LMCache has no touch call, so it changes nothing")
    args = parser.parse_args()
    if args.tp != 1:
        parser.error("--tp > 1 is not supported (one worker adapter, one GPU)")

    kpr = max(4, args.num_blocks // args.requests_per_step)
    pool_chunks = max(args.bs * max(1, args.pipeline_depth),
                      2 * args.requests_per_step * kpr, args.bs) + args.bs

    import importlib.metadata as md
    lmc_version = md.version("lmcache")
    print(f"LMCache Connector Lifecycle Benchmark (lmcache {lmc_version}, torch {torch.__version__})")
    print(f"  server={args.server_url}  http={args.http_url}  gpu={args.gpu}")
    stack = LMCacheStack(args, pool_chunks)
    st = stack.status()["storage_manager"]
    l1_bytes = st["l1_manager"]["memory_total_bytes"]
    has_l2 = stack.has_l2()
    print(f"  chunk_tokens={stack.chunk_tokens}  blocks_per_chunk={stack.bpc}  "
          f"block_bytes(chunk)={stack.chunk_bytes}  layers={args.layers}  "
          f"kv=[{stack.num_gpu_blocks}, {args.tokens_per_block}, {stack.num_heads}, "
          f"{stack.content_size}] fp16")
    print(f"  num_blocks={args.num_blocks}  bs={args.bs}  min_duration={args.min_duration}s  "
          f"L1={l1_bytes / 2**30:.1f} GiB ({l1_bytes // stack.chunk_bytes} chunks)  "
          f"L2={'yes' if has_l2 else 'none'}")

    try:
        results = _run(args, stack, has_l2)
    finally:
        stack.shutdown()

    if args.csv and results:
        extra = {
            "tag": args.tag, "mode": args.mode, "backend": "lmcache",
            "l1_bytes": l1_bytes, "l2_tier": int(has_l2),
            "chunk_tokens": stack.chunk_tokens, "gpu": args.gpu, "tp": 1,
            "bs": args.bs, "num_blocks": args.num_blocks, "block_bytes": args.block_bytes,
            "layers": args.layers, "working_set": args.working_set,
            "lookup_order": args.lookup_order, "lmcache_version": lmc_version,
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


def _run(args, stack: LMCacheStack, has_l2: bool) -> list[BenchResult]:
    if args.verify:
        print("\nVerify round-trip data integrity...")
        n = max(1, min(args.bs, args.num_blocks))
        ok = verify_roundtrip(stack, n, cold=False)
        print(f"  warm (GPU→L1→GPU): {'OK' if ok else 'FAILED'}")
        if has_l2:
            ok_cold = verify_roundtrip(stack, n, cold=True)
            print(f"  cold (GPU→L1→L2→L1→GPU): {'OK' if ok_cold else 'FAILED'}")
            ok = ok and ok_cold
        if not ok:
            sys.exit(1)

    print(f"\nWarmup ({args.warmup} store/load cycles)...")
    for _ in range(args.warmup):
        seq = stack.make_tokens(1)
        store_batch(stack, seq)
        load_batch(stack, seq)
    stack.evict_l1()

    def run_mode(m: str) -> list[BenchResult]:
        out: list[BenchResult] = []
        if m == "pipelined":
            print(f"\n  Mode: pipelined (depth={args.pipeline_depth}, direction={args.direction})")
            for d in ("store", "load"):
                if args.direction in (d, "both"):
                    stack.evict_l1()
                    out.append(bench_pipelined(stack, args.num_blocks, args.bs, args.min_duration,
                                               pipeline_depth=args.pipeline_depth, direction=d))
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
            seqs = make_sequences(stack, args.num_blocks, args.bs)
            populate(stack, seqs)
            out.append(bench_load_lifecycle(stack, seqs, args.min_duration, cold=False))
            if args.no_cold:
                pass
            elif not has_l2:
                print("\n  (cold load skipped: the server has no L2 adapter)")
            else:
                out.append(bench_load_lifecycle(stack, seqs, args.min_duration, cold=True))
            if not args.no_eviction:
                out.append(bench_mixed_eviction(stack, args.bs, args.min_duration,
                                                working_set=args.working_set))
        stack.evict_l1()
        if stack.load_error_blocks:
            print(f"  WARNING: {stack.load_error_blocks} GPU blocks reported retrieve errors")
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
