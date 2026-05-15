# Certus Benchmarking Plan

Two benchmark tiers serve different purposes: **trace replay** validates the storage policy layer in isolation (fast, no GPU needed), while **live serving** measures end-to-end user-facing latency under real scheduling (closed-loop, requires GPU + model).

## Replay vs Live Serving

| | Trace Replay (Tier 1) | Live Serving (Tier 2) |
|---|---|---|
| **Loop type** | Open-loop (fixed call sequence) | Closed-loop (scheduler reacts to cache state) |
| **What runs** | Just the policy layer | Full vLLM stack (model + scheduler + cache + IO) |
| **GPU needed?** | No (policy-only) / Yes (native IO) | Yes (running the model) |
| **Time to run** | Seconds (policy-only) to minutes (native IO) | Minutes to hours |
| **Answers** | "Given same keys, which policy wins?" | "What's the TTFT/throughput improvement for users?" |
| **Closed-loop feedback** | No — different decisions don't change future keys | Yes — faster loads → different scheduling → different eviction pressure |
| **Primary use** | Development, regression testing, evolution evaluator | Customer-facing benchmark |

---

## Tier 1: Trace Replay (Policy-Layer Benchmark)

**Location:** `benchmarks/kv-offload-replay/`

### What it does

Records the exact sequence of `lookup`, `touch`, `prepare_store`, `complete_store` calls that vLLM's scheduler issues to the offloading manager during a live run, then replays that sequence against alternative manager implementations.

### How traces are collected

1. `TracingOffloadingConnector` wraps vLLM's `OffloadingConnector` — intercepts every scheduler↔manager call
2. `TracingOffloadingManager` wraps the actual `OffloadingManager` — logs the 6 policy methods with full key lists as hex block hashes
3. `TracingOffloadingHandler` wraps each `OffloadingHandler` — logs data movement shape (direction, block count, timing)
4. Driver (`run_sharegpt_offloading.py`) runs `vllm bench throughput` with the tracing connectors attached

Output: `offloading_mgr_<pid>.jsonl` (manager-level, portable) + `offloading_handler_<pid>.jsonl` (handler-level)

### How replay works

`replay_offloading_traces.py` reads the JSONL trace and drives each call against a pluggable target:

| Target | What it exercises | Requirements |
|---|---|---|
| `simple-lru` | Pure-Python LRU baseline | None (stdlib only) |
| `cpu-manager` | vLLM's `CPUOffloadingManager` | vLLM |
| `certus-connector` (policy-only) | Certus tiered DRAM+NVMe manager, no IO | vLLM + certus_connector |
| `certus-connector` (native) | Full SPDK DMA to NVMe | vLLM + certus_native + SPDK + NVMe |
| `fs-backend` | llmd_fs_backend (POSIX filesystem) | vLLM + torch + CUDA + storage_offload |
| Custom `module:Class` | Any OffloadingManager-shaped object | User-provided |

### What it measures

| Metric | Source | Faithfully measured? |
|---|---|---|
| Cache hit rate | Each target's `lookup` return values | Yes |
| Admission/rejection count | `prepare_store` decisions | Yes |
| Eviction count and identity | `prepare_store` evicted keys | Yes |
| Manager-layer latency (p50/p95/p99) | Wall-clock per method during replay | Yes |
| Handler throughput (MB/s) | Real IO in native mode | Yes |
| End-to-end tokens/sec | N/A | **No** (open-loop) |

### Limitation: open-loop replay

The captured call sequence is fixed from the original execution. A different manager's different hit rates would have caused different prefills, different block hashes, and different eviction pressure downstream. The replay answers:

> "Given the same key stream, how does manager X compare to manager Y?"

It does NOT answer:

> "What would end-to-end serving throughput have been with manager X?"

For the second question, use Tier 2 (live serving).

### Role in the system

Trace replay is a **development tool**, not a benchmarking deliverable:
- Validate policy logic correctness (hit rate matches expectations)
- Catch regressions (policy change → worse eviction count)
- Prove raw IO speed advantage (9× over fs-backend on writes)
- Fast iteration without GPU (policy-only mode)
- Provide evaluator signal for evolution campaigns (§ Evolution Integration)

---

## Tier 2: Live Serving Benchmark (End-to-End)

### What it does

Runs `vllm serve` with a configured offloading backend, drives real multi-turn HTTP traffic against it, and measures user-facing latency metrics.

### Architecture

