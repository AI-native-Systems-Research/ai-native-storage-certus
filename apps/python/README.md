# Certus Python Benchmarks

These scripts drive `certus-server` over its `/dev/shm` mailbox (shmq) — the
sole control transport now that gRPC has been removed. The wire client is the
pure-Python `Ring` in `../../certus-shmq-connector/certus_shmq_connector/ring.py`,
located at runtime by `certus_shmq_helpers.py` (a `sys.path` insert, not a pip
install). CUDA IPC handles are exported directly via `libcudart` (`ctypes`), so
the scripts are transport-agnostic below the `Ring` call boundary.

## certus-api-bench.py

Multi-client throughput and latency benchmark for the Certus Dispatcher.
Spawns N concurrent client threads, each issuing populate/lookup operations
with 4 MiB cache blocks. Measures both hot (memory-tier) and cold (SSD-tier)
throughput and latency.

### Prerequisites

- Python 3.8+
- NVIDIA GPU with CUDA drivers + `libcudart.so`
- A running `certus-server` with a shmq mailbox

### Setup

```bash
pip install -r requirements.txt   # see the file: no third-party deps for shmq
pip install torch                 # only for scripts that allocate GPU buffers
```

Launch the server so it exposes at least as many channels as the benchmark's
concurrency (each client thread claims its own channel; a thread with no free
channel errors):

```bash
certus-server --shm-path /dev/shm/certus-shmq --channels 32 --device-pci <BDF>
```

### Usage

```bash
python certus-api-bench.py [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--shm-path` | `/dev/shm/certus-shmq` | certus-server shmq mailbox path |
| `--clients` | `1` | Number of concurrent client threads |
| `--num-objects` | `16` | Objects per lookup batch per client |
| `--iterations` | `10` | Lookup iterations per phase |

### Examples

Single client, default settings:

```bash
python certus-api-bench.py
```

Eight concurrent clients (server must be launched with `--channels >= 8`):

```bash
python certus-api-bench.py --clients 8 --num-objects 32 --iterations 20
```

Against a mailbox at a non-default path:

```bash
python certus-api-bench.py --clients 4 --shm-path /dev/shm/certus-alt
```

### Benchmark Phases

1. **Populate** -- Each client writes enough 4 MiB objects to overflow its share of the 256 MiB memory-tier pool, forcing early keys to SSD.
2. **Hot lookups** -- Reads objects still resident in the memory tier (DRAM path).
3. **Cold lookups** -- Reads objects evicted to SSD (NVMe read + promote path).

### Output

Reports per-object latency statistics (avg, p50, p99, min, max) and aggregate
throughput (GB/s) for each phase, plus a per-client breakdown.

## Concurrency note (shmq vs gRPC)

The old gRPC clients pipelined many RPCs down one channel via
`stub.X.future(req)`. The `Ring` client is **one request in flight per
channel**, and each calling thread claims its own channel on first use. The
scripts therefore get pipelining from a `ThreadPoolExecutor` whose worker count
is the pipeline depth, each worker making a blocking `ring.X(...)` call — so the
server must be launched with `--channels >=` that concurrency.
