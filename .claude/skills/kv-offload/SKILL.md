---
name: kv-offload
description: Run KV offloading benchmarks — end-to-end inference or isolated trace replay against CPU, FS, or Certus backends
argument-hint: "[e2e|replay] [cpu|fs|certus] [dataset-or-trace]"
---

This skill drives the kv-offload benchmark pipeline. Two modes:

1. **e2e** — runs vLLM end-to-end with tracing connectors against a dataset. Measures real inference + offloading performance and produces trace files as a side-effect.
2. **replay** — drives a previously-captured trace against a storage backend in isolation (no GPU needed). Measures storage throughput, latency, and hit rate.

### Arguments

**First:** mode
- `e2e` — end-to-end inference benchmark (GPU required). Default backend: **cpu**.
- `replay` — isolated storage replay (no GPU). Default backend: **all**.

**Second:** connector/backend
- `cpu` — in-memory DRAM offloading (no extra hardware)
- `fs` — NVMe via llmd_fs_backend (requires XFS mount)
- `certus` — CXL DRAM via certus_native (requires SPDK + vfio-pci)
- `all` — run against all available backends sequentially (replay default)

**Third:** dataset (e2e) or trace (replay)
- e2e datasets:
  - **sharegpt** — full ShareGPT_V3 conversations (`/home/bdh/kvconn-trace/sharegpt_v3.json`)
  - **sharegpt-5k** — pre-filtered 5000-conversation subset (`/home/bdh/kvconn-trace/sharegpt_subset_5000.json`)
  - Or a path to any ShareGPT-format JSON file
  - Default: **sharegpt-5k**
- replay traces:
  - **500convs-64g** — 500 multi-turn conversations, 64 GiB tier (`traces/sharegpt-multiturn/500convs-64g`)
  - **5000-prompts** — 5000 single-turn prompts (`traces/sharegpt/5000-prompts`)
  - Or a path/glob to trace JSONL files
  - Default: **500convs-64g**

### Examples

```
/kv-offload e2e cpu sharegpt-5k
/kv-offload e2e certus sharegpt
/kv-offload replay cpu 500convs-64g
/kv-offload replay fs 500convs-64g
/kv-offload replay certus 500convs-64g
```

## Paths

All scripts live under `benchmarks/kv-offload-replay/` relative to the repo root.
The Python venv is at `/home/bdh/kvconn-trace/.venv` — activate it before running any Python.

```bash
source /home/bdh/kvconn-trace/.venv/bin/activate
cd /home/bdh/kvconn-trace/ai-native-storage-certus/benchmarks/kv-offload-replay
```

## e2e mode

Runs vLLM end-to-end with a tracing connector against a ShareGPT dataset. Measures actual GPU prefill/decode plus offloading I/O. Trace files are produced as a side-effect for later replay.

### Prerequisites

- A ShareGPT JSON file in the benchmark directory. If missing, generate one:
  ```bash
  python prefilter_sharegpt_for_bench.py --input <raw_sharegpt.json> --output sharegpt_subset_5000.json --max-conversations 5000
  ```
- GPU with enough VRAM for the model (default: Llama-3-8B in float16, ~16 GB)
- For non-CPU backends: the relevant hardware and drivers (see Hardware notes)

### Backend → script mapping

| Backend | Script | Connector |
|---------|--------|-----------|
| cpu | `run_multiturn_offloading.py` | TracingOffloadingConnector |
| certus | `run_sharegpt_certus.py` | TracingCertusConnector |
| fs | `run_fs_bench.py` | OffloadingConnector + fs-backend |

### Running — cpu

```bash
NUM_CONVS=500 CPU_BYTES=68719476736 MODEL=NousResearch/Meta-Llama-3-8B \
  python run_multiturn_offloading.py
```

Environment variables (all optional, with defaults):
- `NUM_CONVS` — number of conversations (default: 500)
- `MAX_MODEL_LEN` — vLLM context window in tokens (default: 8192)
- `OUTPUT_TOKENS` — max generated tokens per turn (default: 200)
- `MAX_NUM_SEQS` — vLLM batch parallelism (default: 64)
- `GPU_MEM_UTIL` — vLLM gpu_memory_utilization (default: 0.90)
- `CPU_BYTES` — offload tier size in bytes (default: 4 GiB). Use 68719476736 for 64 GiB.
- `MODEL` — HuggingFace model ID (default: NousResearch/Meta-Llama-3-8B)

### Running — certus

```bash
# Start SPDK server first
python certus_server.py --data-pci 0000:61:00.0 --meta-pci 0000:62:00.0 &

python run_sharegpt_certus.py --num-prompts 500 --max-model-len 8192
```

