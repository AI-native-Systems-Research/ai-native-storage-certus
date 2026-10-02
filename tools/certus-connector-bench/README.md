# certus-connector-bench

Benchmarks for the Python connector classes (`certus-shmq-connector`) that vLLM uses to talk to certus-server. Unlike `certus-fio` (which calls `Ring.populate`/`Ring.lookup` directly), these exercise the full connector code path: BLAKE2b key hashing, TP key namespacing, per-block region construction, `ThreadPoolExecutor` dispatch, and the lookup cache.

Requires a running `certus-server` with shared memory at `/dev/shm/certus-shmq`.

## Prerequisites

The benchmark does **not** start or stop the server. You must start it before running any benchmark:

```bash
sudo target/release/certus-server \
    --drive-count 4 \
    --channels 16 \
    --memory-tier-size 4G \
    --memory-tier-eviction-threshold 0.7 \
    --format
```

Server parameters (not controlled by the benchmark):

| Parameter | Default | Description |
|---|---|---|
| `--drive-count` | (required) | Number of NVMe drives (or simulated drives) |
| `--channels` | 8 | Mailbox channels = max concurrent requests. Use 16+ for pipelined/contention modes |
| `--memory-tier-size` | (required) | DRAM cache size (e.g. `4G`) |
| `--memory-tier-eviction-threshold` | 0.0 | Start evicting when tier is this fraction full (0.7 = 70%) |
| `--shm-path` | `/dev/shm/certus-shmq` | Shared memory path |
| `--max-eviction-attempts` | 2048 | Per-reserve eviction retry limit |
| `--store-backpressure-ms` | 5000 | Max wait for reserve under pressure |
| `--format` | — | Wipe drives on startup |

To sweep drive counts, restart the server between runs and use `--tag` in the benchmark to label each:

```bash
for drives in 1 2 4; do
    sudo target/release/certus-server --drive-count $drives --channels 16 \
        --memory-tier-size 4G --memory-tier-eviction-threshold 0.7 --format &
    sleep 3
    python tools/certus-connector-bench/bench_connector_lifecycle.py \
        --mode all --min-duration 5 --csv bench-results/drive_sweep.csv \
        --tag "${drives}-drive"
    sudo kill %1; wait
done
```

## Scripts

### bench_connector_lifecycle.py

End-to-end lifecycle benchmark with multiple modes. Without `--mode`, runs the default 4-phase serial benchmark. Each mode targets a different optimization surface.

#### Default mode (no `--mode` or `--mode default`)

Four serial phases:

1. **Store**: touch → prepare_store → submit → finish → complete
2. **Warm Load**: touch → lookup → prepare → submit → finish → complete (DRAM-resident)
3. **Cold Load**: same path but SSD→DRAM→GPU round-trip
4. **Mixed Eviction**: interleaved store + load under memory-tier pressure, with `take_events` drain

```bash
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --shm-path /dev/shm/certus-shmq

# Longer run with larger working set
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --min-duration 30 --working-set 2048
```

#### Pipelined mode (`--mode pipelined`)

Submits multiple batches before reaping to saturate the ThreadPoolExecutor. Reveals whether DMA operations overlap and whether the worker pool or the ring/server is the bottleneck.

```bash
# Store + load, 4 batches in-flight
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --mode pipelined --pipeline-depth 4

# Store only, deeper pipeline
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --mode pipelined --pipeline-depth 8 --direction store
```

Key metric: if `submit` time >> `pipeline_drain` time, the ring is the bottleneck. If `pipeline_drain` >> `submit`, the server/DMA is.

#### Prefix-miss mode (`--mode prefix-miss`)

Measures the cost curve of vLLM's `_maximal_prefix_lookup` at varying hit ratios. Pre-stores a fraction of keys, then runs touch → per-key sequential lookup breaking at the first miss.

