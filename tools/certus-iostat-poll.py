#!/usr/bin/env python3
"""Poll certus-server Dispatcher.GetIoStats and print one CSV line per sample.

A standalone diagnostic that captures SPDK NVMe block-device I/O (the SSD/extent
tier) while any workload runs against certus-server. GetIoStats is a real RPC
(apps/certus-server service.rs -> dispatcher.read_write_stats aggregated over all
data drives); it reports device-level read/write ops+bytes+latency only -- there
is no DRAM-vs-SSD split and no cache hit rate at this layer.

Columns: iso_ts, elapsed_s, read_ops, read_bytes, read_lat_ns_sum,
         write_ops, write_bytes, write_lat_ns_sum
Header is printed first. Ctrl-C / SIGTERM to stop cleanly.

Usage:
    tools/certus-iostat-poll.py [target] [interval_s]   # default localhost:50051 1.0
    tools/certus-iostat-poll.py localhost:50051 1.0 > iostat_samples.csv &
"""
import os
import sys
import time
import signal

import grpc

# The generated gRPC stubs live in apps/python (repo root is this file's parent
# dir's parent, since this script sits in tools/). Override with CERTUS_PY_STUBS.
_stubs = os.environ.get("CERTUS_PY_STUBS") or os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "apps", "python"
)
sys.path.insert(0, _stubs)
import dispatcher_pb2 as pb          # noqa: E402
import dispatcher_pb2_grpc as pbg    # noqa: E402

target = sys.argv[1] if len(sys.argv) > 1 else "localhost:50051"
interval = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0

_run = {"go": True}
signal.signal(signal.SIGTERM, lambda *a: _run.update(go=False))

ch = grpc.insecure_channel(target)
stub = pbg.DispatcherStub(ch)
print("iso_ts,elapsed_s,read_ops,read_bytes,read_lat_ns_sum,"
      "write_ops,write_bytes,write_lat_ns_sum", flush=True)
t0 = time.time()
try:
    while _run["go"]:
        try:
            r = stub.GetIoStats(pb.GetIoStatsRequest(), timeout=3)
            ts = time.strftime("%Y-%m-%dT%H:%M:%S")
            print(f"{ts},{time.time()-t0:.1f},{r.read_ops},{r.read_bytes},"
                  f"{r.read_latency_ns_sum},{r.write_ops},{r.write_bytes},"
                  f"{r.write_latency_ns_sum}", flush=True)
        except grpc.RpcError as e:
            print(f"# rpc-error {e.code()}", flush=True)
        time.sleep(interval)
except KeyboardInterrupt:
    pass