```
Load Generator (async HTTP client)
  │  sends multi-turn requests with accumulated context
  │  measures TTFT per token stream
  ▼
vllm serve (OpenAI-compatible API)
  │  scheduler makes its own lookup/store/evict decisions
  │  prefix matching → offload/reload → prefill → decode
  ▼
Offloading Backend (Certus / CPUOffloadingSpec / fs-backend)
```

### What it measures

| Metric | How | Why it matters |
|---|---|---|
| TTFT (Time to First Token) | `t_first_token - t_request_sent` | Lower TTFT on turn 2+ proves prefix cache hit → skipped prefill |
| TTFT by turn bucket | Group by turn 1 vs 2-5 vs 6-10 vs 11+ | Shows benefit accumulation with reuse depth |
| TPOT (Time per Output Token) | `(t_last - t_first) / output_tokens` | Should be similar across backends (decode is decode) |
| Throughput (tok/s) | Total output tokens / wall time | End-to-end capacity |
| Prefix cache hit rate | vLLM `/metrics` endpoint | Confirms offload→reload is actually happening |

### Backends to compare

| Backend | Configuration | What it proves |
|---|---|---|
| **Certus native** | OffloadingConnector + CertusOffloadingSpec (DRAM hot cache + NVMe cold tier) | Full Certus value: larger effective cache means fewer misses |
| **CPUOffloadingSpec** (vLLM built-in) | OffloadingConnector + CPUOffloadingSpec (DRAM-only offload tier) | Fair baseline — same scheduler code path, different storage tier |
| **No offloading** (GPU-only) | No kv_transfer_config — vLLM recomputes on eviction | Worst-case baseline: what happens without any offload tier |

### Existing tooling

The `llm-d-benchmark` repo (`util/experimental/multi-turn/production-trace-replay-qwen.py`) already implements a multi-turn serving benchmark:
- Replays Qwen production traces turn-by-turn with session context accumulation
- Uses `inference_perf` library for load generation and metrics collection
- Reports TTFT by turn bucket (Turn 1, Turns 2-5, Turns 6-10, Turns 11+)
- Sends raw `prompt_token_ids` to vLLM's Completion API

To benchmark Certus: start `vllm serve` with Certus configured, run the Qwen replay driver against it, repeat with CPUOffloadingSpec as baseline, compare TTFT by turn bucket.

---

## Datasets

### Comparison

