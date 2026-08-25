# certus-fio

Pattern-driven benchmark for the Certus shmq storage dispatcher. Maps real LLM inference KV cache operations to storage IO patterns and measures them against a running certus-server.

## Quick Start

```bash
# Start the server
sudo rm -f /var/tmp/spdk_pci_lock*
./target/release/certus-server \
  --device-pci 0000:61:00.0 \
  --device-pci 0000:62:00.0 \
  --device-pci 0000:63:00.0 \
  --device-pci 0000:64:00.0 \
  --format \
  --memory-tier-size 4G

# List available patterns
python3 tools/certus-fio/certus_fio.py list

# Run a pattern
python3 tools/certus-fio/certus_fio.py run --pattern cold_prefill_store

# Override parameters
python3 tools/certus-fio/certus_fio.py run --pattern cold_prefill_store --override batch_size=1

# Quick run (1 iteration, no auto-repeat)
python3 tools/certus-fio/certus_fio.py run --pattern decode_block_store --min-duration 0

# Sweep batch sizes
for bs in 1 4 8 16 32 64 128 256; do
  python3 tools/certus-fio/certus_fio.py run --pattern cold_prefill_store --override batch_size=$bs
done
```

## CLI Reference

```
certus-fio list                         List all available patterns
certus-fio describe --pattern NAME      Show parameters, keyspaces, phases
certus-fio run --pattern NAME [opts]    Run a single pattern
certus-fio full [opts]                  Full sweep across core patterns × object sizes × batch sizes

Run options:
  --override key=value [...]    Override pattern parameters
  --warmup N                    Warmup ops before measurement (default: 8, 0 to disable)
  --min-duration SECS           Minimum measurement time; auto-repeats iterations (default: 3.0)
  --max-iterations N            Safety cap on iteration repeats (default: 200)
  --gpu ID                      GPU device ID (default: 0)
  --cleanup-before              Clear memory tier before run

Full sweep options:
  --min-duration SECS           Time per run (default: 3.0)
  --warmup N                    Warmup ops (default: 8)
  --output FILE                 Write results to CSV
  --gpu ID                      GPU device ID (default: 0)
```

## Full Sweep

The `full` command runs the 8 core patterns across a matrix of object sizes and batch sizes to characterize the system:

```bash
# Full sweep with CSV output (~6 min at default settings)
python3 tools/certus-fio/certus_fio.py full --output results.csv

# Quick sweep (1s per run, ~2 min)
python3 tools/certus-fio/certus_fio.py full --min-duration 1.0 --output quick.csv
```

**Sweep dimensions:**

| Dimension | Values | Rationale |
|-----------|--------|-----------|
| Object size | 1 MiB (Llama-8B), 3 MiB (Llama-30B), 5 MiB (Llama-70B) | Covers the model-size range |
| Batch size | 1, 4, 16, 64, 256 | From serial decode to full-prefill batched |
| Patterns | 8 core patterns | Store/load/contended across inference scenarios |

This produces a throughput surface that shows:
- How batch_size affects each operation type (loads vs stores)
- How object size scales with PCIe bandwidth
- Where contention and server-side serialization create bottlenecks
- The optimal batch_size knee for this hardware configuration

## How It Works

Each pattern YAML defines:
- **parameters**: model-derived values (prompt_tokens, block_size, num_layers, etc.)
- **keyspaces**: object pools with cardinality, size, and sharing semantics
- **preconditions**: setup state (pre-populate keys, clear memory tier)
- **phases**: actor threads issuing store/load/delete operations
- **batch_size**: keys per shmq ring call (models scheduler-step granularity)

The runner:
1. Runs warmup ops (CUDA context, IPC handles, TLBs)
2. Sets up preconditions (populate/clear as needed)
3. Runs the pattern phases
4. Auto-repeats until `--min-duration` is met for stable measurements
5. Reports throughput, latency percentiles, and op counts
6. Cleans up all stored keys

## Inference-to-Storage Mapping

| Inference Event | Storage Op | batch_size | Direction |
|----------------|-----------|-----------|-----------|
| Prefill completes (cold miss) | Store all KV blocks | ALL | GPU → DRAM |
| Prefix cache hit | Load matched blocks | ALL | DRAM → GPU |
| Decode token seals a block | Store one block | 1 | GPU → DRAM |
| Preemption (V1 incremental) | Flush pending stores | 1 | GPU → DRAM |
| Swap-in / resume | Load entire sequence | ALL | DRAM/SSD → GPU |
| Eviction | Demote to SSD | 1 | DRAM → SSD |
| Background write-through | Persist to SSD | 1 (async) | DRAM → SSD |

## Object Size

Each key = one logical KV block = all layers for `block_size` tokens:
```
object_bytes = block_size × num_layers × num_kv_heads × head_dim × 2(K+V) × dtype_bytes
```
Llama-70B default: 16 × 80 × 8 × 128 × 2 × 2 = 5 MiB per object.

## Core Benchmark Suite

| Pattern | Models |
|---------|--------|
| `cold_prefill_store` | Prefill offload throughput (batched store) |
| `decode_block_store` | Decode seal latency (serial store) |
| `warm_prefill_load_and_suffix_store` | Prefix cache hit restore + suffix store |
| `preemption_and_reschedule` | Incremental offload + bulk restore |
| `compute_local_eviction_and_later_reload` | Eviction + demand reload |
| `bidirectional_store_load_contention` | Concurrent path interference |
| `disaggregated_prefill_decode` | P/D bulk handoff |
| `continuous_batching_mix` | Mixed prefill burst + decode trickle |

All 35 patterns are available as an extended regression suite.

## Prerequisites

- Running `certus-server` (creates `/dev/shm/certus-shmq`)
- Python 3.9+ with `pyyaml`
- CUDA GPU and `libcudart.so`
- `certus_shmq_helpers` module (from `apps/python/`)
