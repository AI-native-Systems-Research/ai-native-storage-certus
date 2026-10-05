# cputier-connector-bench

Benchmarks for the **cputier** KV-offload backend: vLLM's in-tree
`OffloadingConnector` with `TieringOffloadingSpec`, which pairs a pinned host-RAM CPU
primary tier with an optional `fs` disk secondary tier. This is the backend
`apps/vllm-serve/run-serve-cputier.sh` serves. The benchmark is the counterpart of
[`certus-connector-bench`](../certus-connector-bench/README.md). It has the same modes,
the same report format and the same CSV columns, so the two backends can be compared
side by side without running vLLM.

It drives the real vLLM classes directly:

- `TieringOffloadingManager` (scheduler side): `on_new_request`, `touch`, `lookup`, `prepare_store`/`complete_store`, `prepare_load`/`complete_load`, `take_events`, `on_schedule_end`, `on_request_finished`
- `CPUOffloadingWorker` (worker side): `submit_store`, `submit_load`, `get_finished`. It copies GPU↔pinned `/dev/shm` mmap with `swap_blocks_batch`, one CUDA stream per transfer.
- `FileSystemTierManager`: CPU→disk write-through cascade, async existence probes, and disk→CPU promotion through its read/write thread pool

No engine, model or HTTP server is started. The benchmark builds the `OffloadingConfig` by hand and allocates a fake per-layer GPU KV cache (`--layers` int8 tensors). It then **plays the vLLM scheduler**: one `ReqContext` per request, `on_schedule_end` after each step, and lookups that return `RETRY`/`HIT_PENDING` are retried on the next step, as the connector does. That retry time is reported as its own phase, `lookup_wait`.

## Differences from certus-connector-bench

| | certus-connector-bench | cputier-connector-bench |
|---|---|---|
| Backend | external `certus-server` over `/dev/shm/certus-shmq` | in-process; nothing to start |
| DRAM tier | certus memory tier (`--memory-tier-size`) | CPU primary tier (`--cpu-bytes`) |
| Disk tier | raw NVMe via SPDK | `fs` tier: one file per block under `--fs-root`, `O_DIRECT` |
| GPU path | CUDA IPC handle → server DMA | `cudaHostRegister`'d mmap + `swap_blocks_batch` |
| Runs | on the host | in a vLLM ≥ 0.30 container (`run-in-container.sh`) |
| "Cold" | `flush_to_ssd` + `clear_memory_tier` | `reset_cache()`: drain fs writes, empty the CPU tier (fs data kept) |
| Remove keys between passes | `ring.remove` | not possible; each store pass uses a **fresh key set** instead |
| Extra phases | — | `new_request`, `lookup_wait`, `schedule_end`, `reset`/`demote` |

There is no counterpart to `bench_connector_path.py` or `bench_connector_race.py`. Those scripts exercise the shmq ring, and cputier has no ring.

Things to know when reading cputier numbers:

