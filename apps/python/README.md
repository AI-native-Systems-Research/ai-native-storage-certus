# Certus Python Benchmarks

## certus-api-bench.py

Multi-client throughput and latency benchmark for the Certus gRPC Dispatcher.
Spawns N concurrent client threads, each issuing populate/lookup operations
with 4 MiB cache blocks. Measures both hot (memory-tier) and cold (SSD-tier)
throughput and latency.

### Prerequisites

- Python 3.8+
- NVIDIA GPU with CUDA drivers (torch must see `cuda:0`)
- A running Certus gRPC server

### Setup

```bash
pip install -r requirements.txt
pip install torch  # install separately per your CUDA version
```

If the `.proto` definition changes, regenerate the Python stubs:

```bash
./generate_pb.sh
```

### Usage

```bash
python certus-api-bench.py [OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--server` | `localhost:50051` | Certus gRPC server address |
| `--clients` | `1` | Number of concurrent client threads |
| `--num-objects` | `16` | Objects per lookup batch per client |
| `--iterations` | `10` | Lookup iterations per phase |

### Examples

Single client, default settings:

```bash
python certus-api-bench.py
```

Four concurrent clients against a remote server:

```bash
python certus-api-bench.py --clients 4 --server 10.0.0.5:50051
```

High-throughput sweep with larger batches:

```bash
python certus-api-bench.py --clients 8 --num-objects 32 --iterations 20
```

### Benchmark Phases

1. **Populate** -- Each client writes enough 4 MiB objects to overflow its share of the 256 MiB memory-tier pool, forcing early keys to SSD.
2. **Hot lookups** -- Reads objects still resident in the memory tier (DRAM path).
3. **Cold lookups** -- Reads objects evicted to SSD (NVMe read + promote path).

### Output

Reports per-object latency statistics (avg, p50, p99, min, max) and aggregate
throughput (GB/s) for each phase, plus a per-client breakdown.
