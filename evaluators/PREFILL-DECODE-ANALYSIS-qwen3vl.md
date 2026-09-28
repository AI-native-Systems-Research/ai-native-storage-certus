# Prefill vs. Decode Analysis — Qwen3-VL-32B Multimodal KV-Offload

**Date:** 2026-09-28
**Model:** `Qwen/Qwen3-VL-32B-Instruct-FP8`, tensor-parallel across 2× A100 (40 GB),
`GPU_MEM_UTIL=0.80`, `MAX_MODEL_LEN=32768`
**Workload (guidellm, closed-loop):** `prompt_tokens=256, output_tokens=128,
prefix_tokens=2048, prefix_count=32, turns=100`, **1× 1080p synthetic image/request**,
32 concurrent streams.
**Backends measured:** certus-SHMQ and cputier (vLLM-native tiering). The per-phase
*time* profiling was run on cputier; the split is model/workload-driven and largely
backend-independent (the backend only changes how much prefill is served from cache).

---

## TL;DR

For this workload the two questions have **opposite answers**, and conflating them is
the main trap:

| Question | Answer | Magnitude |
|---|---|---|
| Where do the **FLOPs / compute** go? | **Prefill** | prompt:generation tokens ≈ **41 : 1** (this window), up to ~92:1 on long conversations |
| Where does the **per-request wall-clock time** go? | **Decode** | **~95 % decode**, ~5 % prefill (GPU phase time) |
| Is the GPU compute- or bandwidth-bound? | **Compute-bound** | SM util **81 %** mean vs. MEM-controller **34 %** |

**Prefill dominates the arithmetic; decode dominates the clock.** The image makes each
request enormous on the input side (~6 k vision tokens), so almost all matmul FLOPs are
prefill — but prefill runs as a fast burst (~1.1 s/req) while the 128-token decode is a
long serialized tail (~21 s/req under load).

---

## Two different questions

- **FLOPs / TFLOPS** — prefill is *compute-bound* (large GEMMs over all input tokens,
  O(n²) attention, plus the vision encoder). Decode is normally *bandwidth-bound* (one
  token/step, dominated by KV reads). Per token, the linear layers cost the same FLOPs
  either way (~2·params·token), so the FLOP ratio ≈ the **token ratio**.
- **Wall-clock time** — prefill is a short burst; decode is a long serialized tail whose
  per-request span is stretched further by time-sharing the GPU across 32 concurrent
  streams. Client **TTFT includes queue wait**, so it is *not* a measure of prefill
  compute.

---

## How to determine it (methods used)

1. **Analytic token ratio** — count input (prefill) vs. output (decode) tokens/request.
   For the linear layers this ≈ the FLOP ratio. (guidellm client log:
   *Request Token Statistics*.)
2. **Server-side phase time (authoritative)** — scrape the full vLLM `/metrics` before
   and after a load and diff the histograms:
   - `vllm:request_prefill_time_seconds_{sum,count}`
   - `vllm:request_decode_time_seconds_{sum,count}`
   - `vllm:prompt_tokens_total` vs. `vllm:generation_tokens_total`
   - `vllm:time_to_first_token_seconds_sum` (prefill **+ queue**, for contrast)
   > Note: the evaluator's captured `*.metrics.txt` is a *filtered* subset (KV-offload +
   > prefix-cache only). The phase-time histograms are on the **full** endpoint —
   > `curl -s localhost:8000/metrics`.
3. **Compute-vs-bandwidth character** — `nvidia-smi dmon -s u` samples SM (compute) vs.
   MEM-controller (bandwidth) utilization. (DCGM `dcgm-dmon` with
   `PROF_TENSOR_ACTIVE`/`PROF_DRAM_ACTIVE` is finer-grained but was not installed here.)
4. **Cache-offload accounting** — the vLLM engine log lines
   (`loggers.py`, every ~10 s) report *Avg prompt throughput*, *Avg generation
   throughput*, *External prefix cache hit rate*, and *MM cache hit rate*; the offload
   tier volume is in `vllm:kv_offload_{load,store}_{bytes,time}_total` and
   `vllm:prompt_tokens_by_source_total{source="external_kv_transfer"}`.

---

## Measured results

### 1. Token / FLOP split (prefill-dominated)

| Source | prefill : decode tokens |
|---|---|
| Client per-request (600 s runs) | **65 : 1** median, **92 : 1** mean |
| Full `/metrics`, 120 s controlled window | **41 : 1** (628,691 prompt vs. 15,227 gen) |
| GPU-*computed* throughput (serve-log integral) | cputier ~12 : 1, certus ~4.4 : 1 |

