#!/bin/bash
# smoke.sh — prove the running vLLM + LMCache stack stores KV to the SSD tier and
# serves a later prefix hit back FROM it, intact (rootless; run after ./run.sh).
#
#   1. basic:   /v1/models lists the model; a short completion succeeds
#   2. store:   PROMPTS distinct PROMPT_TOKENS-token prompts. Together they overflow
#               L1 (run.sh CPU_GB), so LRU drops the oldest chunks from RAM. Those
#               chunks are then only in the L2 files under DISK_DIR/l2
#   3. POST /reset_prefix_cache drops vLLM's GPU prefix cache (LMCache keeps its own)
#   4. replay:  prompt #0, the oldest and so evicted from L1. Pass if
#               - vllm:external_prefix_cache_hits rises by >= MIN_HIT_FRAC of its tokens
#               - the L2 block device reads >= MIN_READ_MB during the replay
#                 (O_DIRECT, so these reads bypass the page cache)
#               - the greedy output is token-identical to the cold run (KV came back
#                 intact — the check that catches the in-process connector's corruption)
#
#   ./run.sh CPU_GB=2 && ./smoke.sh              # defaults sized for a 2G L1
#   PROMPTS=24 PROMPT_TOKENS=12000 ./smoke.sh    # for a bigger L1
#
# Sizing: Qwen2.5-7B KV is 56 KiB/token, so a 2G L1 holds ≈ 37K tokens. The
# default 12 × 8000 = 96K tokens overflows it about 2.5×. If L1 still holds prompt
# #0, step 4 hits in RAM, the read check fails, and the message says to raise
# PROMPTS. Prompts are random token IDs (no tokenizer needed; exact lengths).
set -euo pipefail

PORT="${PORT:-8000}"
LMC_HTTP_PORT="${LMC_HTTP_PORT:-8080}"
PROMPTS="${PROMPTS:-12}"
PROMPT_TOKENS="${PROMPT_TOKENS:-8000}"
MIN_HIT_FRAC="${MIN_HIT_FRAC:-0.9}"
MIN_READ_MB="${MIN_READ_MB:-64}"
DISK_DIR="${DISK_DIR:-/cache/lmcache}"

curl -sf "http://127.0.0.1:${PORT}/health" >/dev/null || { echo "error: no vLLM on :${PORT} — ./run.sh first" >&2; exit 1; }
curl -sf "http://127.0.0.1:${LMC_HTTP_PORT}/status" >/dev/null || { echo "error: no LMCache server on :${LMC_HTTP_PORT}" >&2; exit 1; }

# Block device under DISK_DIR, for the read-bytes check (sector counts in /sys).
DEV=$(basename "$(readlink -f "$(findmnt -n -o SOURCE --target "$DISK_DIR")")")
[[ -r "/sys/class/block/${DEV}/stat" ]] || { echo "error: no block stats for $DISK_DIR ($DEV)" >&2; exit 1; }

python3 - "$PORT" "$LMC_HTTP_PORT" "$PROMPTS" "$PROMPT_TOKENS" "$MIN_HIT_FRAC" "$MIN_READ_MB" "$DISK_DIR/l2" "$DEV" <<'PY'
import json, os, random, sys, time, urllib.request

port, lmc_port, n_prompts, n_tok, min_frac, min_read_mb, l2_dir, dev = sys.argv[1:]
n_prompts, n_tok, min_frac, min_read_mb = int(n_prompts), int(n_tok), float(min_frac), float(min_read_mb)
base, lmc = f"http://127.0.0.1:{port}", f"http://127.0.0.1:{lmc_port}"

def http(url, body=None):
    req = urllib.request.Request(url, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"},
                                 method="GET" if body is None else "POST")
    with urllib.request.urlopen(req, timeout=600) as r:
        raw = r.read().decode()
    return json.loads(raw) if raw[:1] in "{[" else raw

def metric(url, name):
    return sum(float(l.rsplit(" ", 1)[1]) for l in http(url + "/metrics").splitlines()
               if l.startswith(name + "{") or l.startswith(name + " "))

def dev_read_bytes():
    with open(f"/sys/class/block/{dev}/stat") as f:
        return int(f.read().split()[2]) * 512

def l2_usage():
    n = size = 0
    for root, _, files in os.walk(l2_dir):
        for f in files:
            n += 1
            size += os.path.getsize(os.path.join(root, f))
    return n, size

def complete(prompt):
    t = time.time()
    r = http(base + "/v1/completions", {"model": model, "prompt": prompt, "max_tokens": 16, "temperature": 0})
    return r["choices"][0]["text"], r["usage"]["prompt_tokens"], time.time() - t

# 1. basic
model = http(base + "/v1/models")["data"][0]["id"]
text, _, dt = complete("The capital of France is")
print(f"[smoke] 1/4 basic: model={model} -> {text.strip()[:40]!r} ({dt:.2f}s)")

# 2. store (token-ID prompts, disjoint by seed)
prompts = [[random.Random(1000 + i).randrange(1000, 30000) for _ in range(n_tok)] for i in range(n_prompts)]
t0 = time.time()
cold = [complete(p) for p in prompts]
# L2 writes are asynchronous: wait for the file count to settle.
prev = -1
for _ in range(60):
    nf, nbytes = l2_usage()
    if nf == prev:
        break
    prev = nf
    time.sleep(2)
st = http(lmc + "/status")["storage_manager"]["l1_manager"]
print(f"[smoke] 2/4 store: {n_prompts} × {n_tok} tokens in {time.time()-t0:.1f}s -> "
      f"L2 {nf} files, {nbytes/2**30:.2f} GiB; L1 {st['memory_usage_ratio']:.0%} of {st['memory_total_bytes']/2**30:.0f} GiB")
if nf == 0:
    sys.exit("[smoke] FAIL: nothing written to L2 " + l2_dir)

# 3. drop GPU prefix cache only
print(f"[smoke] 3/4 reset GPU prefix cache: {http(base + '/reset_prefix_cache', {})}")

# 4. replay the oldest prompt
hits0 = metric(base, "vllm:external_prefix_cache_hits_total")
l2_hit0 = metric(lmc, "lmcache_mp_l2_prefetch_hit_chunks_total")
rd0 = dev_read_bytes()
text_w, ptok, dt_w = complete(prompts[0])
rd = (dev_read_bytes() - rd0) / 2**20
hits = metric(base, "vllm:external_prefix_cache_hits_total") - hits0
l2_hits = metric(lmc, "lmcache_mp_l2_prefetch_hit_chunks_total") - l2_hit0
text_c, _, dt_c = cold[0]
print(f"[smoke] 4/4 replay prompt 0: {int(hits)}/{ptok} tokens from LMCache ({int(l2_hits)} chunks from L2), "
      f"{rd:.0f} MiB read from {dev}, latency {dt_c:.2f}s cold -> {dt_w:.2f}s warm")

fails = []
if hits < min_frac * ptok:
    fails.append(f"external hits {int(hits)} < {min_frac:.0%} of {ptok}")
if rd < min_read_mb:
    fails.append(f"only {rd:.0f} MiB read from {dev} (< {min_read_mb:.0f}) — served from L1? raise PROMPTS")
if text_w != text_c:
    fails.append(f"output differs (KV corrupted?): cold {text_c!r} vs warm {text_w!r}")
if fails:
    sys.exit("[smoke] FAIL: " + "; ".join(fails))
print("[smoke] PASS")
PY
