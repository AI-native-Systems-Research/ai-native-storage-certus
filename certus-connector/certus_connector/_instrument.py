# SPDX-License-Identifier: Apache-2.0
"""Throughput instrumentation for Certus connector.

Provides a global COUNTERS singleton and a background reporter thread that
prints [INSTR] and [TPUT] lines to stderr every 5 seconds.
"""

from __future__ import annotations

import sys
import threading
import time
from dataclasses import dataclass, field


def _percentile(data: list[float], p: float) -> float:
    if not data:
        return 0.0
    s = sorted(data)
    idx = int(len(s) * p / 100)
    return s[min(idx, len(s) - 1)]


@dataclass
class Counters:
    # Store (GPU → Certus)
    store_blocks_submitted: int = 0
    store_blocks_completed: int = 0
    store_total_bytes: int = 0
    store_latencies: list[float] = field(default_factory=list)

    # Load (Certus → GPU)
    load_blocks_submitted: int = 0
    load_blocks_completed: int = 0
    load_total_bytes: int = 0
    load_latencies: list[float] = field(default_factory=list)

    # Manager
    lookup_calls: int = 0
    prepare_store_calls: int = 0
    prepare_load_calls: int = 0
    evictions: int = 0


COUNTERS = Counters()

_last_store_bytes = 0
_last_load_bytes = 0
_last_time = 0.0


def _reporter():
    global _last_store_bytes, _last_load_bytes, _last_time
    _last_time = time.monotonic()
    while True:
        time.sleep(5.0)
        now = time.monotonic()
        dt = now - _last_time
        _last_time = now

        c = COUNTERS

        # Delta throughput
        ds = c.store_total_bytes - _last_store_bytes
        dl = c.load_total_bytes - _last_load_bytes
        _last_store_bytes = c.store_total_bytes
        _last_load_bytes = c.load_total_bytes

        store_mbps = (ds / 1e6) / dt if dt > 0 else 0
        load_mbps = (dl / 1e6) / dt if dt > 0 else 0

        # Latency percentiles (drain)
        s_lat = c.store_latencies[:]
        l_lat = c.load_latencies[:]
        c.store_latencies.clear()
        c.load_latencies.clear()

        sp50 = _percentile(s_lat, 50)
        sp95 = _percentile(s_lat, 95)
        lp50 = _percentile(l_lat, 50)
        lp95 = _percentile(l_lat, 95)

        # Diagnostic: sample live DRAM memory-tier occupancy so we can see
        # whether the tier ever fills (and thus whether capacity eviction fires).
        mt_str = ""
        eng = _ENGINE_REF
        if eng is not None and hasattr(eng, "mem_tier_usage"):
            try:
                used, cap = eng.mem_tier_usage()
                pct = (100.0 * used / cap) if cap else 0.0
                mt_str = (f" mt_used={used / 1024**3:.2f}/"
                          f"{cap / 1024**3:.2f}GiB ({pct:.0f}%)")
            except Exception:
                pass

        print(
            f"[INSTR] store={c.store_blocks_completed}blk "
            f"load={c.load_blocks_completed}blk "
            f"lookup={c.lookup_calls} prep_store={c.prepare_store_calls} "
            f"prep_load={c.prepare_load_calls} evict={c.evictions}{mt_str}",
            file=sys.stderr,
        )
        print(
            f"[TPUT] store={store_mbps:.0f}MB/s (p50={sp50:.1f}ms p95={sp95:.1f}ms) "
            f"| load={load_mbps:.0f}MB/s (p50={lp50:.1f}ms p95={lp95:.1f}ms)",
            file=sys.stderr,
        )


_reporter_started = False

# The real CertusEngine lives in the vLLM EngineCore worker PROCESS (the spec's
# handlers are constructed there), not the driver process running the benchmark
# loop. To let an out-of-process reader (e.g. run_multiturn_certus.py) sample
# per-round SSD I/O, a thread in this worker process periodically writes the
# engine's cumulative read_write_stats() to a small file. Path via CERTUS_IOSTAT_FILE
# (default /tmp/certus_iostat.txt). Line format:
# "read_ops read_bytes read_latency_ns_sum write_ops write_bytes write_latency_ns_sum".
_ENGINE_REF = None


def set_engine(engine):
    """Register the CertusEngine so the iostat writer thread can sample it."""
    global _ENGINE_REF
    _ENGINE_REF = engine


def _iostat_writer():
    import os
    path = os.environ.get("CERTUS_IOSTAT_FILE", "/tmp/certus_iostat.txt")
    tmp = path + ".tmp"
    while True:
        time.sleep(0.5)
        eng = _ENGINE_REF
        if eng is None or not hasattr(eng, "read_write_stats"):
            continue
        try:
            vals = eng.read_write_stats()  # 6-tuple incl. per-direction latency sums
            # Append cache-level counters (mem_tier_hits, ssd_hits, misses,
            # dram_evictions) when the engine exposes them, so a reader can see
            # what fraction of loads were served from DRAM vs SSD and how many
            # blocks were evicted from DRAM under pressure. Line becomes 10
            # fields; older readers that slice [:6] stay compatible.
            if hasattr(eng, "cache_stats"):
                vals = tuple(vals) + tuple(eng.cache_stats())
            # Diagnostic fields 11-13: entry_count (index entries), mt_used,
            # mt_capacity (bytes). Lets a reader compare the dispatch-map index
            # against actual resident memory-tier bytes per round.
            if hasattr(eng, "entry_count"):
                vals = tuple(vals) + (eng.entry_count(),)
            if hasattr(eng, "mem_tier_usage"):
                vals = tuple(vals) + tuple(eng.mem_tier_usage())
            # Fields 14-16: live dispatch-map index counts by location
            # (total, memory_tier, block_device). Compare memory_tier against
            # mt_used/SLAB to detect stale index entries.
            if hasattr(eng, "index_stats"):
                vals = tuple(vals) + tuple(eng.index_stats())
            with open(tmp, "w") as f:
                f.write(" ".join(str(v) for v in vals) + "\n")
            os.replace(tmp, path)  # atomic publish
        except Exception:
            pass


def start_reporter():
    global _reporter_started
    if _reporter_started:
        return
    _reporter_started = True
    t = threading.Thread(target=_reporter, daemon=True, name="certus-instr")
    t.start()
    w = threading.Thread(target=_iostat_writer, daemon=True, name="certus-iostat")
    w.start()
