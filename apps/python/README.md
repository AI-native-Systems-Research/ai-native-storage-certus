# Certus Python Integration Tests

These scripts exercise `certus-server` over its `/dev/shm` mailbox (shmq),
using the pure-Python `Ring` client in
`../../certus-shmq-connector/certus_shmq_connector/ring.py`, located at runtime
by `certus_shmq_helpers.py` (a `sys.path` insert, not a pip install).

The throughput/latency benchmarks that used to live here have moved to
[`benchmarks/python`](../../benchmarks/python/README.md), which carries its
own copy of `certus_shmq_helpers.py` and `requirements.txt`.

```bash
pip install -r requirements.txt   # no third-party deps for shmq
pip install torch                 # only for scripts that allocate GPU buffers
python3 test-promote.py
```
