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

# Run a single pattern
python3 tools/certus-fio/certus_fio.py run --pattern cold_prefill_store

# Override parameters
python3 tools/certus-fio/certus_fio.py run --pattern cold_prefill_store --override batch_size=1

# Run the cold SSD path individually
python3 tools/certus-fio/certus_fio.py run --pattern hot_vs_cold_load_paths

# Quick run (1 iteration, no auto-repeat)
python3 tools/certus-fio/certus_fio.py run --pattern decode_block_store --min-duration 0
```

## CLI Reference

```
certus-fio list                             List all available patterns
certus-fio describe --pattern NAME          Show parameters, keyspaces, phases
certus-fio run --pattern NAME [opts]        Run a single pattern
certus-fio full [opts]                      Full sweep (core patterns × sizes × batch)
certus-fio report [opts]                    Full sweep + HTML report with analysis

Run options:
  --override key=value [...]    Override pattern parameters
  --warmup N                    Warmup ops before measurement (default: 8, 0 to disable)
  --min-duration SECS           Minimum measurement time; auto-repeats iterations (default: 3.0)
  --max-iterations N            Safety cap on iteration repeats (default: 200)
  --gpu ID                      GPU device ID (default: 0)
  --cleanup-before              Clear memory tier before run

Full/Report options:
  --min-duration SECS           Time per run (default: 3.0)
  --warmup N                    Warmup ops (default: 8)
  --output FILE                 Write HTML report (report) or CSV (full)
  --csv FILE                    Also write raw CSV (report command)
  --from-csv FILE               Generate report from existing CSV (skip sweep)
  --gpu ID                      GPU device ID (default: 0)
```

## Full Sweep & Report

The `full` and `report` commands run 12 core patterns (8 warm + 4 cold) across object sizes and batch sizes:

```bash
# Full sweep with CSV output (~9 min)
python3 tools/certus-fio/certus_fio.py full --output results.csv

# Generate HTML report with optimization analysis
python3 tools/certus-fio/certus_fio.py report --output report.html --csv results.csv

