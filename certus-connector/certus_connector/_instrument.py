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

        print(
            f"[INSTR] store={c.store_blocks_completed}blk "
            f"load={c.load_blocks_completed}blk "
            f"lookup={c.lookup_calls} prep_store={c.prepare_store_calls} "
            f"prep_load={c.prepare_load_calls} evict={c.evictions}",
            file=sys.stderr,
        )
        print(
            f"[TPUT] store={store_mbps:.0f}MB/s (p50={sp50:.1f}ms p95={sp95:.1f}ms) "
            f"| load={load_mbps:.0f}MB/s (p50={lp50:.1f}ms p95={lp95:.1f}ms)",
            file=sys.stderr,
        )


_reporter_started = False


def start_reporter():
    global _reporter_started
    if _reporter_started:
        return
    _reporter_started = True
    t = threading.Thread(target=_reporter, daemon=True, name="certus-instr")
    t.start()
