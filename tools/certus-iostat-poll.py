#!/usr/bin/env python3
"""Poll certus-server GetIoStats and print one CSV line per sample.

A standalone diagnostic that captures SPDK NVMe block-device I/O (the SSD/extent
tier) while any workload runs against certus-server. GetIoStats is a real shmq op
(dispatcher.read_write_stats aggregated over all data drives); it reports
device-level read/write ops+bytes+latency only -- there is no DRAM-vs-SSD split
and no cache hit rate at this layer.

Columns: iso_ts, elapsed_s, read_ops, read_bytes, read_lat_ns_sum,
         write_ops, write_bytes, write_lat_ns_sum
Header is printed first. Ctrl-C / SIGTERM to stop cleanly.

Usage:
    tools/certus-iostat-poll.py [shm_path] [interval_s]   # default /dev/shm/certus-shmq 1.0
    tools/certus-iostat-poll.py /dev/shm/certus-shmq 1.0 > iostat_samples.csv &
"""
import os
import sys
import time
import signal

# The shmq Ring client + helpers live in apps/python (repo root is this file's
# parent dir's parent, since this script sits in tools/). certus_shmq_helpers
# performs the sys.path insert that locates the certus-shmq-connector package.
# Override the helper location with CERTUS_PY_HELPERS.
_helpers = os.environ.get("CERTUS_PY_HELPERS") or os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "apps", "python"
)
sys.path.insert(0, _helpers)
from certus_shmq_helpers import DEFAULT_SHM_PATH, RingError, connect  # noqa: E402

shm_path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_SHM_PATH
interval = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0

_run = {"go": True}
signal.signal(signal.SIGTERM, lambda *a: _run.update(go=False))

ring = connect(shm_path)
print("iso_ts,elapsed_s,read_ops,read_bytes,read_lat_ns_sum,"
      "write_ops,write_bytes,write_lat_ns_sum", flush=True)
t0 = time.time()
try:
    while _run["go"]:
        try:
            r = ring.get_io_stats()
            ts = time.strftime("%Y-%m-%dT%H:%M:%S")
            print(f"{ts},{time.time()-t0:.1f},{r['read_ops']},{r['read_bytes']},"
                  f"{r['read_latency_ns_sum']},{r['write_ops']},{r['write_bytes']},"
                  f"{r['write_latency_ns_sum']}", flush=True)
        except RingError as e:
            print(f"# rpc-error {e}", flush=True)
        time.sleep(interval)
except KeyboardInterrupt:
    pass
finally:
    ring.close()
