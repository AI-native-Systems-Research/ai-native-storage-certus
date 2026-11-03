# lmcache-connector-bench

Benchmarks for the **LMCache** KV-offload backend in multi-process (MP) mode: an `lmcache server` holding a pinned-RAM L1, optionally written through to an L2 such as `fs_native` on NVMe. This is the backend `deploy/lmcache` and `benchmarks/vllm-serve/run-serve-lmcache*.sh` serve. The bench is the counterpart of [`certus-connector-bench`](../certus-connector-bench/README.md) and [`cputier-connector-bench`](../cputier-connector-bench/README.md). It has the same modes, the same report format and the same CSV row labels, so the backends can be compared side by side without running vLLM.

It drives the two adapters vLLM's `LMCacheMPConnector` builds:

- `LMCacheMPSchedulerAdapter` (scheduler side): `maybe_submit_lookup_request`, `check_lookup_result`, `cleanup_lookup_result`, `end_session`
- `LMCacheMPWorkerAdapter` (worker side): `register_kv_caches`, `batched_submit_store_requests`, `batched_submit_retrieve_requests`, `get_finished`, `get_block_ids_with_load_errors`

No engine, model or HTTP front end is started. The bench allocates a fake GPU KV cache in vLLM 0.30's layout (one fp16 tensor per layer, `[blocks, block_size, heads, 2·head_dim]`, K and V fused) and registers it with the server, which maps it over CUDA IPC exactly as it maps vLLM's. It then plays the connector: one request per batch, a lookup polled until the server answers (including any L2 prefetch), a retrieve of the hit prefix, and `end_session` when the request finishes.

## Differences from the sibling benches

| | certus-connector-bench | cputier-connector-bench | lmcache-connector-bench |
|---|---|---|---|
| Backend | external `certus-server`, `/dev/shm` ring | in-process vLLM `TieringOffloadingSpec` | external `lmcache server` over ZMQ |
| DRAM tier | `--memory-tier-size` | `--cpu-bytes` | L1, `L1_GB` (server `--l1-size-gb`) |
| Disk tier | raw NVMe via SPDK | `fs`: one file per block, `O_DIRECT` | L2 `fs_native`: one file per chunk, `O_DIRECT` |
| GPU path | CUDA IPC → server DMA | `swap_blocks` into a pinned mmap | CUDA IPC → server copy kernels |
| Runs | on the host | in a vLLM image | in the vLLM + LMCache image, server started by the wrapper |
| Key | one per block | one per block | one per **chunk** (`--chunk-size` tokens), prefix-chained |
| "Cold" | `flush_to_ssd` + `clear_memory_tier` | `reset_cache()` | wait for L2 write-through, then `POST /cache/clear` (L1 only) |
| Lookup | per key | per key, `RETRY`/`HIT_PENDING` across steps | one round trip per request for the whole prefix, polled |
| No `touch` | | | LMCache has no touch; `--lookup-order` is accepted and ignored |

There's no counterpart to `bench_connector_path.py`, `bench_connector_race.py` or the `contention` and `--pattern` modes.

Things to know when reading LMCache numbers:

- **A key is a chunk.** `--block-bytes` (default 2 MiB) is one LMCache chunk across all layers, covering the server's `--chunk-size` tokens (256). `--bs` chunks form one request, whose token sequence is `--bs × 256` tokens. The fake KV cache's head count and head dim are derived so that one chunk is exactly `--block-bytes`. Match `--block-bytes` and `--bs` to the sibling runs; `CHUNK_SIZE=64` with `--block-bytes 512K` gives the same per-token geometry as a 64-token offload block.
- **Keys are prefix hashes.** Chunk *i*'s key covers every token before it, so a batch is a prefix, not `--bs` independent keys. Each store pass uses fresh token sequences, because LMCache skips chunks it already holds.
- **Store throughput** excludes the per-pass `reset`, which waits for that pass's L2 write-through. The headline figure is GPU → L1. The `Sustained incl. L2 write-through drain` line below it is the disk-bound rate.
- **L1 eviction is not inline.** A server thread wakes once a second and, above 80% full, evicts 20% of L1. A store into a full L1 doesn't wait for it: the server drops the store, and the request still completes. The mixed-eviction phase counts those as **store drops**, read from the server's `lmcache_mp_l1_write_chunks_total`. At the bench's store rate they dominate once L1 fills, which is a real LMCache behaviour, not a bench artifact.
- **`lookup_wait`** is the polling after the first `check_lookup_result`: the server's ack plus, on a cold lookup, the L2 → L1 prefetch. Even a warm lookup costs about 1.7 ms here, because the scheduler adapter polls without blocking.
- **Single rank only.** `--tp 1`; LMCache's TP sharding is not simulated.

## Running

Prerequisites:

- The vLLM + LMCache image: `deploy/lmcache/build.sh` builds `localhost/vllm-lmcache-bench:vllm0.30.0` in about a minute.
- A CUDA GPU visible to podman through the NVIDIA CDI device. The bench and server share it; the bench's fake KV cache is about 0.5 GiB at the defaults (272 chunks × `--block-bytes`), and grows with `--bs`, `--pipeline-depth` and `--requests-per-step`.

From the repo root:

