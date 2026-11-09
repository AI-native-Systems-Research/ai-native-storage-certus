#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""microbench_shm.py — control-plane round-trip latency of the SHM ring.

The whole point of the shared-memory connector is to cut the *per-op* control
cost. This microbench isolates that cost: it times synchronous ``Check`` round-
trips of a fixed key batch over the ``/dev/shm`` mailbox and reports the latency
distribution, so the transport cost is measured directly (no vLLM, no GPU, no
SSD in the path — Check touches only the in-memory index).

``Check`` is chosen because it is the smallest, purest control op: request is a
list of u64 keys, response a list of bools, and it hits neither the DMA path nor
the SSD — so the number it produces is the bare transport + dispatch cost.

  # needs a running certus-server on --shm-path
  python microbench_shm.py --shm-path /dev/shm/certus-shmq
  python microbench_shm.py --shm-path /dev/shm/certus-shmq --batch 8 --iters 50000

The keys are arbitrary (0xB0000000+i); a hit/miss doesn't change the transport
cost, which is what we are measuring. Run on the same host as the server.

(Historical note: this file used to carry a second arm that timed the same
``Check`` over gRPC for a head-to-head speedup. gRPC has been removed from
Certus — the SHM ring is the sole control transport — so only the SHM arm
remains. The earlier finding that motivated the connector, ~19x lower Check
round-trip latency vs gRPC, is recorded in the project memory/notes.)
"""

from __future__ import annotations

import argparse
import os
import sys
import time

_here = os.path.dirname(os.path.abspath(__file__))
if _here not in sys.path:
    sys.path.insert(0, _here)


def _percentiles(samples_ns: list[int]) -> dict[str, float]:
    """p50/p90/p99/max/mean in microseconds from a list of ns samples."""
    s = sorted(samples_ns)
    n = len(s)

    def pct(p: float) -> float:
        # Nearest-rank; clamp the index into range.
        idx = min(n - 1, max(0, int(round(p / 100.0 * n)) - 1))
        return s[idx] / 1000.0

    return {
        "p50": pct(50),
        "p90": pct(90),
        "p99": pct(99),
        "max": s[-1] / 1000.0,
        "mean": (sum(s) / n) / 1000.0,
    }


def _report(label: str, samples_ns: list[int], batch: int) -> dict[str, float]:
    p = _percentiles(samples_ns)
    total_s = sum(samples_ns) / 1e9
    reqs = len(samples_ns)
    rps = reqs / total_s if total_s else 0.0
    print(
        f"  {label:<10} n={reqs} batch={batch}  "
        f"p50={p['p50']:.2f}us p90={p['p90']:.2f}us p99={p['p99']:.2f}us "
        f"max={p['max']:.1f}us mean={p['mean']:.2f}us  "
        f"{rps:,.0f} req/s"
    )
    return p


def _timed_loop(fn, keys, iters: int, warmup: int) -> list[int]:
    """Call ``fn(keys)`` ``iters`` times (after ``warmup`` untimed calls); return
    per-call latencies in nanoseconds."""
    for _ in range(warmup):
        fn(keys)
    samples = [0] * iters
    perf = time.perf_counter_ns
    for i in range(iters):
        t0 = perf()
        fn(keys)
        samples[i] = perf() - t0
    return samples


def bench_shm(shm_path: str, keys: list[int], iters: int, warmup: int) -> list[int]:
    from certus_shmq_connector.ring import Ring

    r = Ring(shm_path, ready_timeout=10.0)
    print(
        f"[shm] attached: channels={r.channel_count} cap_req={r.cap_req} "
        f"cap_resp={r.cap_resp} generation={r.generation}",
        file=sys.stderr,
    )
    try:
        # Sanity: one Check must return one bool per key.
        assert len(r.check(keys)) == len(keys)
        return _timed_loop(r.check, keys, iters, warmup)
    finally:
        r.close()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--shm-path", default=os.environ.get("SHM_PATH", "/dev/shm/certus-shmq"),
                    help="certus-server mailbox file (default /dev/shm/certus-shmq)")
    ap.add_argument("--batch", type=int, default=int(os.environ.get("BATCH", 8)),
                    help="keys per Check request (default 8)")
    ap.add_argument("--iters", type=int, default=int(os.environ.get("ITERS", 50000)),
                    help="timed round-trips (default 50000)")
    ap.add_argument("--warmup", type=int, default=int(os.environ.get("WARMUP", 2000)),
                    help="untimed warmup round-trips (default 2000)")
    args = ap.parse_args()

    keys = [0xB0000000 + i for i in range(args.batch)]
    print(
        f"[bench] batch={args.batch} iters={args.iters} warmup={args.warmup} "
        f"op=Check",
        file=sys.stderr,
    )

    s = bench_shm(args.shm_path, keys, args.iters, args.warmup)
    print("[results] shared-memory ring:")
    _report("shm", s, args.batch)

    return 0


if __name__ == "__main__":
    sys.exit(main())
