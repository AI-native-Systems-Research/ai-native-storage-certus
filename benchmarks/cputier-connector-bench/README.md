# cputier-connector-bench

Benchmarks for the **cputier** KV-offload backend: vLLM's in-tree
`OffloadingConnector` with `TieringOffloadingSpec`, which pairs a pinned host-RAM CPU
primary tier with an optional `fs` disk secondary tier. This is the backend
`benchmarks/vllm-serve/run-serve-cputier.sh` serves. The benchmark is the counterpart of
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


**Working set larger than the CPU tier (`--cpu-bytes`).** Refused by default, as in certus-connector-bench, because the warm loads would not be warm. `--no-warm` skips the warm pass, so store and cold run at any `--num-blocks`. `--allow-warm-overflow` runs the warm pass and contention anyway and marks it: the label ends `OVERFLOW: set exceeds tier, not warm`, the CSV row has `warm_overflow=1` and the blocks that came back from disk (`disk_read_bytes`), and `connector-sweep/make_report.py` flags the row.

## Running

Prerequisites:

- A CUDA GPU visible to podman through the NVIDIA CDI device (`--device nvidia.com/gpu=N`).
- A vLLM ≥ 0.30 image: `podman pull docker.io/vllm/vllm-openai:v0.30.0`.

vLLM isn't installed on the host, so the bench runs inside the stock vLLM image with the repo bind-mounted at `/certus`. Nothing needs to be built. There are three ways to run it.

### 1. Wrapper script (easiest)

From the repo root:

```bash
benchmarks/cputier-connector-bench/run-in-container.sh                # default 4 phases
benchmarks/cputier-connector-bench/run-in-container.sh --verify       # data round-trip check first
benchmarks/cputier-connector-bench/run-in-container.sh --mode all --csv bench-results/cputier.csv --tag cputier
benchmarks/cputier-connector-bench/run-in-container.sh --spec cpu     # CPU tier only, no disk tier

# Different GPU, disk-tier location and CPU tier size:
GPU=1 FS_TIER_HOST=/path/on/fast/disk CPU_BYTES=13G \
  benchmarks/cputier-connector-bench/run-in-container.sh --cpu-bytes 13G --min-duration 10
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
  benchmarks/cputier-connector-bench/bench_cputier_lifecycle.py --fs-root /fs-tier --verify
```

