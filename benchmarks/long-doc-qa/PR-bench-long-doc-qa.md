# bench(long-doc-qa): containerized long_doc_qa runner + Certus gRPC offload mode + SSD-I/O capture

## What

A self-contained `long_doc_qa` benchmark harness under `benchmarks/long-doc-qa/`
plus the tooling to measure a KV-offload backend's SSD-tier I/O while it runs.
Built up over these commits:

1. Containerize the LMCache `long_doc_qa` client.
2. Add a Llama-3 baseline runner with engine-stats capture.
3. Generalize the runner to any model (`run_llama3.sh` → `run_bench.sh`, `MODEL` param).
4. Capture all engine-stats lines + wait for the engine to settle.
5. Add a `CONNECTOR=certus` mode and bake tier-exercising defaults.
6. Add the SSD-tier I/O capture runbook.
7. Fetch `long_doc_qa.py` from a pinned LMCache commit instead of vendoring it.
8. Add `tools/certus-iostat-poll.py` (GetIoStats sampler).

## How to run

```bash
# baseline plain-vLLM (control)
./run_bench.sh
# against Certus gRPC offload
CONNECTOR=certus MODEL=Qwen/Qwen2.5-7B-Instruct ./run_bench.sh
# point at any existing OpenAI server (LMCache, CPU-offload, ...)
SERVE=0 BASE_URL=http://host:8000/v1 ./run_bench.sh
```

Defaults are the tier-exercising workload: **51 docs × 10k tokens, out=100,
4-way inflight, 2 MiB slab**. That working set (~29 GiB KV for Qwen2.5-7B)
overflows the 12 GiB DRAM tier, so evicted blocks spill to / re-fetch from SSD
(external prefix hit > 0). Smaller sets stay GPU-resident and never exercise the
tier read path.

## No vendored third-party code

`long_doc_qa.py` (LMCache, Apache-2.0) is **not** committed here. The image
`ADD`s it at build time from a pinned commit
(`ed56197172aaf22d8806328c30f3923eddfa314c`) with a `sha256` `--checksum`, so the
build is reproducible and fails if the remote bytes ever change. Bumping the
version is a one-line commit+checksum edit in the Dockerfile.

## SSD I/O capture

`tools/certus-iostat-poll.py` samples certus-server's `Dispatcher.GetIoStats`
@1Hz to CSV (device-level read/write ops+bytes+latency — no DRAM/SSD split, no
hit rate at that layer). `RUNBOOK-certus-iostat.md` documents the full capture
procedure, including that clearing the SSD tier requires a `--format` restart
(the `ClearMemoryTier`/`FlushToSsd` RPCs only touch DRAM).

Reference numbers for the 51-doc run (from the runbook):

| | warm SSD | cold SSD (`--format`) |
|---|---|---|
| SSD write | 0 GiB | 27.84 GiB (228k ops, 33 µs/op) |
| SSD read | 34.12 GiB (128 KiB/op, 438 µs/op) | 27.24 GiB (223k ops, 2068 µs/op) |
| external prefix hit | 100.0% | 49.7% |
| warmup / query round | 45.8 s / 43.3 s | 103.2 s / 43.9 s |

Cold warmup pays the full prefill + store (27.84 GiB written); the query round
then re-fetches 22.4 GiB off SSD instead of recomputing. Writes ack ~60× faster
than reads (async staging-to-SSD worker vs synchronous fetch on the gen path;
cuFile in compatible mode, not true GPUDirect).

## Scope / notes

- Bench harness + one Python diagnostic tool — no changes to Certus core crates.
- `certus-iostat-poll.py` depends only on the `GetIoStats` RPC (already on
  `unstable`) and reuses the generated stubs in `apps/python`.
- `GetIoStats` measures the SPDK NVMe block device; DRAM-tier hits are invisible
  to it, so "DRAM vs SSD share of a hit" is inferred, not directly measured.
