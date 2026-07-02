import os, sys, json, time
os.chdir("/home/bdh/kvconn-trace")

SUBSET_PATH = os.environ.get("DATASET_PATH", "sharegpt_12turn_450.json")
NUM_CONVS = int(os.environ.get("NUM_CONVS", 450))
MAX_MODEL_LEN = int(os.environ.get("MAX_MODEL_LEN", 8192))
OUTPUT_TOKENS = int(os.environ.get("OUTPUT_TOKENS", 150))
MAX_NUM_SEQS = int(os.environ.get("MAX_NUM_SEQS", 64))
GPU_MEM_UTIL = float(os.environ.get("GPU_MEM_UTIL", 0.90))
MODEL = os.environ.get("MODEL", "NousResearch/Meta-Llama-3-8B")
DRAM = int(os.environ.get("DRAM", 8589934592))
PROMPT_BUDGET = MAX_MODEL_LEN - OUTPUT_TOKENS

KV_CONFIG = {
    "kv_connector": "OffloadingConnector",
    "kv_role": "kv_both",
    "kv_connector_extra_config": {
        "spec_name": "SharedStorageOffloadingSpec",
        "spec_module_path": "llmd_fs_backend.spec",
        "shared_storage_path": "/mnt/fs-backend-bench/shared-kv",
        "max_staging_memory_gb": DRAM // (1024**3),
        "threads_per_gpu": 64,
    },
}

t0 = time.perf_counter()
print(f"[trace] +0.0s loading dataset", file=sys.stderr, flush=True)
with open(SUBSET_PATH) as f:
    all_data = json.load(f)
convs = []
for entry in all_data:
    if len(convs) >= NUM_CONVS:
        break
    turns = entry.get("conversations", [])
    human_turns = [t["value"] for t in turns if t.get("from") == "human"]
    if len(human_turns) >= 2:
        convs.append(human_turns)
print(f"[trace] +{time.perf_counter()-t0:.1f}s loaded {len(convs)} conversations", file=sys.stderr, flush=True)

print(f"[trace] +{time.perf_counter()-t0:.1f}s importing vllm", file=sys.stderr, flush=True)
from vllm import LLM, SamplingParams

print(f"[trace] +{time.perf_counter()-t0:.1f}s creating LLM", file=sys.stderr, flush=True)
llm = LLM(
    model=MODEL,
    max_model_len=MAX_MODEL_LEN,
    max_num_seqs=MAX_NUM_SEQS,
    gpu_memory_utilization=GPU_MEM_UTIL,
    dtype="float16",
    enable_prefix_caching=True,
    enforce_eager=True,
    kv_transfer_config=KV_CONFIG,
    disable_log_stats=True,
)
print(f"[trace] +{time.perf_counter()-t0:.1f}s LLM ready", file=sys.stderr, flush=True)

sp = SamplingParams(temperature=0.7, top_p=0.95, max_tokens=OUTPUT_TOKENS)
tokenizer = llm.get_tokenizer()

def n_tokens(text):
    return len(tokenizer.encode(text))

alive = [True] * len(convs)
next_turn = [0] * len(convs)
contexts = [""] * len(convs)
total_generations = 0
rounds_done = 0
t_start = time.perf_counter()
print(f"[trace] +{time.perf_counter()-t0:.1f}s entering generate loop", file=sys.stderr, flush=True)

while True:
    active_idx = []
    active_prompts = []
    for i, conv in enumerate(convs):
        if not alive[i]:
            continue
        k = next_turn[i]
        if k >= len(conv):
            alive[i] = False
            continue
        human = conv[k]
        candidate = human if k == 0 else contexts[i] + "\n\n" + human
        if n_tokens(candidate) > PROMPT_BUDGET:
            alive[i] = False
            continue
        contexts[i] = candidate
        active_idx.append(i)
        active_prompts.append(candidate)

    if not active_prompts:
        break

    rounds_done += 1
    print(f"[trace] +{time.perf_counter()-t0:.1f}s calling generate round {rounds_done} ({len(active_prompts)} prompts)", file=sys.stderr, flush=True)
    round_start = time.perf_counter()
    outs = llm.generate(active_prompts, sp)
    for i, out in zip(active_idx, outs):
        response = out.outputs[0].text if out.outputs else ""
        contexts[i] = contexts[i] + response
        next_turn[i] += 1
    total_generations += len(active_prompts)
    round_elapsed = time.perf_counter() - round_start
    n_alive = sum(alive)
    print(f"[run] round {rounds_done}: {len(active_prompts)} prompts in "
          f"{round_elapsed:.1f}s  ({n_alive} convs still alive)",
          file=sys.stderr, flush=True)

elapsed = time.perf_counter() - t_start
print(f"\n[run] done. wall={elapsed:.1f}s  generations={total_generations} rounds={rounds_done}",
      file=sys.stderr)

import sys
try:
    from prometheus_client import REGISTRY
    print("\n[PROM] vLLM Offloading Metrics:", file=sys.stderr)
    for metric in REGISTRY.collect():
        if "kv_offload" in metric.name:
            for sample in metric.samples:
                if sample.value > 0:
                    if "_bucket" in sample.name:
                        continue
                    print(f"[PROM] {sample.name} {sample.labels} = {sample.value}", file=sys.stderr)
except Exception as e:
    print(f"[PROM] error: {e}", file=sys.stderr)

