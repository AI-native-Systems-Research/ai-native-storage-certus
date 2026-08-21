#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""microbench_shm_vs_grpc.py — control-plane round-trip latency: SHM ring vs gRPC.

The whole point of the shared-memory connector is to cut the *per-op* control
cost. gRPC's per-call overhead (protobuf encode/decode + socket syscalls +
tonic/tokio scheduling) is exactly what a prior finding showed makes CPUOffload
beat Certus-SPDK at low concurrency. This microbench isolates that cost: it
times synchronous ``Check`` round-trips of a fixed key batch over each transport
and reports the latency distribution, so the transport win is measured directly
(no vLLM, no GPU, no SSD in the path — Check touches only the in-memory index).

``Check`` is chosen because it is the smallest, purest control op: request is a
list of u64 keys, response a list of bools, and it hits neither the DMA path nor
the SSD — so the number it produces is the bare transport + dispatch cost.

Either side is optional; run whichever server(s) you have up:

  # SHM only (needs a running certus-shmq-server on --shm-path)
  python microbench_shm_vs_grpc.py --shm-path /dev/shm/certus-shmq

  # gRPC only (needs a running certus-server at --grpc-server)
  python microbench_shm_vs_grpc.py --grpc-server localhost:50051

  # both -> prints the head-to-head speedup
  python microbench_shm_vs_grpc.py --shm-path /dev/shm/certus-shmq \
      --grpc-server localhost:50051 --batch 8 --iters 50000

The keys are arbitrary (0xB0000000+i); a hit/miss doesn't change the transport
cost, which is what we are measuring. Run on the same host as the servers so the
gRPC number reflects loopback (the deployment topology), not a network hop.
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


# ── SHM ring side ──


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


# ── gRPC side ──


def bench_grpc(server: str, keys: list[int], iters: int, warmup: int) -> list[int]:
    # Imported lazily so the SHM-only path needs neither grpcio nor the gRPC
    # connector package installed.
    from certus_grpc_connector import dispatcher_pb2 as pb
    from certus_grpc_connector.client import make_stub

    channel, stub = make_stub(server)
    print(f"[grpc] channel open to {server}", file=sys.stderr)

    def check(ks):
        return stub.Check(pb.BatchCheckRequest(keys=ks)).results

    try:
        assert len(check(keys)) == len(keys)
        return _timed_loop(check, keys, iters, warmup)
    finally:
        channel.close()


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--shm-path", default=os.environ.get("SHM_PATH"),
                    help="certus-shmq-server mailbox file (enables the SHM run)")
    ap.add_argument("--grpc-server", default=os.environ.get("CERTUS_SERVER"),
                    help="certus-server host:port (enables the gRPC run)")
    ap.add_argument("--batch", type=int, default=int(os.environ.get("BATCH", 8)),
                    help="keys per Check request (default 8)")
    ap.add_argument("--iters", type=int, default=int(os.environ.get("ITERS", 50000)),
                    help="timed round-trips per transport (default 50000)")
    ap.add_argument("--warmup", type=int, default=int(os.environ.get("WARMUP", 2000)),
                    help="untimed warmup round-trips (default 2000)")
    args = ap.parse_args()

    if not args.shm_path and not args.grpc_server:
        print("error: give at least one of --shm-path / --grpc-server", file=sys.stderr)
        return 2

    keys = [0xB0000000 + i for i in range(args.batch)]
    print(
        f"[bench] batch={args.batch} iters={args.iters} warmup={args.warmup} "
        f"op=Check",
        file=sys.stderr,
    )

    shm_p = grpc_p = None
    if args.shm_path:
        s = bench_shm(args.shm_path, keys, args.iters, args.warmup)
        print("[results] shared-memory ring:")
        shm_p = _report("shm", s, args.batch)
    if args.grpc_server:
        g = bench_grpc(args.grpc_server, keys, args.iters, args.warmup)
        print("[results] gRPC:")
        grpc_p = _report("grpc", g, args.batch)

    if shm_p and grpc_p:
        print("[compare] gRPC / SHM latency ratio (higher = SHM faster):")
        for k in ("p50", "p90", "p99", "mean"):
            ratio = grpc_p[k] / shm_p[k] if shm_p[k] else float("inf")
            print(f"    {k}: {ratio:.1f}x  (gRPC {grpc_p[k]:.2f}us vs SHM {shm_p[k]:.2f}us)")

    return 0


if __name__ == "__main__":
    sys.exit(main())
