# SPDX-License-Identifier: Apache-2.0
"""Low-overhead aggregate RPC telemetry for the Certus gRPC connector."""

from __future__ import annotations

import atexit
import os
import sys
import threading
import time
from dataclasses import dataclass, field


def _env_flag(name: str, default: str = "0") -> bool:
    value = os.environ.get(name, default).strip().lower()
    return value not in {"", "0", "false", "no", "off"}


ENABLED = _env_flag("CERTUS_GRPC_RPC_TELEMETRY")
_SAMPLE_LIMIT = int(os.environ.get("CERTUS_GRPC_RPC_TELEMETRY_SAMPLES", "100000"))
_REPORT_INTERVAL_S = float(os.environ.get("CERTUS_GRPC_RPC_TELEMETRY_INTERVAL_S", "30"))
_START_NS = time.perf_counter_ns()
_NEXT_REPORT_NS = _START_NS + int(_REPORT_INTERVAL_S * 1e9)
_LOCK = threading.Lock()
_REGISTERED = False


@dataclass
class _RpcStats:
    calls: int = 0
    errors: int = 0
    items: int = 0
    total_ns: int = 0
    min_ns: int | None = None
    max_ns: int = 0
    samples_ns: list[int] = field(default_factory=list)

    def add(self, elapsed_ns: int, items: int, ok: bool) -> None:
        self.calls += 1
        self.items += max(0, int(items))
        self.total_ns += elapsed_ns
        self.max_ns = max(self.max_ns, elapsed_ns)
        self.min_ns = elapsed_ns if self.min_ns is None else min(self.min_ns, elapsed_ns)
        if not ok:
            self.errors += 1
        if len(self.samples_ns) < _SAMPLE_LIMIT:
            self.samples_ns.append(elapsed_ns)


_STATS: dict[str, _RpcStats] = {}


def call_rpc(stub, method: str, request, *, items: int = 0):
    """Call ``stub.<method>(request)`` and optionally record aggregate timing.

    Telemetry is disabled by default. With ``CERTUS_GRPC_RPC_TELEMETRY=1`` this
    records one monotonic timestamp pair per RPC and prints a summary at process
    exit. The disabled path is just a direct bound-method call.
    """

    rpc = getattr(stub, method)
    if not ENABLED:
        return rpc(request)

    _register_report_once()
    start_ns = time.perf_counter_ns()
    ok = False
    try:
        response = rpc(request)
        ok = True
        return response
    finally:
        elapsed_ns = time.perf_counter_ns() - start_ns
        should_report = False
        with _LOCK:
            stats = _STATS.setdefault(method, _RpcStats())
            stats.add(elapsed_ns, items, ok)
            should_report = _should_report_locked()
        if should_report:
            report()


def _percentile(sorted_samples: list[int], pct: float) -> int:
    if not sorted_samples:
        return 0
    idx = int(round((len(sorted_samples) - 1) * pct))
    return sorted_samples[idx]


def report() -> None:
    if not ENABLED:
        return
    with _LOCK:
        snapshot = {name: stats for name, stats in _STATS.items()}
    if not snapshot:
        return

    elapsed_s = (time.perf_counter_ns() - _START_NS) / 1e9
    total_calls = sum(s.calls for s in snapshot.values())
    total_ms = sum(s.total_ns for s in snapshot.values()) / 1e6
    print(
        f"[certus-grpc][rpc] summary pid={os.getpid()} "
        f"uptime_s={elapsed_s:.1f} calls={total_calls} rpc_time_ms={total_ms:.1f}",
        file=sys.stderr,
        flush=True,
    )
    print(
        "[certus-grpc][rpc] method calls errors items avg_us p50_us p95_us "
        "max_us items_per_call us_per_item",
        file=sys.stderr,
        flush=True,
    )
    for name in sorted(snapshot):
        stats = snapshot[name]
        samples = sorted(stats.samples_ns)
        avg_us = (stats.total_ns / stats.calls) / 1000 if stats.calls else 0.0
        p50_us = _percentile(samples, 0.50) / 1000
        p95_us = _percentile(samples, 0.95) / 1000
        max_us = stats.max_ns / 1000
        items_per_call = stats.items / stats.calls if stats.calls else 0.0
        us_per_item = (stats.total_ns / stats.items) / 1000 if stats.items else 0.0
        print(
            f"[certus-grpc][rpc] {name} {stats.calls} {stats.errors} "
            f"{stats.items} {avg_us:.1f} {p50_us:.1f} {p95_us:.1f} "
            f"{max_us:.1f} {items_per_call:.1f} {us_per_item:.1f}",
            file=sys.stderr,
            flush=True,
        )


def _should_report_locked() -> bool:
    global _NEXT_REPORT_NS
    if _REPORT_INTERVAL_S <= 0:
        return False
    now_ns = time.perf_counter_ns()
    if now_ns < _NEXT_REPORT_NS:
        return False
    _NEXT_REPORT_NS = now_ns + int(_REPORT_INTERVAL_S * 1e9)
    return True


def _register_report_once() -> None:
    global _REGISTERED
    if _REGISTERED:
        return
    with _LOCK:
        if _REGISTERED:
            return
        atexit.register(report)
        _REGISTERED = True