- **The fs tier never rewrites an existing block.** Its compiled writer skips a path that already exists. So the store benchmarks re-key every pass; otherwise disk writes would stop after the first pass.
- **Store throughput** excludes the per-pass `reset`, which waits for that pass's fs write-through. The headline figure is the GPU→CPU path, which compares with Certus's DRAM store. The `Sustained incl. fs write-through drain` line below it is the disk-bound rate.
- **A cold lookup takes scheduler steps.** A CPU-tier miss first returns `RETRY` until the async fs probe is flushed. A disk hit then returns `HIT_PENDING` until the promotion lands in the CPU tier. All of this is `lookup_wait`. With an fs tier, even a true miss costs one step (see `prefix-miss`).
- **CPU-tier space is pinned by write-through.** A stored chunk can't be evicted until its fs write finishes. If the disk falls behind, `prepare_store` returns `None`, which is reported as **store drops**.
- **The worker submits one copy descriptor per (block, layer).** `submit_*` cost grows with `--layers`; try `--layers 1` versus the default of 32.
- `blocks` and `iops` count offload **chunks**. With `--blocks-per-chunk N`, each chunk is N GPU blocks (`N × --block-bytes`).
- Single rank only: TP is not simulated (certus-connector-bench's `--tp` has no counterpart).

## Running

Prerequisites:

- A CUDA GPU visible to podman through the NVIDIA CDI device (`--device nvidia.com/gpu=N`).
- A vLLM ≥ 0.30 image: `podman pull docker.io/vllm/vllm-openai:v0.30.0`.

vLLM isn't installed on the host, so the bench runs inside the stock vLLM image with the repo bind-mounted at `/certus`. Nothing needs to be built. There are three ways to run it.

### 1. Wrapper script (easiest)

From the repo root:

```bash
tools/cputier-connector-bench/run-in-container.sh                # default 4 phases
tools/cputier-connector-bench/run-in-container.sh --verify       # data round-trip check first
tools/cputier-connector-bench/run-in-container.sh --mode all --csv bench-results/cputier.csv --tag cputier
tools/cputier-connector-bench/run-in-container.sh --spec cpu     # CPU tier only, no disk tier

# Different GPU, disk-tier location and CPU tier size:
GPU=1 FS_TIER_HOST=/path/on/fast/disk CPU_BYTES=13G \
  tools/cputier-connector-bench/run-in-container.sh --cpu-bytes 13G --min-duration 10
```

All arguments are passed through to the bench. If you change `--cpu-bytes`, set `CPU_BYTES` to the same value so the container's `/dev/shm` is big enough.

### 2. podman directly

This is the command the wrapper runs:

```bash
cd ~/certus
mkdir -p ~/cputier-bench-fs-tier

podman run --rm --pull=never \
  --security-opt label=disable \
  --shm-size 8g \
  --device nvidia.com/gpu=0 \
  -v "$(pwd -P):/certus" \
  -v ~/cputier-bench-fs-tier:/fs-tier \
  -w /certus \
  --entrypoint python3 \
  docker.io/vllm/vllm-openai:v0.30.0 \
  tools/cputier-connector-bench/bench_cputier_lifecycle.py --fs-root /fs-tier --verify
```

- `--security-opt label=disable` lets the container read the bind mounts under SELinux without relabelling the repo.
- `--shm-size` must be at least `--cpu-bytes` plus some headroom; the default `--cpu-bytes` is 4G.
- Leave out `--fs-root` and the `/fs-tier` mount if you use `--spec cpu`.

### 3. Interactively inside the container

Useful for iterating on the script or poking at vLLM:

```bash
podman run --rm -it --pull=never --security-opt label=disable --shm-size 8g \
  --device nvidia.com/gpu=0 -v "$(pwd -P):/certus" -v ~/cputier-bench-fs-tier:/fs-tier \
  -w /certus --entrypoint bash docker.io/vllm/vllm-openai:v0.30.0

# then, inside the container:
python3 tools/cputier-connector-bench/bench_cputier_lifecycle.py --fs-root /fs-tier --help
python3 tools/cputier-connector-bench/bench_cputier_lifecycle.py --fs-root /fs-tier --mode pipelined
```

Edits you make to the script on the host show up in the container straight away, because the repo is mounted.

### Without a container

If you have vLLM ≥ 0.30 in a Python environment, run the script directly:

```bash
python tools/cputier-connector-bench/bench_cputier_lifecycle.py --fs-root /path/on/nvme
```

### Wrapper environment

| Variable | Default | Description |
|---|---|---|
| `IMAGE` | `docker.io/vllm/vllm-openai:v0.30.0` | vLLM ≥ 0.30. The 0.26 `certus-offload-fix026` image has an older `kv_offload` API and is not supported |
| `GPU` | `0` | CDI GPU index (`--device nvidia.com/gpu=$GPU`) |
| `FS_TIER_HOST` | `$HOME/cputier-bench-fs-tier` | Host directory for the fs tier, mounted at `/fs-tier`. Put it on the filesystem you want to measure, and not on container overlay storage (no `O_DIRECT`) |
| `CPU_BYTES` | `4G` | Sizes the container `/dev/shm`. Pass the same value as `--cpu-bytes` |
| `SHM_SIZE` | `CPU_BYTES + 4G` | Explicit `/dev/shm` size |

### Notes

- **Disk tier:** `--fs-root` (or `FS_TIER_HOST` with the wrapper) decides which disk the cold-load and write-through numbers measure. The bench writes block files into a per-run subdirectory and deletes it at exit; `--keep-fs` keeps them. Mixed-eviction, contention and scheduler-step modes store new blocks continuously, so expect several GB of disk writes per mode.
- **Output paths:** paths such as `--csv` are resolved inside the container, where the repo is the working directory (`/certus`). So `--csv bench-results/cputier.csv` lands in the repo's `bench-results/`.

### Backend parameters

These play the role of certus-connector-bench's server flags:

| Parameter | Default | Description |
|---|---|---|
| `--spec` | `tiering` | `tiering` = `TieringOffloadingSpec` (cputier); `cpu` = `CPUOffloadingSpec` (host RAM only, no cold phase) |
| `--cpu-bytes` | `4G` | CPU primary tier (`cpu_bytes_to_use`) |
| `--fs-root` | `$TMPDIR` | fs tier root (the wrapper passes `/fs-tier`) |
| `--no-fs` | — | Tiering spec with no secondary tier |
| `--fs-read-threads` / `--fs-write-threads` | 16 / 16 | fs tier I/O thread pool |
| `--eviction-policy` | `lru` | CPU tier policy (`lru`, `arc`) |
| `--layers` | 32 | Per-layer KV tensors per block (copy descriptors per block) |
| `--blocks-per-chunk` | 1 | GPU blocks per offloaded chunk |
| `--no-kv-events` | — | Disable KV cache events (`take_events` yields nothing) |
| `--verify` | — | Store random GPU data, zero it, load it back (warm, and cold through the fs tier), and compare bytes |

The workload flags (`--bs`, `--num-blocks`, `--block-bytes`, `--min-duration`, `--working-set`, `--mode`, `--pipeline-depth`, `--direction`, `--hit-ratio`, `--requests-per-step`, `--num-sessions`, `--pattern`, `--override`, `--csv`, `--tag`, `--no-cold`, `--no-eviction`) mean the same as in certus-connector-bench.

## Modes

```bash
B=tools/cputier-connector-bench/run-in-container.sh
$B                                         # default: store, warm load, cold load, mixed eviction
$B --mode pipelined --pipeline-depth 4     # N batches in flight before reaping
$B --mode prefix-miss --hit-ratio 0.5      # maximal-prefix lookup cost
$B --mode contention                       # store thread + load thread on one CPU tier
$B --mode scheduler-step                   # multi-request steps with shared prefixes
$B --mode all --csv bench-results/cputier.csv --tag cputier
$B --pattern cold_prefill_store            # certus_fio YAML pattern
$B --spec cpu                              # CPU-only offload baseline
```

- **default**: four serial phases.
  1. Store (GPU→CPU, write-through to fs in the background).
  2. Warm load (CPU→GPU).
  3. Cold load. The CPU tier is emptied before each pass, so every block is promoted fs→CPU (`lookup_wait`) and then copied to the GPU.
  4. Mixed store and load under CPU-tier pressure, draining `take_events`. Size `--working-set` against `--cpu-bytes`; the run prints after how many generations the tier fills.
- **pipelined**: the CPU worker chains transfers in submission order, so this shows how much per-batch scheduler overhead overlaps the DMA.
- **prefix-miss**: per-key `lookup_hit` and `lookup_miss`. With an fs tier, `lookup_miss` includes the one-step async probe.
- **contention**: both threads share one manager and worker behind a lock, because vLLM's scheduler is single-threaded. Their GPU→CPU and CPU→GPU copies still overlap on separate CUDA streams. After a dropped store, the store thread backs off for one step (1 ms).
- **scheduler-step**: per request, the bench runs `on_new_request`, `touch`, maximal-prefix lookup and an immediate `prepare_load` (as the connector's `update_state_after_alloc` does). It then prepares stores, submits and drains all jobs, and ends with `on_request_finished`, `on_schedule_end` and `take_events`.
- **--pattern**: reads patterns from `knowledge/workload_patterns/` using certus_fio's `WorkloadPattern`. The CPU-tier geometry is fixed when the stack is built, so all keyspaces use one block size: the largest `object_bytes`, unless `--block-bytes` is given.

## Side-by-side with certus

Run the two benches with matching sizes. Set `--cpu-bytes` to the certus server's `--memory-tier-size`, and use the same `--bs`, `--num-blocks`, `--block-bytes` and `--working-set`. Then join the CSVs on `label`. Rows are labelled identically, but cputier adds columns for its extra phases, so use separate files; appending to one CSV keeps only the first writer's header columns.

```bash
python tools/certus-connector-bench/bench_connector_lifecycle.py --mode all \
    --csv bench-results/connector-certus.csv --tag certus
tools/cputier-connector-bench/run-in-container.sh --cpu-bytes 4G --mode all \
    --csv bench-results/connector-cputier.csv --tag cputier
```
