# T056 — two nodes across the RoCE fabric

**Run**: 2026-10-20, `loopback_test::two_node`, release build, `--features rdma`, the
working tree of Phase 9 on top of `98b0e44b`.

| | node2 (destination) | node5 (source) |
|---|---|---|
| address | `10.0.0.102:47100` | `10.0.0.105:47100` |
| NIC | `mlx5_0`, 200 Gb/s (4X HDR), RoCE | `mlx5_0`, 200 Gb/s (4X HDR), RoCE |
| pinning | `numactl -N0 -m0` (the NIC's node) | `numactl -N0 -m0` (the NIC's node) |

node5's certus and vLLM pods were left running: the test uses its own port and a
1536 × 64 KiB pool of ordinary memory, so nothing was stopped.

The source serves 1,536 entries of 64 KiB (96 MiB), each filled with a pattern derived
from its key. The destination fetches all 1,536 in one `fetch` — five chunks of the
peer's 340 — five times, checking every 509th byte of every entry against the pattern.

```
T056 source 10.0.0.105:47100: serving 1536 x 65536 B for 40 s
T056 round 0: 1536 entries, 96.0 MiB in 12.3 ms = 8.18 GB/s
T056 round 1: 1536 entries, 96.0 MiB in 5.3 ms = 18.99 GB/s
T056 round 2: 1536 entries, 96.0 MiB in 5.3 ms = 19.06 GB/s
T056 round 3: 1536 entries, 96.0 MiB in 5.3 ms = 19.12 GB/s
T056 round 4: 1536 entries, 96.0 MiB in 5.3 ms = 19.07 GB/s
T056 dest done: chunks_sent 25 requests 5/5 deadline_expiries 0
T056 source done: hits 0 misses 0 failed 0 protocol_faults 0 pins held 0
```

* **Correct**: every entry `HitMemoryTier`, every sampled byte matched, 25 chunk
  messages for 5 fetches, no deadline, no protocol fault, no pin left held.
* **~19.1 GB/s steady** — 76% of the 25 GB/s line rate, at 64 KiB entries, one
  connection, the default 8 send credits. Round 0 includes connection setup.
* **Not a benchmark.** One run, one entry size, default credits, a sampled check
  (the full-byte check is the loopback test's). T057+ measure.
* **The source's counters read zero hits.** `hit_*` counts outcomes of requests *this
  end made*; serving a peer's fetch is not counted on the source. That is consistent
  with the counters' definitions, but a source has no counter of what it served.