| | Qwen Production Trace | ShareGPT | Shared-Prefix Synthetic | SWE-bench / ToolBench |
|---|---|---|---|---|
| **Source** | `qwen_traceA_blksz_16.jsonl` (Alibaba) | HuggingFace anon8231489123 | Generated by guidellm/inference-perf | HuggingFace princeton-nlp / sambanovasystems |
| **Access pattern** | Real multi-turn conversations, natural inter-arrival timing | Multi-turn chat (if replayed incrementally) | Single-turn, shared system prompt across group | Agentic: tool calls with growing shared context |
| **Prefix reuse** | High — sessions accumulate context across turns; block hashes encode real sharing structure | High (if replayed turn-by-turn) | Fixed — always the same N-token prefix per group | Very high — 10-50 calls per task sharing task prefix |
| **Eviction pressure** | Natural — many sessions interleave, old turns get evicted | Moderate (depends on GPU memory pressure) | Controlled — tune group count vs cache size | High — long contexts compete for GPU memory |
| **Context growth** | Grows per turn (turn 5 has 5× turn 1's prefix) | Grows per turn | Fixed (no growth) | Grows per tool call (accumulated results) |
| **Load path exercised?** | Yes — turn N reloads turns 1..N-1 from offload | Yes (if multi-turn) | Yes — request 2+ reloads shared prefix | Yes — tool call N reloads task context |
| **Realism** | Production traffic from Alibaba Qwen serving | Real conversations but synthetic replay timing | Synthetic but representative of RAG/chatbot | Real agentic tasks |
| **Best for** | Primary end-to-end benchmark; proves value of NVMe tier under real concurrency | Validating multi-turn replay driver | Quick sanity check; confirms prefix caching works at all | Proving Certus value for agentic workloads (killer use case) |
| **Limitation** | Alibaba-specific model/traffic; may not represent agentic patterns | Flat if not replayed incrementally | Doesn't stress tiering (small prefixes may fit in DRAM) | No published KV-level traces yet; requires live instrumented run |

### Qwen Production Trace (Primary)

The trace contains real production traffic from Alibaba's Qwen model serving. Each line encodes:

```json
{"timestamp": 1234.5, "hash_ids": [101, 202, 303, ...], "chat_id": 42, "parent_chat_id": 17, "output_length": 64, "turn": 3}
```

- `hash_ids` are block-level content hashes from Alibaba's production prefix cache — requests sharing hash_ids share KV cache blocks
- `parent_chat_id` links turns into sessions — the replay driver accumulates context across turns
- `timestamp` preserves original inter-arrival timing
- The replay driver converts each `hash_id` → 16 deterministic tokens, so shared hashes produce shared token prefixes

This is the most valuable dataset because it encodes real prefix-sharing structure without leaking user data. Sessions interleave naturally, creating realistic eviction pressure on the offload tier.

**Availability:** `wget https://github.com/alibaba-edu/qwen-bailian-usagetraces-anon/raw/refs/heads/main/qwen_traceA_blksz_16.jsonl`

### Shared-Prefix Synthetic (Smoke Test)

Generated by `guidellm` with parameters:

```yaml
prefix_tokens: 2048    # shared system prompt per group
prefix_count: 32       # distinct prefix groups
prompt_tokens: 256     # unique user question
output_tokens: 256
```

Useful for confirming prefix caching works at all. Not sufficient for proving NVMe tier value — 32 groups × 2048 tokens likely fits entirely in DRAM.

### SWE-bench / ToolBench (Agentic — Future)

Agentic workloads are Certus's killer use case: 10-50 LLM calls per task, each appending tool results to a growing shared context. This creates the access pattern where NVMe offloading provides maximum benefit:

- Long shared prefixes (thousands of tokens of file/task context)
- High reuse within a session (every tool call reloads the task prefix)
- Ephemeral leaves (tool results processed once, never reused)
- Bursty arrivals (parallel tool calls within one agent step)

No published KV-level traces exist yet. To produce them: run SWE-bench tasks through vLLM with TracingOffloadingConnector attached, or instrument a live agentic system.

---

## Evolution Integration

Trace replay serves as the **evaluator** for LLM-driven code evolution campaigns (see `adaptive-evolution-proposal.md`). The evolution loop:

1. LLM proposes a mutation to the eviction policy (Rust or Python)
2. Build + test (fast-fail on compile/test errors)
3. **Replay traces** against the mutated policy → score = `0.4 × hit_ratio + 0.3 × (1/p99) + 0.3 × ops_per_sec`
4. Score feeds back to the evolution framework (SkyDiscover, GEPA, K-Search, etc.)

Trace replay is ideal for this because:
- Fast feedback (~1s per evaluation in Python, ~30s with cargo bench)
- Deterministic (same trace = same score for same policy)
- Safe (LLM-generated code that panics doesn't crash the model server)
- Isolatable (policy layer only, no GPU/network/scheduling noise)

The live serving benchmark is used for **validation** after evolution converges — confirming that the evolved policy's trace-replay advantage translates to real TTFT improvement.

---

## Execution: Running a Live Serving Benchmark

### Prerequisites

- GPU box with model weights (e.g., Llama-3-8B)
- vLLM ≥ 0.20 installed
- For Certus runs: `certus_native` built, SPDK-bound NVMe
- `inference_perf` library (`pip install inference-perf`)
- Qwen trace file downloaded

### Steps

```bash
# 1. Start vllm serve with Certus backend
vllm serve NousResearch/Meta-Llama-3-8B \
    --max-model-len 4096 \
    --gpu-memory-utilization 0.85 \
    --kv-transfer-config '{
        "kv_connector": "OffloadingConnector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            "spec_name": "CertusOffloadingSpec",
            "spec_module_path": "certus_connector.spec",
            "use_native": true,
            "data_pci_addrs": ["0000:61:00.0"],
            "metadata_pci_addr": "0000:62:00.0",
            "slab_size_bytes": 131072,
            "dram_cache_bytes": 8589934592
        }
    }'

# 2. Run multi-turn benchmark
python production-trace-replay-qwen.py \
    --model-name NousResearch/Meta-Llama-3-8B \
    --base-url http://localhost:8000 \
    --trace-file qwen_traceA_blksz_16.jsonl \
    --limit 1000

# 3. Repeat with baseline (CPUOffloadingSpec — DRAM-only offload tier)
vllm serve NousResearch/Meta-Llama-3-8B \
    --max-model-len 4096 \
    --gpu-memory-utilization 0.85 \
    --kv-transfer-config '{
        "kv_connector": "OffloadingConnector",
        "kv_role": "kv_both",
        "kv_connector_extra_config": {
            "cpu_bytes_to_use": 8589934592
        }
    }'

# 4. Run same benchmark against baseline, compare TTFT by turn bucket
```

