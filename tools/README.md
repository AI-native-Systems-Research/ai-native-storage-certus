# certus-fio

FIO-like pattern-driven benchmark for certus KV cache storage.

Reads workload pattern YAML files and executes them against a running certus-server. Each pattern isolates one storage IO behavior for microbenchmarking.

## 9 Core Benchmarks

| # | Benchmark | Pattern YAML | Bottleneck it isolates |
|---|-----------|-------------|----------------------|
| 1 | bulk-store | `cold_prefill_store.yaml` | GPU→DRAM DMA bandwidth |
| 2 | trickle-store | `decode_block_store.yaml` | Per-RPC overhead (gRPC + IPC open + DMA setup) |
| 3 | durable-store | `background_writeback_flush_and_backpressure.yaml` | NVMe write bandwidth (background writer saturation) |
| 4 | hot-load | `hot_vs_cold_load_paths.yaml` | DRAM→GPU DMA bandwidth |
| 5 | shared-key-fanout | `cohort_sharing_and_load_coalescing.yaml` | Lock contention on same dispatch-map entries |
| 6 | bidirectional | `bidirectional_store_load_contention.yaml` | NVMe/DRAM bandwidth arbitration between read+write |
| 7 | warm-prefill | `warm_prefill_load_and_suffix_store.yaml` | End-to-end request latency (load prefix + store suffix) |
| 8 | eviction+reload | `compute_local_eviction_and_later_reload.yaml` | LRU policy + cold-load path under natural pressure |
| 9 | multi-turn | `multi_turn_reuse.yaml` | Growing context overhead (does latency degrade with history size?) |

When BENCH_TARGET (vLLM multi-turn replay) regresses, run all 9 — the one that degraded tells you which subsystem broke.

## Usage

```bash
# List all available patterns
python3 tools/certus-fio.py list

# Describe what a pattern will do (no server needed)
python3 tools/certus-fio.py describe --pattern cold_prefill_store

# Describe with parameter overrides
python3 tools/certus-fio.py describe --pattern bidirectional_store_load_contention \
  --override store_actors=2 load_actors=8

# Run a benchmark (requires running certus-server)
python3 tools/certus-fio.py run --pattern cold_prefill_store --server localhost:50051

# Run with overrides
python3 tools/certus-fio.py run --pattern hot_vs_cold_load_paths \
  --server localhost:50051 --override requested_blocks=512

# Run with cleanup (clear memory tier before measuring)
python3 tools/certus-fio.py run --pattern hot_vs_cold_load_paths \
  --server localhost:50051 --cleanup-before

# Control concurrency
python3 tools/certus-fio.py run --pattern cold_prefill_store \
  --server localhost:50051 --pipeline-depth 8
```

## How It Works

1. **Load pattern YAML** — parse parameters, keyspaces, preconditions, phases
2. **Resolve parameters** — apply `--override` values or use defaults
3. **Setup preconditions**:
   - `present_in_store` → Populate seed blocks
   - `absent_from_local_cache` → ClearMemoryTier (force cold path)
   - `absent_from_store` → Remove any existing keys
4. **Execute phases** in order:
   - `store` → Populate RPC (GPU→DRAM)
   - `load` → Lookup RPC (DRAM→GPU or SSD→DRAM→GPU)
   - `delete` → Remove RPC
   - Concurrency controlled by semaphore (actors.concurrency)
   - Barrier between phases if `barrier_after: true`
5. **Report** per-phase: latency p50/p99/avg, throughput GB/s, total ops, errors
6. **Cleanup** — Remove all keys created during the run, free GPU buffers

## Cleaning Between Runs

certus-fio cleans up automatically after each run:
- All keys created during the run are removed via `BatchRemoveRequest`
- All GPU buffers are freed via `cudaFree`
- The gRPC channel is closed

For explicit pre-run cleanup, use `--cleanup-before` which calls `ClearMemoryTier` before starting.

Between different benchmark patterns, no manual cleanup is needed — each run uses a unique random key base that doesn't collide with prior runs.

**Server restart**: For the most reliable results (no stale SSD state), restart certus-server with `--format` between benchmark suites. Individual runs within a suite don't need server restarts.

## Pattern YAML Location

Patterns are in: `knowledge/workload_patterns/*.yaml`

Each YAML file follows the workload pattern schema:
```yaml
id: cold-prefill-store
name: Cold Prefill Bulk Store
status: candidate
parameters:
  prompt_tokens: {type: integer, range: [128, 131072], default: 4096}
  ...
keyspaces:
  prefill_blocks:
    cardinality: "ceil(prompt_tokens / block_size)"
    object_bytes: "block_size * num_layers * kv_bytes_per_token_per_layer"
    sharing: per_actor
    disjoint_between_actors: true
preconditions:
  - {subject: prefill_blocks, state: absent_from_store, value: true}
phases:
  - id: prefill-writeback
    actors: {count: 1, arrival: burst, concurrency: 1}
    operations:
      - {op: store, keys: prefill_blocks, order: sequential}
expected_io:
  stores: "ceil(prompt_tokens / block_size)"
  loads: "0"
```

## Operation Mapping

| Pattern op | certus RPC | Direction |
|-----------|-----------|-----------|
| `store` | `Populate` | GPU → DRAM (immediate) + DRAM → SSD (background) |
| `load` | `Lookup` | DRAM → GPU (hot) or SSD → DRAM → GPU (cold) |
| `delete` | `Remove` | Free from all tiers |

## Hot vs Cold Load

The same `hot_vs_cold_load_paths` pattern tests both paths depending on preconditions:
- **Hot**: blocks in DRAM → precondition `present_in_store` only → measures DRAM→GPU
- **Cold**: blocks on SSD only → precondition `present_in_store` + `absent_from_local_cache` → measures SSD→DRAM→GPU

Use `--cleanup-before` to force cold path on any load pattern.

## Relationship to certus-api-bench v1/v2/v3

| certus-api-bench | certus-fio equivalent |
|-----------------|----------------------|
| v2 Populate phase | `cold_prefill_store` |
| v2 Hot Lookup | `hot_vs_cold_load_paths` |
| v2 Cold Lookup | `hot_vs_cold_load_paths --cleanup-before` |
| v3 Bidirectional phase | `bidirectional_store_load_contention` |
| v3 Per-block latency | `decode_block_store` |

certus-fio adds 5 benchmarks v1/v2/v3 cannot do: durable-store, shared-key-fanout, warm-prefill, eviction+reload, multi-turn.
