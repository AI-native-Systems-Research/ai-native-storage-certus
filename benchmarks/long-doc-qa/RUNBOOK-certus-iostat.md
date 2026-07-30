# Runbook: long_doc_qa over Certus gRPC offload, with SSD I/O captured

Reproduces the 51-doc tier-exercising run and captures SPDK NVMe (SSD/extent
tier) I/O via `Dispatcher.GetIoStats`. `run_bench.sh` drives the workload;
`tools/certus-iostat-poll.py` samples the device counters in parallel.

Outputs land under `results/certus-51docs-iostat/` (warm SSD) or
`results/certus-51docs-iostat-cold/` (freshly `--format`ed SSD); expected
numbers are tabulated at the bottom.

## Why 51 docs

51 docs x 10000 tokens (~510k tokens, ~29 GiB KV for Qwen2.5-7B) overflows the
12 GiB DRAM tier, so evicted blocks spill to / are re-fetched from the SSD tier.
Smaller working sets stay GPU-resident and Certus is written but never read
(external prefix hit stays 0). These are already the baked defaults in
`run_bench.sh` (NUM_DOCUMENTS=51, OUTPUT_LEN=100, MAX_INFLIGHT_REQUESTS=4,
SLAB_SIZE_BYTES=2 MiB).

## 1. Launch certus-server

Warm run: reuse a populated tier. Cold run: add `--format` to wipe the extent
store (RPCs `ClearMemoryTier`/`FlushToSsd` only touch DRAM — you MUST `--format`
to truly clear the SSD, else dedup makes stores a no-op and writes read as 0).

```bash
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64 \
  target/release/certus-server \
  --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 \
  --device-pci 0000:63:00.0 --device-pci 0000:64:00.0 \
  --memory-tier-size 12G --listen 0.0.0.0:50051 --format   # drop --format for a warm run
```

## 2. Start the I/O sampler (before the workload)

```bash
python3 tools/certus-iostat-poll.py localhost:50051 1.0 \
  > results/certus-51docs-iostat/iostat_samples.csv &
IOSTAT_PID=$!
```

Deltas are last-minus-first sample; the per-phase throughput in the summaries is
computed over the active read/write window of each round.

## 3. Run the workload

```bash
cd benchmarks/long-doc-qa
CONNECTOR=certus MODEL=Qwen/Qwen2.5-7B-Instruct ./run_bench.sh
```

`CONNECTOR=certus` uses the `certus-grpc-bench` image, resets its ENTRYPOINT to
`vllm serve`, and adds `--ipc=host` + a `--kv-transfer-config`
(OffloadingConnector / kv_both / CertusGrpcOffloadingSpec, server localhost:50051
via `--network host`, slab_size_bytes=2097152) + `--enforce-eager`.

## 4. Stop the sampler and summarize

```bash
kill -TERM "$IOSTAT_PID"
```

Whole-run I/O = last-minus-first row of the CSV. External (Certus) prefix hit
rate comes from the client's `metrics.txt`
(`vllm:external_prefix_cache_{hits,queries}_total`); GPU prefix hit from
`vllm:prefix_cache_*`. Round wall-times / TTFT are in the client stdout captured
under `results/.../{warmup,query}_round.csv` and `engine_stats.txt`.

## Expected (reference)

| | warm SSD | cold SSD (`--format`) |
|---|---|---|
| SSD write | 0 GiB | 27.84 GiB (228k ops, 33 µs/op) |
| SSD read | 34.12 GiB (128 KiB/op, 438 µs/op) | 27.24 GiB (223k ops, 2068 µs/op) |
| external prefix hit | 100.0% | 49.7% (warmup miss/store, query hit/fetch) |
| warmup round | 45.8 s | 103.2 s |
| query round | 43.3 s | 43.9 s |

Note the read/write latency asymmetry (~60x): writes ack fast via the async
staging-to-SSD worker (off the critical path); reads are synchronous on the
KV-fetch/generation path. cuFile runs in compatible mode (not true GPUDirect).