```bash
B=benchmarks/lmcache-connector-bench/run-in-container.sh
$B                                          # default: store, warm load, cold load, mixed eviction
$B --verify                                 # data round trip (warm, and cold through L2) first
$B --mode all --csv bench-results/lmcache.csv --tag lmcache
L1_GB=8 L2_DIR=/mnt/nvme/lmcache-bench-l2 $B --working-set 2048
L2=0 $B                                     # L1 only, no cold phase
```

The wrapper starts a private `lmcache server` container on side ports (ZMQ 5575, HTTP 8095), so it doesn't disturb a `deploy/lmcache` stack on 5555/8080. It waits for `/status`, runs the bench in a second container, and removes the server and its L2 run directory at exit (`KEEP_L2=1` keeps the files). All arguments go to the bench. Paths such as `--csv` resolve inside the container, where the repo is the working directory.

Both containers use `--ipc host --network host --pid host`. On this host the server's CUDA IPC import of the bench's tensors fails with `CUDA error: invalid device context` unless both sides share the host PID namespace. vLLM itself doesn't hit this in `deploy/lmcache`, but a bare two-container test reproduces it.

### Wrapper environment

| Variable | Default | Description |
|---|---|---|
| `IMAGE` | `localhost/vllm-lmcache-bench:vllm0.30.0` | vLLM + LMCache image |
| `GPU` | `0` | CDI GPU index |
| `L1_GB` | `4` | Server L1 (pinned RAM). Match cputier's `--cpu-bytes` / certus's `--memory-tier-size` |
| `L2` | `1` | `1` = `fs_native` L2 on `L2_DIR`; `0` = none |
| `L2_DIR` / `L2_GB` | `$HOME/lmcache-bench-l2` / `64` | L2 host directory and capacity. Put it on the disk you want to measure, not on overlay storage (no `O_DIRECT`) |
| `USE_ODIRECT` | `1` | `O_DIRECT` L2 I/O |
| `L2_THREADS` | `8` | `fs_native` I/O workers |
| `CHUNK_SIZE` | `256` | Server `--chunk-size` in tokens |
| `LMC_WORKERS` | `1` | Server `--max-workers` |
| `ZMQ_PORT` / `HTTP_PORT` | `5575` / `8095` | Server ports |

### Bench parameters

| Parameter | Default | Description |
|---|---|---|
| `--layers` | 32 | Per-layer KV tensors |
| `--tokens-per-block` | 16 | vLLM GPU block size; the chunk is `--chunk-size / this` blocks |
| `--kv-heads` | derived | KV heads per layer tensor |
| `--verify` | — | Store random GPU data, zero it, retrieve it (warm, then cold through L2) and compare bytes |
| `--mq-timeout` | 30 | Seconds the adapters wait for blocking server calls |

The workload flags (`--bs`, `--num-blocks`, `--block-bytes`, `--min-duration`, `--warmup`, `--working-set`, `--mode`, `--pipeline-depth`, `--direction`, `--hit-ratio`, `--requests-per-step`, `--num-sessions`, `--csv`, `--tag`, `--no-cold`, `--no-eviction`) mean the same as in the sibling benches.

## Modes

- **default**: four serial phases.
  1. Store (GPU → L1, written through to L2 in the background).
  2. Warm load (L1 → GPU).
  3. Cold load. L1 is cleared before each pass (after its L2 write-through drains), so every chunk is prefetched L2 → L1 (`lookup_wait`) and then copied to the GPU.
  4. Mixed store and load under L1 pressure. Size `--working-set` against `L1_GB`; the run prints after how many generations L1 fills.
- **pipelined**: `--pipeline-depth` requests submitted before reaping. For loads, the lookup is part of `prepare`.
- **prefix-miss**: the first `--hit-ratio` of the keys are stored, then every request is looked up and its hit prefix retrieved. LMCache answers a request's whole prefix in one round trip, so `lookup_hit`/`lookup_miss` are per request (split by whether any of it hit), not per key.
- **scheduler-step**: sessions share one stored prefix pool and diverge at geometric points; each request is a session prefix plus a fresh suffix. Per step: every request's lookup is submitted, then all are polled until answered (`touch_lookup`), the hit prefixes are retrieved in one batched call and drained (`drain_workers`), and the unstored tails are submitted as stores and drained, untimed, at the start of the next step.

## Side-by-side with cputier and certus

Run the benches with matching sizes and join the CSVs on `label`. Use separate files: appending to one CSV keeps only the first writer's header columns.

```bash
benchmarks/cputier-connector-bench/run-in-container.sh --cpu-bytes 4G --mode all \
    --csv bench-results/connector-cputier.csv --tag cputier
L1_GB=4 benchmarks/lmcache-connector-bench/run-in-container.sh --mode all \
    --csv bench-results/connector-lmcache.csv --tag lmcache
```

A 3-second `--mode all` run on this host (one A30, `L1_GB=4`, L2 on the `/cache` NVMe, default sizes) gave:

| Phase | GB/s |
|---|---|
| Store (GPU → L1) | 5.6 (1.9 sustained incl. L2) |
| Warm load | 5.4 |
| Cold load (L2 → L1 → GPU) | 2.0 |
| Mixed under eviction | 4.5 (3792 store drops) |
| Pipelined store / load (depth 4) | 7.7 / 7.5 |
| Prefix-miss (75%) | 4.9 |
| Scheduler step | 4.9 (19.9 steps/s) |
