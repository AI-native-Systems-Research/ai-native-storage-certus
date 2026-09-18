# Trace Data Formats and Tooling

This note catalogs the trace/dataset formats used across the Certus KV-offload and
inferencing benchmarks — **ShareGPT**, **WEKA (Certus "cc")**, **Mooncake**, and
**OTel** — how they differ, which tools consume them (upstream and Certus), and the
shape (context length, turns, size) of the concrete trace sources staged on
`/mnt/certus1`.

> Scope: these are *workload* traces (what to send the model) and *replay* traces
> (what the KV-offload manager/handler did). They are distinct from the
> `offloading_mgr_*.jsonl` / `offloading_handler_*.jsonl` connector recordings that
> `kv-offload-replay/replay_offloading_traces.py` produces and consumes — those are
> documented in [`kv-offload-replay/README.md`](kv-offload-replay/README.md#trace-formats).

---

## 1. The four trace types at a glance

| Type | Granularity | Multi-turn? | Carries prompt text? | Carries KV block ids? | Timing | Primary origin |
|---|---|---|---|---|---|---|
| **ShareGPT** | conversation (list of turns) | yes | **yes** (real chat text) | no | synthesized at replay | `anon8231489123/ShareGPT_Vicuna_unfiltered` |
| **WEKA (cc-trace)** | conversation (nested requests) | yes | no (token counts only) | **yes** (`hash_ids`, local scope) | recorded per-turn `t` + `api_time`/`ttft` | captured Claude API usage |
| **Mooncake** | request (one per line, flat) | no (turns flattened) | no (synthetic filler) | **yes** (`hash_ids`, global scope) | `timestamp` per request (replay-only) | guidellm deserializer format |
| **OTel** | conversation (one file, spans=turns) | yes | partial (tokenised filler) | no | recorded span `start/end_time` gaps | inference-perf `conversation_replay` |

### How they differ

- **ShareGPT** is the only format with *real* natural-language content. It is a JSON
  array of conversations, each `{"id", "conversations": [{"from": "human"|"gpt",
  "value": "..."}]}`. Prefix reuse across turns is real because the text is real; the
  benchmark drivers accumulate context turn-by-turn. No block ids, no arrival
  timestamps — arrival/think-time is *modeled* by the driver.

- **WEKA cc-trace** is a **nested** JSONL: one line = one conversation, with a
  `requests[]` array of per-turn records `{t, model, in, out, hash_ids, api_time,
  type, ttft}`. It carries **no prompt text** — only token counts (`in`/`out`) and
  per-turn KV `hash_ids` at `block_size=64`. `hash_id_scope` is **`local`** (block N of
  conversation A ≠ block N of conversation B), so only *intra*-conversation prefix
  reuse is meaningful. It preserves real recorded per-turn timing (`t`), model routing
  (opus/sonnet/haiku/fable across versions), TTFT and API latency.

- **Mooncake** is a **flat** JSONL — one request per line, columns
  `{timestamp, input_length, output_length, hash_ids}` — the format guidellm's
  `mooncake` deserializer expects. `hash_ids` are a **global** id space and the
  deserializer requires `ceil(input_length / block_size) == len(hash_ids)`. Prompt
  text is synthetic (Faker/filler sized to the token counts). Turn structure, model
  routing, and latency fields from the source are dropped — every row is an
  independent request. Arrival timing is only honored under `--profile kind=replay`.

- **OTel** is a directory of per-conversation JSON files (`conv_NNNNN.json`), each a
  trace envelope `{trace_id, span_count, collected_at, spans[]}` where **each span is
  one LLM turn** with `gen_ai.usage.{input,output}_tokens`,
  `gen_ai.request.max_tokens`, `start_time`/`end_time`, and `gen_ai.input.messages`
  (Qwen-tokenised random filler for user turns; short markers for assistant turns).
  Inter-turn think/tool-call delays are encoded as the **gap between spans'
  timestamps**, which the replay drivers reproduce as real wall-clock sleeps. It is
  fully synthetic and reproducible from `(seed, config)` via inference-perf, not
  captured from a live service.

### How a token-count-only trace exercises vLLM

WEKA cc-trace, Mooncake, and the OTel corpus carry **no real prompt text** — only
token *counts* and (for the two `hash_ids` formats) KV block ids. They do not test
*what* the model says; they test the **serving system** (scheduler, prefill/decode
compute, KV-cache, offload tiers), for which only sizes and reuse structure matter.
The drivers reconstruct a workload of the right shape in three steps:

1. **Token counts → a correctly-sized prompt.** Each request's `input_length` is
   turned into throwaway text that tokenizes to *exactly* that many tokens, so prefill
   does identically-sized work. guidellm/Mooncake
   (`trace_common.generate_token_ids`) generates Faker text, `encode()`s it, and
   truncates to the exact token count; the OTel generator (`trace_to_otel.gen_text`)
   decodes random Qwen vocab ids. `output_length` is then passed as `max_tokens` (EOS
   ignored) so **decode** runs exactly that many steps. Prefill FLOPs, decode steps,
   and KV footprint match the real request even though the content is meaningless.

2. **`hash_ids` → genuine KV-cache reuse** (the point of these formats for
   cache/offload work). Each `hash_id` names a `block_size`-token KV block; two
   requests sharing a `hash_id` are given the **same token block**, so vLLM's prefix
   cache and the offload manager see the *exact* reuse pattern the original workload
   had — hits, admissions, evictions — reconstructed without any real text. Scope is
   why `cc_trace_to_mooncake.py` remaps the cc-trace's **local** ids into disjoint
   **global** ranges: intra-conversation reuse is preserved, false cross-conversation
   sharing is not invented, and guidellm's `ceil(input_length/block_size) ==
   len(hash_ids)` invariant holds.

3. **Timestamps → arrival/inter-turn timing.** `timestamp` (Mooncake, under guidellm
   `--profile kind=replay`) or span `start/end_time` gaps (OTel, slept per
   conversation) reproduce *when* requests arrive, so queueing and concurrency match
   the workload instead of saturating the engine.

**Faithfully measured** (all depend on sizes + block reuse, not content): throughput
(tok/s, gen/s), TTFT, cache hit rate, offload-tier admission/eviction pressure,
latency percentiles, tier bandwidth. **Not measured:** output quality/correctness —
the generated text is random. For answer quality, use a real-text dataset (ShareGPT).

---

## 2. Conversion paths

```
                    sharegpt_to_trace.py          (ShareGPT → turn-level trace JSONL)
ShareGPT  ─────────────────────────────────────▶  {arrival_s, conv_id, turn_idx,
 (real text)                                        prompt_tokens[], output_len, is_agent}

                    cc_trace_to_mooncake.py       (nested → flat, local→global ids)
WEKA cc-trace  ────────────────────────────────▶  Mooncake  (guidellm-ready, one req/line)
 (nested, hash_ids local)                          --model / --max-context filters

                    trace-gen/gen_conversation_trace.py  →  trace_to_otel.py
inference-perf  ───────────────────────────────▶  OTel corpus (conv_NNNNN.json, spans=turns)
 conversation_replay plan                          Qwen-tokenised, 120k sliding cap
```

- **`kv-offload-replay/sharegpt_to_trace.py`** — builds a turn-level JSONL from
  ShareGPT_Vicuna_unfiltered (one record per *user* turn). Tokenizes with
  `NousResearch/Meta-Llama-3-8B`, accumulates prior turns into `prompt_tokens`, and
  *models* arrival (Poisson conversation starts + per-turn think-time/decode spacing).
  Knobs: `--num-conversations`, `--min-turns`, `--max-prompt-tokens` (default 8192),
  `--arrival-rate`, `--decode-tps`, `--think-time-s`.

- **`kv-offload-replay/cc_trace_to_mooncake.py`** — flattens the nested WEKA cc-trace
  to one Mooncake row per request, renames `t/in/out` → `timestamp/input_length/
  output_length`, and **remaps each conversation's local `hash_ids` into a disjoint
  global range** so intra-conversation reuse survives while false cross-conversation
  sharing is removed. `--model` keeps only rows for one Claude model; `--max-context`
  drops rows whose `in+out` exceeds the serving `--max-model-len` (the cc traces reach
  ~1M tokens; e.g. cap at 32768 for Qwen2.5-7B, or 131072 with YaRN).

- **`kv-offload-otel-replay/trace-gen/`** — two-stage OTel generator:
  `gen_conversation_trace.py` runs inference-perf's `ConversationReplayDataGenerator`
  and dumps a compact plan (`[input_tokens, output_tokens, tool_call_latency_sec]` per
  turn); `trace_to_otel.py` expands it into the `otel_1k`-style corpus (Qwen filler,
  timestamp gaps, 120k sliding cap). `generate-otel-corpus.sh` chains both with the
  shipped defaults (seed 42, 1000 convs, Qwen2.5-7B).

---

## 3. Tool / format support matrix

| Tool | ShareGPT | WEKA cc | Mooncake | OTel | Notes |
|---|:--:|:--:|:--:|:--:|---|
| **`kv-offload-replay`** (`run_multiturn_*`, `run_sharegpt_offloading.py`) | ✅ | via convert | via `vllm bench` | — | Certus. Multi-turn 450×12 + ShareGPT/long-doc-qa workloads across all 4 KV-offload backends. |
| **`kv-offload-otel-replay`** (`run_otel_replay.py`, `run_otel_async.py`, `run_otel_shmq_certus.py`) | — | — | — | ✅ | Certus. Timestamp-scheduled OTel replay; reproduces recorded inter-turn gaps. |
| **`replay_offloading_traces.py`** | (n/a) | (n/a) | (n/a) | (n/a) | Certus. Replays *manager/handler* connector traces, not workload traces. |
| **`profile_all.sh`** | ✅ | — | — | — | Certus. All-backends sweep; also `long-doc-qa` synthetic workload. |
| **guidellm** (`--profile kind=replay`, mooncake deserializer) | — | — | ✅ | — | Upstream. Consumes flat Mooncake JSONL; the target of `cc_trace_to_mooncake.py`. |
| **inference-perf** (`conversation_replay`, `otel_trace_replay`) | — | — | — | ✅ | Upstream. Generates and can replay the OTel corpus (`otel_replay_1k.yaml`). |
| **`vllm bench throughput`** | ✅ | — | random/sonnet/custom | — | Upstream. Native dataset options; tracing connectors are dataset-agnostic. |

**Certus KV-offload backends** exercised by the replay tools (same across ShareGPT and
OTel): `nooffload` (GPU-only baseline), `cpuoffload` (vLLM host-RAM
`OffloadingConnector`), `cputier` / tiered CPU+FS (vLLM 0.26 `TieringOffloadingSpec`,
CPU primary + `fs` disk secondary), and `certus-shmq` (host `certus-server` over SPDK
NVMe via the shmq client). Backend selection is by env (`OFFLOAD_MODE`,
`SECONDARY_TIER`, shmq `SHM_PATH`), not dataset.

---

## 4. Trace sources on `/mnt/certus1`

Measured shape of the staged trace/dataset files (context = `input+output` tokens per
turn/request unless noted).

### ShareGPT-family (real text, JSON array of conversations)

| File | Convs | Max turns/conv | Prompt cap | Notes |
|---|--:|--:|--:|---|
| `sharegpt_v3.json` | 94,145 | 54 (sampled) | — | Full ShareGPT V3 corpus. 670 MB. Source for `--min-turns/--max-turns` sharegpt workload and the 1×1 full-corpus mode. |
| `sharegpt_12turn_450.json` | 450 | ~25 | — | Baked 450-conversation subset behind the default 450×12 multi-turn workload. 4.2 MB. |
| `synth-1K-M50.json` | 1,000 | 146 | — | Synthetic deep multi-turn ("M50") stress set; drives the deep-context head-to-heads. 83 MB. |

*Turns are ShareGPT `conversations[]` entries (human+gpt); the driver replays human
turns and accumulates context. Effective context is bounded by the serving
`--max-model-len` / `--max-prompt-tokens`, not the file.*

### WEKA cc-trace (nested, token-count + hash-id, no text)

| File | Lines | Meaning | Block size | Scope | Max turns/conv | Max ctx (in+out) |
|---|--:|---|--:|---|--:|--:|
| `cc-traces-weka-062126.jsonl` | 393 | conversations | 64 | `local` | **3,052** | **996,579 tokens** |

- Models routed within the trace: `claude-opus-4-{6,7,8}`, `claude-sonnet-4-{5,6}`,
  `claude-haiku-4-5-20251001`, `claude-fable-5`.
- Reaches ~1M tokens of context — **must** be `--max-context`-capped before replay on
  any bounded-window server. 1.8 GB.

### Mooncake (flat, guidellm-ready — derived from the WEKA cc-trace)

| File | Requests | Max ctx (in+out) | Purpose |
|---|--:|--:|---|
| `cc-traces-weka-062126.mooncake-32k.jsonl` | 1,209 | 32,747 | Capped for a 32k window (e.g. Qwen2.5-7B base). 0.8 MB. |
| `cc-traces-weka-062126.mooncake-131k.jsonl` | 13,413 | 131,071 | Capped for a 131k window (YaRN). 153 MB. |

*Produced by `cc_trace_to_mooncake.py` with `--max-context 32768` / `131072`. Global
`hash_ids`; one request per line; `timestamp` honored only under guidellm replay.*

### OTel corpus (synthetic, spans=turns, Qwen-tokenised)

| Path | Files (convs) | Turns/conv (span_count) | Per-turn context | Window cap | Model |
|---|--:|--:|--:|--:|---|
| `inference-perf-syn-data/otel_1k/` | 1,000 | ~50 (e.g. 54) | up to ~120k input, ~1k output | 120,000 (sliding) | Qwen/Qwen2.5-7B-Instruct |

- One `conv_NNNNN.json` per conversation; each ~25 MB (Qwen filler text materialised).
  Full corpus ≈ 22 GB — **bind-mounted read-only**, never baked into images.
- Recorded inter-turn gaps up to ~2000–2800 s of think/tool-call delay per deep
  conversation, so `TIME_SCALE=1.0` runs are dominated by the longest conversation;
  use `TIME_SCALE=0` (saturating) + small `NUM_CONVS` for smoke tests.
- Reproducible from `(seed=42, conversation_replay_50turn.yaml)`.

### Qwen coder traces (Mooncake-like, per-turn hash-id)

| File | Lines | Format | Notes |
|---|--:|---|---|
| `qwen-traces/qwen_coder_blksz_16.jsonl` | 43,011 | `{chat_id, parent_chat_id, timestamp, input_length, output_length, type, turn, hash_ids}` | block_size 16, coder workload; `parent_chat_id`/`turn` express conversation threading. |
| `qwen-traces/out/coder_64s_mml32768.json`, `…_min8_mml32768.json` | — | prepared subsets | 64-session, max-model-len 32768 replay configs. |

---

## 5. Choosing a format

- Need **real content / true prefix reuse** and end-to-end token quality →
  **ShareGPT** (`sharegpt_v3.json`, or the baked 450×12 subset).
- Need **production-shaped Claude usage** (routing, TTFT, deep contexts, real per-turn
  timing) but no text → **WEKA cc-trace**; convert to **Mooncake** to drive guidellm.
- Driving **guidellm** rate/replay profiles → **Mooncake** (already global-id, flat).
- Need **reproducible synthetic multi-turn with real inter-turn timing** and
  tool-call delays for the OTel replay tools → **OTel** corpus.

For per-request/per-turn context limits, remember the serving window
(`--max-model-len` / `CONTEXT_CAP`) governs what actually reaches the model — the WEKA
and OTel sources both exceed common windows and are capped by the converters/drivers.