### Running — fs

```bash
python run_fs_bench.py
```

Requires `/mnt/fs-backend-bench` mounted (XFS on NVMe).

### Output

Raw trace files appear in the benchmark directory:
- `offloading_mgr_*.jsonl` — manager-level events (lookup, prepare_load, complete_load, prepare_store, touch)
- `offloading_handler_*.jsonl` — handler-level events (store_async, load_async, complete_store, complete_load)

### Packaging traces for later replay

After an e2e run, compress and archive traces:
```bash
TRACE_NAME="500convs-64g"  # descriptive name
mkdir -p traces/sharegpt-multiturn
gzip -c offloading_mgr_*.jsonl > traces/sharegpt-multiturn/${TRACE_NAME}.mgr.jsonl.gz
gzip -c offloading_handler_*.jsonl > traces/sharegpt-multiturn/${TRACE_NAME}.handler.jsonl.gz
```

Create a metadata file `traces/sharegpt-multiturn/${TRACE_NAME}.meta.json`:
```json
{
  "model": "<MODEL>",
  "block_size": 16,
  "per_block_bytes": <2 * num_layers * num_kv_heads * head_dim * block_size * dtype_bytes>,
  "num_gpu_blocks": <from vLLM log>,
  "num_kv_heads": <from model config>,
  "head_dim": <from model config>,
  "num_layers": <from model config>,
  "dtype": "float16",
  "cpu_offload_bytes": <CPU_BYTES>,
  "max_model_len": <MAX_MODEL_LEN>,
  "max_num_seqs": <MAX_NUM_SEQS>
}
```

### Validation

```bash
python analyze_trace.py offloading_mgr_*.jsonl offloading_handler_*.jsonl
```

## replay mode

Replays a previously-captured trace against a storage backend, measuring throughput, latency, and hit rate. No GPU required.

If the backend argument is omitted or `all`, run replay against all three backends (cpu, fs, certus) sequentially, skipping any whose hardware is unavailable.

### Running

**cpu:**
```bash
python replay_offloading_traces.py \
  --trace traces/sharegpt-multiturn/500convs-64g \
  --connector cpu --num-blocks 32768 \
  --output-json results/replay_cpu.json
```

**fs:**
```bash
python replay_offloading_traces.py \
  --trace traces/sharegpt-multiturn/500convs-64g \
  --connector fs --num-blocks 32768 \
  --output-json results/replay_fs.json
```

**certus** (start SPDK server first):
```bash
python certus_server.py --data-pci 0000:61:00.0 --meta-pci 0000:62:00.0 &
python replay_offloading_traces.py \
  --trace traces/sharegpt-multiturn/500convs-64g \
  --connector certus --num-blocks 32768 \
  --output-json results/replay_certus.json
```

The `--trace` argument is a prefix — the script finds `<prefix>.mgr.jsonl[.gz]` and `<prefix>.handler.jsonl[.gz]` automatically.

### Key parameters

- `--trace` — trace prefix (e.g. `traces/sharegpt-multiturn/500convs-64g`).
- `--connector` — storage backend: `cpu`, `fs`, or `certus`.
- `--connector-args` — JSON dict of extra kwargs (e.g. `'{"root_dir": "/mnt/other", "per_block_bytes": 4194304}'`).
- `--num-blocks` — capacity of the offload tier in blocks. Compute from meta.json: `cpu_offload_bytes / per_block_bytes`.
- `--block-size` — tokens per block (default: 16, must match trace).

### Output

The `--output-json` file contains:
- `elapsed_s` — wall-clock time
- `throughput_gb_s` — effective bandwidth
- `hit_rate` — fraction of lookups that found offloaded data
- `evictions` — number of blocks evicted
- `latency_ms` — p50/p90/p99 per-op latencies
- Per-op counts and error counts

## Hardware notes

- FS-backend NVMe: PCI `0000:c4:00.0` (kernel nvme driver), mounted at `/mnt/fs-backend-bench` (XFS)
- Certus data SSD: PCI `0000:61:00.0` (vfio-pci)
- Certus metadata SSD: PCI `0000:62:00.0` (vfio-pci)
- All PCIe Gen 4 x4

## Troubleshooting

- If `run_multiturn_offloading.py` can't find `sharegpt_subset_5000.json`, run `prefilter_sharegpt_for_bench.py` first.
- If Certus replay shows 100% failures, verify SPDK devices are bound: `dpdk-devbind.py --status`
- If FS replay is slow, check XFS mount options (`noatime,nodiratime`) and that no other I/O is active on the device.
- CPU "121ms p50 latency" is a known measurement artifact (polling gap in the tracing layer, not real DMA time).
