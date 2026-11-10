# Long-context 7B-1M KV-offload config (2× A100-40G)

Finalized parameters for the long-context KV-offload tiering test. Paired scripts:
`run-serve-cputier-7b1m-longctx.sh` (server) and `run-guidellm-synthetic-longctx.sh`
(client). Hardware: 2× NVIDIA A100-PCIE-40GB, vLLM 0.29.0 (`certus-offload:vllm0.29.0`).

```bash
./run-serve-cputier-7b1m-longctx.sh      # server: 7B-1M @ 524K, 16G GPU-KV cap
./run-guidellm-synthetic-longctx.sh      # client: 96 prefixes, syfi think-time, 1200s
```

## Model

| | |
|---|---|
| Checkpoint | **Qwen2.5-7B-Instruct-1M**, served as a local **dense repack** at `/mnt/certus1/hf-cache/qwen7b1m-dense` (`dual_chunk_attention_config` removed, no `sparse_attention_config.json`) |
| Architecture | 28 layers · 28 attn heads · **4 KV heads (GQA)** · head_dim 128 · ~7.6B params · fp16 (~15 GB, 4 shards) |
| Native context | `max_position_embeddings=1,010,000` via `rope_theta=1e7` (base window 262,144); **no YaRN** (`ROPE_YARN=0`) |
| KV footprint | **56 KB/token** (2·28·4·128·2 B) |
| Why repacked | vLLM 0.29.0's dual-chunk-attention (DCA) path crashes (`FlashAttentionImpl … unexpected kwarg 'layer_idx'`); the sparse config requires DCA (`verify_dual_chunk_attention_config` indexes `None`). Dense repack removes both triggers; loaded by local path so the hub cannot re-download them. |
| Quality caveat | Dense attention is quality-correct to the **native 262,144**; the 262 K–524 K band is admitted but degraded (DCA is what makes 262 K–1 M work). Synthetic conversations are ~41 K tokens, far under. |

## Server — `run-serve-cputier-7b1m-longctx.sh`

| Param | Value |
|---|---|
| Served model name | `qwen2.5-7b-1m` |
| `MAX_MODEL_LEN` | **524,288** |
| `KV_CACHE_BYTES` | **16 G / worker** → GPU KV pool **599,184 tokens**, max concurrency **1.14×** at 524 K (14 G = the floor, rejected; 16 G is the safe minimum) |
| Tensor parallel / GPUs | 2 / 2× A100-40G, `GPU_MEM_UTIL=0.9`, dtype fp16 |
| KV offload | vLLM `OffloadingConnector` → `TieringOffloadingSpec`: 32 G CPU primary tier + `fs` disk tier (NVMe), LRU eviction |
| Chunked prefill | `--max-num-batched-tokens 8192` |
| Block size | 16 |
| HF cache | `/mnt/certus1/hf-cache` |

## Client — `run-guidellm-synthetic-longctx.sh`

| Param | Value |
|---|---|
| Model / tokenizer | `qwen2.5-7b-1m` / `Qwen/Qwen2.5-7B-Instruct-1M` |
| Workload | `synthetic_text`, prompt 364 tok, output 1398 tok, **`TURNS=12`** (fixed; set `TURNS=` to sample from the syfi turn distribution) |
| Shared prefix | **`PREFIX_COUNT=96` × `PREFIX_TOKENS=20,000`** = ~1.92 M reused tokens (≈3.2× the 599 K GPU pool → forces tier spill) |
| Think-time | truncated normal from `~/certus/tools/kv-cache-dynamics/syfi-coding-intervals.yaml`: `delay≈47.8 s`, `stdev≈144.1`, `min=0.001`, `max=1493.4` (p99 cap); `delay_min>0` required (guidellm rejects 0) |
| Load profile | `concurrent`, `CONCURRENT_STREAMS=8`, `MAX_SECONDS=1200` |
| `CONTEXT_WINDOW` | 524,288 (informational — drives the overflow warning only) |

## Measured (240 s confirmation run, 2026-10-12)

| Metric | Value |
|---|---|
| GPU KV pool | 599,184 tokens @ 1.14× concurrency |
| **External (offload-read) prefix-cache hit rate** | **26.2%** (161,120 / 615,734) — vs **0%** uncapped |
| Bytes stored to tiers | ~29 GB |
| GPU prefix-cache hit rate | 65.4% |

Key rule: external hits require the **reused working set to exceed GPU KV**. Uncapped,
the 7B-1M's ~1 M-token pool holds the whole set (0% external); the 16 G cap + 96
prefixes oversubscribe it ~3.2× so cold prefixes evict and reload from the tiers.
