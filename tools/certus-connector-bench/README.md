# certus-connector-bench

Benchmarks for the Python connector classes (`certus-shmq-connector`) that vLLM uses to talk to certus-server. Unlike `certus-fio` (which calls `Ring.populate`/`Ring.lookup` directly), these exercise the full connector code path: BLAKE2b key hashing, TP key namespacing, per-block region construction, `ThreadPoolExecutor` dispatch, and the lookup cache.

Requires a running `certus-server` with shared memory at `/dev/shm/certus-shmq`.

## Scripts

### bench_connector_lifecycle.py

End-to-end lifecycle benchmark with four phases:

1. **Store**: touch → prepare_store → submit → finish → complete
2. **Warm Load**: touch → lookup → prepare → submit → finish → complete (DRAM-resident)
3. **Cold Load**: same path but SSD→DRAM→GPU round-trip
4. **Mixed Eviction**: interleaved store + load under memory-tier pressure, with `take_events` drain

```bash
# Basic run (5s per phase, bs=16, 512-block working set)
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --shm-path /dev/shm/certus-shmq

# Longer run with larger working set
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --min-duration 30 --working-set 2048

# Run a certus-fio YAML pattern through the connector
python tools/certus-connector-bench/bench_connector_lifecycle.py \
    --pattern cold_prefill_store

# TP=2 (2× namespaced keys per logical block)
python tools/certus-connector-bench/bench_connector_lifecycle.py --tp 2

# CSV output
python tools/certus-connector-bench/bench_connector_lifecycle.py --csv

# Skip eviction phase
python tools/certus-connector-bench/bench_connector_lifecycle.py --no-eviction
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

Output goes to `bench-results/` in the repo root. The lifecycle benchmark writes `connector_lifecycle.csv` with per-phase throughput, IOPS, and latency percentiles (p50/p99/mean) for every sub-operation.
