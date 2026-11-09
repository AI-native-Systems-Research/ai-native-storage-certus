# deploy/lmcache — vLLM 0.30 + LMCache, CPU tier spilling to SSD

This directory stands up [LMCache](https://github.com/LMCache/LMCache) as a KV-offload tier for vLLM 0.30.0 on this host. LMCache keeps an L1 cache in pinned host RAM and writes it through to an L2 directory on the local NVMe. A prefix that has dropped out of the GPU prefix cache and out of L1 is read back from SSD. It is the LMCache counterpart of `deploy/3fs` and of the cputier (CPU + fs tier) and Certus-SHMQ arms in `benchmarks/vllm-serve`.

```
 host (--network host, --ipc host, both containers rootless)
 ┌ certus-lmcache-server ───────────────┐  ZMQ 127.0.0.1:5555   ┌ certus-lmcache-vllm ─────────────┐
 │ lmcache server (MP mode)             │◀─────────────────────▶│ vllm serve … :8000               │
 │  L1  pinned RAM   CPU_GB   LRU       │  KV by CUDA IPC       │  LMCacheMPConnector              │
 │  L2  fs_native    DISK_GB  O_DIRECT ─┼──▶ /cache/lmcache/l2  │  GPU prefix cache (HBM)          │
 │  HTTP /status /metrics  :8080        │     (nvme0n1, XFS)    └──────────────────────────────────┘
 └──────────────────────────────────────┘
```

## Files

| File | Purpose |
|---|---|
| `Containerfile` | A thin layer on `vllm/vllm-openai:v0.30.0`, which already bundles LMCache 0.5.5 built for its torch 2.13 / CUDA 13. Adds import checks and a role-dispatching entrypoint |
| `build.sh` | Builds `localhost/vllm-lmcache-bench:vllm0.30.0` in the default rootless store (about a minute; nothing compiles) |
| `entrypoint.sh` | `lmcache-server …` checks the L2 dir (writable, O_DIRECT, free space), then runs `lmcache server`. Anything else runs `vllm serve` |
| `run.sh` | Starts the server, waits for `/status`, then starts vLLM and waits for `/health`. `FOREGROUND=1` attaches to vLLM, and Ctrl-C stops both |
| `smoke.sh` | Overflows L1, drops the GPU prefix cache, replays the oldest prompt, and checks that its KV came back from SSD unchanged |
| `teardown.sh` | Stops both containers. `--wipe` also deletes the L2 files |

## Use

```bash
cd deploy/lmcache
./build.sh
CPU_GB=2 ./run.sh        # small L1 so the smoke test reaches the SSD quickly
./smoke.sh
./teardown.sh

./run.sh                 # benchmark defaults: 8G L1, 200G L2
```

A passing smoke run on this host (one A30, Qwen2.5-7B, 2 GiB L1):

```
[smoke] 2/4 store: 12 × 8000 tokens in 20.8s -> L2 372 files, 5.09 GiB; L1 76% of 2 GiB
[smoke] 4/4 replay prompt 0: 7936/8000 tokens from LMCache (31 chunks from L2), 434 MiB read from nvme0n1, latency 1.64s cold -> 0.53s warm
[smoke] PASS
```

## Knobs (`run.sh` environment)

| Var | Default | Notes |
|---|---|---|
| `CPU_GB` | `8` | L1 size (pinned host RAM, LRU). Evicted chunks remain on L2 |
| `DISK_DIR` / `DISK_GB` | `/cache/lmcache` / `200` | L2 lives in `$DISK_DIR/l2` and is **cleared on every start**, because a fresh server has an empty index. `KEEP_DISK=1` keeps the files |
| `USE_ODIRECT` | `1` | O_DIRECT L2 I/O, so reads hit the SSD and not the page cache. `0` gives buffered I/O |
| `DISK_IO_THREADS` | `8` | `fs_native` C++ I/O workers |
| `CHUNK_SIZE` | `256` | Tokens per chunk, which is also one L2 file. Qwen2.5-7B: 14 MiB per file |
| `LMC_WORKERS` | `1` | Server STORE/RETRIEVE and LOOKUP threads |
| `MODEL`, `SERVED_MODEL_NAME`, `MAX_MODEL_LEN`, `GPU`, `TENSOR_PARALLEL`, `GPU_MEM_UTIL`, `KV_CACHE_BYTES` | Qwen2.5-7B, 32768, `0`, 1, 0.90, — | Same meaning as in `benchmarks/vllm-serve/run-serve-*.sh`. A window above 32768 on Qwen2.x turns on YaRN |
| `PORT` / `LMC_PORT` / `LMC_HTTP_PORT` | `8000` / `5555` / `8080` | All are on the host network |
| `EXTRA_SERVE_ARGS` / `EXTRA_LMCACHE_ARGS` | — | Extra flags passed through to `vllm serve` and `lmcache server` |

## Observing it

- **vLLM** `:8000/metrics`. `vllm:external_prefix_cache_hits_total` counts tokens served by LMCache.
- **LMCache** `:8080/metrics`. `lmcache_mp_l1_*`, `lmcache_mp_l2_store_*`, and `lmcache_mp_l2_prefetch_hit_chunks_total` (chunks read back from SSD). `:8080/status` reports L1 usage as JSON.
- SSD traffic: `iostat -x nvme0n1 1`. With O_DIRECT, every L2 read appears there.

## Design notes and caveats for the comparison

- **MP mode, not the in-process connector.** vLLM 0.30 lays KV out with K and V fused per layer as `[blocks, block_size, heads, 2·head_dim]` (logged as "Using LBNHC KV cache layout"). With LMCache 0.5.5 (bundled) and 0.5.6 (latest stable), the in-process `LMCacheConnectorV1` (`LMCACHE_LOCAL_CPU` / `LMCACHE_LOCAL_DISK`) mis-gathers this layout. Every chunk retrieved from LMCache is wrong and the output turns into garbage, with no error. This is [LMCache#6156](https://github.com/LMCache/LMCache/issues/6156), open as of 2026-10-29. Measured here on single-chunk prompts: the HND layout, FlashInfer, and `--enforce-eager` all corrupt. vLLM's `OffloadingConnector` and `LMCacheMPConnector` are exact. MP mode maps the GPU KV directly by CUDA IPC and doesn't go through the faulty gather. `smoke.sh` compares outputs token for token, so it would catch a regression.
- **Write-through, not spill-on-evict.** Every chunk stored to L1 is also queued to L2, so SSD write bandwidth is spent on all new prefixes, not only on evicted ones. Reads go to L2 only on an L1 miss: the server prefetches the chunks into L1, then copies them to the GPU. The cputier fs tier and Certus behave differently, so name the policy when you compare results.
- **The L2 index is in memory.** A restarted server can't see the files a previous run left behind, which is why `run.sh` clears `$DISK_DIR/l2`.
- **`--security-opt label=disable`** is required on this host. With SELinux enforcing, the CDI GPU device nodes fail inside the container ("Failed to initialize NVML: Insufficient Permissions"). Both containers need the GPU, because the server copies to and from vLLM's KV over CUDA IPC.
- **Noise you can ignore in the vLLM log:** podman's `Error descending into path /dev/shm/slapd-…` (from `--ipc host`) and Triton's "0 active drivers" at build time.