# Regenerate report from existing data (no server needed)
python3 tools/certus-fio/certus_fio.py report --from-csv results.csv --output report.html
```

**Sweep dimensions:**

| Dimension | Values | Rationale |
|-----------|--------|-----------|
| Object size | 1 MiB (Llama-8B), 3 MiB (Llama-30B), 5 MiB (Llama-70B) | Covers the model-size range |
| Batch size | 1, 4, 16, 64, 256 | From serial decode to full-context handoff |
| Warm patterns | 8 patterns (DRAM path) | Store/load/contention across inference scenarios |
| Cold patterns | 4 patterns (SSD path) | Cache miss, selective retrieval, prefetch, remote miss |

The report outputs:
- Peak throughput stat tiles (warm load, cold load, store, contention)
- Optimization findings with severity and impact
- Throughput-by-pattern table with data path (GPU→DRAM / DRAM→GPU / SSD→GPU)
- Batch size sensitivity heatmap
- Object size scaling heatmap
- Prioritized optimization recommendations

## How It Works

Each pattern YAML defines:
- **parameters**: model-derived values (prompt_tokens, block_size, num_layers, etc.)
- **keyspaces**: object pools with cardinality, size, and sharing semantics
- **preconditions**: setup state (pre-populate keys, flush to SSD, clear memory tier)
- **phases**: actor threads issuing store/load/delete operations
- **batch_size**: keys per shmq ring call (models scheduler-step granularity)

The runner:
1. Runs warmup ops (CUDA context, IPC handles, TLBs)
2. Sets up preconditions (populate/flush/clear as needed)
3. Runs the pattern phases with threaded actors
4. Auto-repeats until `--min-duration` is met for stable measurements
5. Reports throughput, latency percentiles, and op counts
6. Cleans up all stored keys

**Warm vs cold path isolation**: The sweep runs warm patterns first (data stays in DRAM memory tier), then reconnects and runs cold patterns (each forces SSD reads via flush + clear_memory_tier). This prevents cold pattern memory tier evictions from interfering with warm path measurements.

## Inference-to-Storage Mapping

| Inference Event | Storage Op | batch_size | Data Path |
|----------------|-----------|-----------|-----------|
| Prefill completes (cold miss) | Store all KV blocks | ALL | GPU → DRAM |
| Prefix cache hit | Load matched blocks | ALL | DRAM → GPU |
| Decode token seals a block | Store one block | 1 | GPU → DRAM |
| Preemption (V1 incremental) | Flush pending stores | 1 | GPU → DRAM |
| Swap-in / resume (warm) | Load entire sequence | ALL | DRAM → GPU |
| Swap-in / resume (cold) | Load from SSD | ALL | SSD → GPU |
| Selective KV retrieval | Load subset of pages | ~10% | SSD → GPU |
| Routing-hint prefetch | Speculative SSD load | BATCH | SSD → GPU |
| Eviction | Demote to SSD | 1 | DRAM → SSD |
| Background write-through | Persist to SSD | 1 (async) | DRAM → SSD |

## Realistic Batch Sizes

Each `batch_size` in the sweep maps to a real scheduler behavior. These are physical call sizes — blocks per single ring/RPC submission to the storage layer (not logical request sizes, which may be split across submissions).

| Operation | Realistic batch_size | Rationale |
|-----------|---------------------|-----------|
| Prefill store (cold miss) | 32–256 blocks | Whole sequence: 4096 tokens / 16 block_size = 256. Chunked prefill caps at 512–2048 tokens/step → 32–128 blocks/call. Our ShareGPT trace: 94.5% of stores are bs=1, tail reaches 48. |
| Decode store | 1 | One block seals every 16 generated tokens. Multiple sequences may coalesce to 2–16 in one scheduler step. |
| Swap-in / restore (warm or cold) | 32–256 blocks | Whole preempted sequence loaded at once. A 4096-token Llama-70B context = 256 × 5 MiB = 1.28 GiB — real systems split into 16–64 block submissions. |
| Eviction / preemption | 1–16 | LRU watermark reclamation evicts one sequence at a time. Per-block incremental eviction (bs=1) or small bulk (bs=4–16). Full-request preemption up to 64–256. |
| Selective retrieval | 8–64 | Query-aware page selection: ~10% of pages for a 1024-page context. Mooncake/MemServe pattern. |

**Sweep batch sizes**: `[1, 4, 16, 64, 256]`
- `1`: Decode trickle, per-block eviction (dominant steady-state case, 94.5% of real stores)
- `4`: Coalesced decode, small eviction batch
- `16`: Chunked prefill step, eviction watermark
- `64`: Large prefill chunk, swap-in submission (real-world physical call ceiling)
- `256`: Full 4K-context handoff — shows the plateau vs 64 (throughput no longer increases)

## Object Size

Each key = one logical KV block = all layers for `block_size` tokens:
```
object_bytes = block_size × num_layers × num_kv_heads × head_dim × 2(K+V) × dtype_bytes
```
Llama-70B default: 16 × 80 × 8 × 128 × 2 × 2 = 5 MiB per object.

## Core Benchmark Suite

### Warm Path (DRAM ↔ GPU)

| Pattern | Models |
|---------|--------|
| `cold_prefill_store` | Prefill offload throughput (batched store) |
| `decode_block_store` | Decode seal latency (serial store, bs=1) |
| `warm_prefill_load_and_suffix_store` | Prefix cache hit restore + suffix store |
| `preemption_and_reschedule` | Incremental offload + bulk restore |
| `compute_local_eviction_and_later_reload` | Eviction + demand reload (barrier between phases) |
| `bidirectional_store_load_contention` | Concurrent path interference |
| `disaggregated_prefill_decode` | P/D bulk handoff (batched both ways) |
| `continuous_batching_mix` | Mixed prefill burst + decode trickle (concurrent actors) |

### Cold Path (SSD → GPU)

| Pattern | Models |
|---------|--------|
| `hot_vs_cold_load_paths` | Cache-miss reload after eviction (full-sequence SSD fetch) |
| `selective_kv_retrieval` | Query-aware page selection (Mooncake/MemServe style, 10% of pages) |
| `tier_promotion_and_prefetch` | Routing-hint speculative prefetch from lower tier |
| `cache_aware_routing_and_remote_hit_migration` | Remote miss fallback to local SSD |

All 35+ patterns are available as an extended regression suite via `certus-fio list`.

## Prerequisites

- Running `certus-server` (creates `/dev/shm/certus-shmq`)
- Python 3.9+ with `pyyaml`
- CUDA GPU and `libcudart.so`
- `certus_shmq_helpers` module (from `apps/python/`)