- `--security-opt label=disable` lets the container read the bind mounts under SELinux without relabelling the repo.
- `--shm-size` must be at least `--cpu-bytes` plus some headroom; the default `--cpu-bytes` is 8G (certus-connector-bench's `--tier-gib`).
- Leave out `--fs-root` and the `/fs-tier` mount if you use `--spec cpu`.

### 3. Interactively inside the container

Useful for iterating on the script or poking at vLLM:

```bash
podman run --rm -it --pull=never --security-opt label=disable --shm-size 8g \
  --device nvidia.com/gpu=0 -v "$(pwd -P):/certus" -v ~/cputier-bench-fs-tier:/fs-tier \
  -w /certus --entrypoint bash docker.io/vllm/vllm-openai:v0.30.0

# then, inside the container:
python3 benchmarks/cputier-connector-bench/bench_cputier_lifecycle.py --fs-root /fs-tier --help
python3 benchmarks/cputier-connector-bench/bench_cputier_lifecycle.py --fs-root /fs-tier --mode contention
```

Edits you make to the script on the host show up in the container straight away, because the repo is mounted.

### Without a container

If you have vLLM ≥ 0.30 in a Python environment, run the script directly:

```bash
python benchmarks/cputier-connector-bench/bench_cputier_lifecycle.py --fs-root /path/on/nvme
```

### Wrapper environment

| Variable | Default | Description |
|---|---|---|
| `IMAGE` | `docker.io/vllm/vllm-openai:v0.30.0` | vLLM ≥ 0.30. The 0.26 `certus-offload-fix026` image has an older `kv_offload` API and is not supported |
| `GPU` | `0` | CDI GPU index (`--device nvidia.com/gpu=$GPU`), or a comma list (`GPU=0,1`) to expose several GPUs for `--tp`. Inside the container they are renumbered `0..N-1` |
| `FS_TIER_HOST` | `$HOME/cputier-bench-fs-tier` | Host directory for the fs tier, mounted at `/fs-tier`. Put it on the filesystem you want to measure, and not on container overlay storage (no `O_DIRECT`) |
| `CPU_BYTES` | `8G` | Sizes the container `/dev/shm`. Pass the same value as `--cpu-bytes` |
| `SHM_SIZE` | `CPU_BYTES + 4G` | Explicit `/dev/shm` size |

### Notes

- **Disk tier:** `--fs-root` (or `FS_TIER_HOST` with the wrapper) decides which disk the cold-load and write-through numbers measure. The bench writes block files into a per-run subdirectory and deletes it at exit; `--keep-fs` keeps them. Mixed-eviction, contention and scheduler-step modes store new blocks continuously, so expect several GB of disk writes per mode.
- **Output paths:** paths such as `--csv` are resolved inside the container, where the repo is the working directory (`/certus`). So `--csv bench-results/cputier.csv` lands in the repo's `bench-results/`.

### Backend parameters

These play the role of certus-connector-bench's server flags:

| Parameter | Default | Description |
|---|---|---|
| `--spec` | `tiering` | `tiering` = `TieringOffloadingSpec` (cputier); `cpu` = `CPUOffloadingSpec` (host RAM only, no cold phase) |
| `--cpu-bytes` | `8G` | CPU primary tier (`cpu_bytes_to_use`), certus-connector-bench's `--tier-gib` default. To change it, set this and `CPU_BYTES` to the same size; `--num-blocks` follows |
| `--num-blocks` | derived | default and contention: 90% of `--cpu-bytes` x 0.8 / `--block-bytes`, at most 4000 (2948 at the 8G default, 4000 at 12G), as certus-connector-bench derives it from its tier; the other modes: 128 |
| `--fs-root` | `$TMPDIR` | fs tier root (the wrapper passes `/fs-tier`) |
| `--no-fs` | — | Tiering spec with no secondary tier |
| `--fs-read-threads` / `--fs-write-threads` | 16 / 16 | fs tier I/O thread pool |
| `--eviction-policy` | `lru` | CPU tier policy (`lru`, `arc`) |
| `--layers` | 32 | Per-layer KV tensors per block (copy descriptors per block) |
| `--blocks-per-chunk` | 1 | GPU blocks per offloaded chunk |
| `--no-kv-events` | — | Disable KV cache events (`take_events` yields nothing) |
| `--verify` | — | Store random GPU data, zero it, load it back (warm, and cold through the fs tier), and compare bytes |
| `--tp` | 1 | Tensor-parallel world size, one worker per GPU (see below) |
| `--gpus` | `--gpu..--gpu+tp-1` | Devices for the TP ranks |

The workload flags (`--bs`, `--num-blocks`, `--block-bytes`, `--min-duration`, `--working-set`, `--mode`, `--job-blocks`, `--inflight`, `--step-us`, `--store-job-blocks`, `--store-inflight`, `--hit-ratio`, `--requests-per-step`, `--num-sessions`, `--pattern`, `--override`, `--csv`, `--tag`, `--no-cold`, `--no-eviction`, `--lookup-order`) mean the same as in certus-connector-bench.

`--lookup-order` (default `lookup-first`): a request's lookups come first and its `touch` after them, as vLLM's offloading scheduler does since vLLM 0.21 (vLLM PR #41228; still so in 0.30). When a lookup returns `RETRY`/`HIT_PENDING`, the touch follows every lookup pass, because vLLM calls `_touch()` after `_lookup()` even when it defers the request. Exception: `prefix-miss` retries each key in place (unchanged) and touches once after the whole scan. `touch-first` touches once before the lookups, which is vLLM ≤ 0.20's order and this benchmark's behaviour before the option existed. Store-only paths touch before `prepare_store` either way.

### Tensor parallelism (`--tp`)

`--tp 2` runs the vLLM `--tensor-parallel-size 2` pattern: one scheduler-side manager and one
`CPUOffloadingWorker` per GPU, every rank joining the same `/dev/shm` CPU-tier
region and owning its slot of each chunk. Rank 0 runs in the benchmark process,
the other ranks in spawned processes. Every rank receives the same transfer
spec, and a job counts as finished only once all ranks report it.
`--block-bytes` is each rank's shard size (vLLM's `worker_kv_bytes_per_block`);
throughput counts all shards. `--cpu-bytes` is the total CPU tier, shared by
the ranks.

vLLM derives a worker's slot from `current_device_index() % world_size`, so
rank `i` must run on a device whose index maps to slot `i`; the benchmark
rejects other orders. With the wrapper, expose the GPUs and number them from 0:

```bash
GPU=0,1 benchmarks/cputier-connector-bench/run-in-container.sh --tp 2 --mode scheduler-step
```

`--verify` checks rank 0's data only.

## Modes

```bash
B=benchmarks/cputier-connector-bench/run-in-container.sh
$B                                         # default: store, warm load, cold load, mixed eviction
$B --mode prefix-miss --hit-ratio 0.5      # maximal-prefix lookup cost
$B --mode contention                       # loads 64 x 4 and stores 32 x 1 in flight together
$B --mode scheduler-step                   # multi-request steps with shared prefixes
$B --mode all --csv bench-results/cputier.csv --tag cputier
$B --pattern cold_prefill_store            # certus_fio YAML pattern
$B --spec cpu                              # CPU-only offload baseline
```

- **default**: four phases; the first three run jobs in flight (see "Jobs in flight").
  1. Store (GPU→CPU, write-through to fs in the background): `--store-job-blocks` × `--store-inflight`.
  2. Warm load (CPU→GPU): `--job-blocks` × `--inflight`. The working set (`--num-blocks` keys, one CPU-tier chunk each) must fit 90% of the CPU tier's chunks (`--cpu-bytes`), as certus-connector-bench's must fit its tier; a larger one is refused before any run.
  3. Cold load, the same jobs. The CPU tier is emptied before each pass, so every block is promoted fs→CPU (`lookup_wait`, across scheduler steps) and then copied to the GPU.
  4. Mixed store and load under CPU-tier pressure, draining `take_events`. Size `--working-set` against `--cpu-bytes`; the run prints after how many generations the tier fills.
- **prefix-miss**: per-key `lookup_hit` and `lookup_miss`. With an fs tier, `lookup_miss` includes the one-step async probe.
- **contention**: loads and stores in flight together on one manager and worker, single-threaded through the step loop, as vLLM's scheduler and worker are and as certus-connector-bench's contention mode is: `--inflight` load jobs of `--job-blocks` keys over the working set (stored first, shuffled per pass) and `--store-inflight` store jobs of `--store-job-blocks` fresh keys (defaults 32 × 1 here). The fs write-through and any eviction it forces run inside the window. A refused store is counted as dropped and tried again next tick. Labels `Contention: Store Thread (...)` and `Contention: Load Thread (...)` are kept. The two-thread mode (a lock and a one-entry mailbox) is gone; its numbers are not comparable with these.
- **scheduler-step**: the call pattern certus-connector-bench's scheduler-step had until 2026-11-06 (each step's loads merged and waited for inside the step). certus-connector-bench's now runs vLLM 0.30's scheduler (one load job per request, `--inflight`, one reap per tick), so the two no longer compare directly, and `connector-sweep/run_compare.sh` does not compare this mode. Sessions share one prefix pool and diverge at geometric points. Per request, the bench runs `on_new_request`, maximal-prefix lookup, `touch` (before the lookup with `--lookup-order touch-first`) and an immediate `prepare_load` (as the connector's `update_state_after_alloc` does). Loads and stores are deduplicated across the step. Only the loads are submitted and drained, so `drain_workers` measures load completion. The step ends with `on_schedule_end` and `take_events`. Stores are submitted after that and drained, untimed, at the start of the next step, and a request with a store in flight calls `on_request_finished` only once that store completes. `touch_lookup`, `prepare_load` and `prepare_store` are per-step totals.
- **--pattern**: reads patterns from `knowledge/workload_patterns/` using certus_fio's `WorkloadPattern`. The CPU-tier geometry is fixed when the stack is built, so all keyspaces use one block size: the largest `object_bytes`, unless `--block-bytes` is given.

## Jobs in flight

The default mode's store, warm and cold passes and the contention mode drive the stack as vLLM 0.30's `OffloadingConnector` does, with certus-connector-bench's knobs and defaults:

| Flag | Default | |
|---|---|---|
| `--job-blocks` | 64 | keys per load job (and per default-mode store job) |
| `--inflight` | 4 | jobs outstanding; prefix-miss and scheduler-step do not use it |
| `--step-us` | 100 | scheduler tick in µs; 0 = `time.sleep(0)` each tick |
| `--store-job-blocks`, `--store-inflight` | default mode: `--job-blocks`, `--inflight`; contention: 32, 1 | the store side |

Each tick submits the stores prepared in the previous tick and starts loads into free slots. It then makes one non-blocking `get_finished()`, ends the scheduler step (`on_schedule_end`, as vLLM does every step, which flushes the fs tier's async probes and promotions) and sleeps to the next `--step-us` boundary. A load whose lookup answers RETRY or HIT_PENDING is not spun on. It parks and is looked up again on a later tick, as vLLM defers the request to its next step. The wait is `lookup_wait`. Each job has its own GPU block range (slot × job blocks × `--blocks-per-chunk`). Phases `get_finished` (and `load_job` / `store_job`) are each job's submit-to-reap time, `reap_call` the one poll per tick, `*_transfer` the worker's `transfer_time`. Every CSV row records `job_blocks`, `inflight`, `step_us`, `store_job_blocks` and `store_inflight`. `--mode pipelined`, `--pipeline-depth` and `--direction` are gone, as are the one-batch-at-a-time default passes.

**What jobs in flight mean for cputier.** This is vLLM's own async model for this connector. But `CPUOffloadingWorker` chains its transfers per direction on the GPU: each transfer's CUDA stream waits for the previous transfer's end event. So several loads in flight overlap only their host work (lookups, `prepare_load`, submit) with the running copy, and stores likewise; a load and a store do overlap each other. A small gain from `--inflight` is a property of the connector, not of the benchmark.

## Side-by-side with certus

Run the two benches with matching sizes. Set `--cpu-bytes` to the certus server's `--memory-tier-size`, and use the same `--bs`, `--num-blocks`, `--block-bytes` and `--working-set`. Then join the CSVs on `label`. Rows are labelled identically, but cputier adds columns for its extra phases, so use separate files; appending to one CSV keeps only the first writer's header columns.

```bash
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py --mode all \
    --csv bench-results/connector-certus.csv --tag certus
benchmarks/cputier-connector-bench/run-in-container.sh --cpu-bytes 4G --mode all \
    --csv bench-results/connector-cputier.csv --tag cputier
```