The single 1080p image is ~6 k tokens, so input ≫ the 128 generated. The GPU-computed
ratio is lower than the raw prompt ratio because most prefill is served from cache
(below); the 120 s window is lower than the 600 s mean because early-conversation
requests carry less accumulated history.

### 2. Prefill served from cache, not recomputed (steady-state avg, 3 reps each)

| | External-prefix (tier) hit | MM / vision-encoder cache hit |
|---|---|---|
| cputier | 68.8 % | 78.9 % |
| certus  | 75.3 % | 79.1 % |

~70–75 % of prefill KV blocks arrive as **tier loads (bandwidth)** rather than GPU
matmul, and ~79 % of vision-encoder work is reused (fixed-seed synthetic image). This is
the offload backends' core job: turn prefill compute into cache traffic. certus offloads
more prefill than cputier — a mechanistic reason it wins throughput (more GPU freed for
decode; its decode-token rate is higher).

### 3. GPU per-request phase time (authoritative — cputier, 120 s controlled run)

| Phase | Σ time | per request | share |
|---|---|---|---|
| `request_prefill_time` | 116.2 s / 106 reqs | **1.096 s** | **4.9 %** |
| `request_decode_time`  | 2,258.3 s / 106 reqs | **21.305 s** | **95.1 %** |
| `time_to_first_token` (prefill + queue) | — | 10.204 s | — |

Decode dominates per-request wall time. TTFT (~10.2 s) minus prefill compute (~1.1 s)
implies **~9 s of queue wait** per request before prefill even starts — so client TTFT is
mostly queueing, not prefill.

### 4. GPU utilization (nvidia-smi dmon, both A100s, during load)

| Metric | mean | max |
|---|---|---|
| SM / compute util | **81 %** | 100 % |
| MEM-controller util (bandwidth proxy) | **34 %** | 70 % |

SM ≫ MEM → the GPU is **compute-bound**, consistent with prefill GEMMs plus large-batch
decode (at concurrency ~10–32, decode batches up and stops being purely bandwidth-bound).

---

## Interpretation

- **Compute/energy budget → prefill.** Any FLOP or TFLOPS accounting is dominated by
  prefill (tens-to-one token ratio, plus attention and the vision encoder). Optimizations
  that pay off: prefix/KV-offload caching, vision-embedding reuse, chunked prefill,
  FP8/quantized prefill GEMMs.
- **Latency budget → decode.** Per-request wall time is ~95 % decode. What users feel is
  (a) queue wait before first token (~9 s here, a scheduling/concurrency effect) and
  (b) the 128-token decode tail (~21 s under load). Optimizations that pay off: raising
  decode batch efficiency, admission/concurrency control to cut queue wait, speculative
  decoding.
- **Backend effect.** certus converts more prefill into tier loads (75 % vs. 69 % tier
  hit) and sustains a higher decode-token rate — the throughput win (+~40 %) comes with a
  higher median TTFT (more admitted concurrency ⇒ longer queue).

## Caveats

- `request_decode_time_seconds` is **per-request wall time in the decode phase**, which
  under concurrency includes time-sharing the GPU — not pure decode GPU-kernel time. The
  95 %/5 % split is a *request-experienced* time split, not a GPU-kernel-time split.
- `nvidia-smi dmon` "MEM util" is memory-controller busy %, a coarse bandwidth proxy;
  DCGM `PROF_DRAM_ACTIVE` / `PROF_TENSOR_ACTIVE` would be more precise.
- Phase-time profiling was cputier-only; the split is model/workload-driven, but exact
  numbers shift with backend, concurrency, and conversation depth.

## Reproduce

```bash
# 1. Start a server (self-contained cputier is simplest)
cd apps/vllm-serve && PORT=8000 ./run-serve-cputier-multimodal-2gpu.sh

# 2. Before/after full-metrics + concurrent GPU sampling around a 120s load
curl -s localhost:8000/metrics > before.txt
nvidia-smi dmon -s u -d 1 -o T > dmon.txt &
PORT=8000 MAX_SECONDS=120 OUTPUT=prof.json ./run-guidellm-synthetic-multimodal.sh
kill %1; curl -s localhost:8000/metrics > after.txt

# 3. Diff the histograms
#   request_prefill_time_seconds_sum / request_decode_time_seconds_sum   -> time split
#   prompt_tokens_total / generation_tokens_total                        -> token/FLOP split
#   dmon SM% vs MEM%                                                     -> compute vs bandwidth
```

**Data:** `/mnt/certus1/certus-bench-archive/mm600-reps/` (6× 600 s runs) and
`/mnt/certus1/certus-bench-archive/profile/` (controlled profiling run: `metrics_before.txt`,
`metrics_after.txt`, `dmon.txt`, `client.log`).