```bash
# 75% prefix hit ratio (default)
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --mode prefix-miss --hit-ratio 0.75

# Compare: all-miss vs all-hit
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --mode prefix-miss --hit-ratio 0.0
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --mode prefix-miss --hit-ratio 1.0
```

Reports cache-hit vs cache-miss-fallback lookup latency separately, plus average prefix length before the first miss.

#### Contention mode (`--mode contention`)

Runs store and load streams from separate threads against the same ring. Reveals server-side contention (reserve vs pin, copy_to_store vs lookup, write-through vs promote).

```bash
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --mode contention --min-duration 10 --working-set 1024
```

Reports per-thread throughput and latency. Compare against `--mode default` to see the contention penalty.

#### Scheduler-step mode (`--mode scheduler-step`)

Simulates full continuous-batching scheduler steps: multiple requests per step, each with touch + maximal-prefix-lookup, pipelined dispatch, and event drain. Models multi-turn conversation prefix sharing.

```bash
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --mode scheduler-step --requests-per-step 8 --num-sessions 4

# Heavier load
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --mode scheduler-step --requests-per-step 16 --num-sessions 8 \
    --min-duration 30
```

Reports steps/sec, per-step phase breakdown (touch_lookup, prepare, submit, drain, complete, events), and prefix hit/suffix store counts.

#### Pattern mode (`--pattern`)

Runs a certus-fio YAML workload pattern through the connector lifecycle.

```bash
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --pattern cold_prefill_store

python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --pattern warm_prefill_load_and_suffix_store
```

#### Run all modes

```bash
python tools/certus-connector-bench/bench_connector_lifecycle.py --mode all --min-duration 5
```

#### Common options

```
--shm-path PATH        certus-server shmq mailbox path (default: /dev/shm/certus-shmq)
--bs N                 Batch size in blocks (default: 16)
--num-blocks N         Total blocks per iteration (default: 128)
--block-bytes N        Per-block size in bytes (default: 2097152 = 2 MiB)
--gpu N                CUDA device index (default: 0)
--tp N                 Simulated tensor-parallel world size (default: 1)
--min-duration SECS    Minimum seconds per phase (default: 5.0)
--workers N            ThreadPoolExecutor worker count (default: 4)
--working-set N        Blocks for eviction/contention pressure (default: 512)
--csv PATH             Append results to CSV file
--tag TEXT             Tag column for CSV (e.g. branch name)
```

#### Per-mode options

```
--pipeline-depth N     (pipelined, scheduler-step) Batches in-flight before reaping (default: 4)
--direction DIR        (pipelined) store, load, or both (default: both)
--hit-ratio F          (prefix-miss) Fraction of keys pre-stored, 0.0-1.0 (default: 0.75)
--requests-per-step N  (scheduler-step) Requests per scheduler step (default: 8)
--num-sessions N       (scheduler-step) Distinct conversations in the pool (default: 4)
```

### bench_connector_path.py

Measures the connector's 3-phase store and check→pin→lookup load paths at the `Ring` level with connector overhead (reserve → copy_to_store → commit_store for stores; check → pin → lookup for loads).

```bash
python tools/certus-connector-bench/bench_connector_path.py \
    --shm-path /dev/shm/certus-shmq --bs 16 --num-blocks 128
```

### bench_connector_race.py

Reproduces and validates the check→pin→lookup eviction race. Two threads run concurrently: a storer that fills the memory tier forcing eviction, and a loader that uses `check_and_pin` to atomically verify+protect keys. Validates that pinned keys see zero load failures.

```bash
python tools/certus-connector-bench/bench_connector_race.py \
    --shm-path /dev/shm/certus-shmq --duration 30
```

## Results

Output goes to `bench-results/` in the repo root. The lifecycle benchmark writes `connector_lifecycle.csv` with per-phase throughput, IOPS, and latency percentiles (p50/p99/mean) for every sub-operation. The `mode` column identifies which benchmark mode produced each row.
