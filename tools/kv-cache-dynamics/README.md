# kv-cache-dynamics

Model KV-cache **reuse under a tiered (HBM → DRAM → SSD), block-level LRU
cache**, driven by **sampled turn-interval and turn-count distributions** rather
than fixed values.

A conversational serving system keeps prefix KV blocks in a cache of bounded
size and evicts least-recently-used blocks when it fills. Whether a returning
turn hits the cache depends on wall-clock recency: if the user paused only
briefly, their blocks are still resident; if they paused for a long tail
interval, other sessions' churn has pushed their blocks out and the turn pays a
prefill. Real inter-turn gaps span sub-second to tens of thousands of seconds,
and real conversation depths span 3 to 3000+ turns, so the achieved reuse rate
at a given capacity is set by the shape of those distributions — which this tool
samples directly.

## Model

- **Three-tier shared cache** of fixed-size KV blocks (`--block-size` tokens):
  HBM (`--hbm-gb`, default 4), DRAM (`--dram-gb`, default 32) and SSD
  (`--ssd-gb`, default `inf` = unbounded storage). Tiers are **exclusive** (a
  block lives in exactly one). A full tier evicts its LRU blocks by **demoting**
  them one tier down; blocks evicted from the bottom tier are dropped. A turn
  **promotes** the lower-tier blocks it reuses back into HBM. A size of `0`
  disables DRAM or SSD; `--dram-gb 0 --ssd-gb 0` is a plain HBM-only LRU cache.
- **Closed-loop load**: `--concurrent-sessions` conversations run at once over
  one timeline; each advances turn by turn with the gap between its turns drawn
  from the interval distribution. When one finishes, a fresh one starts, holding
  concurrency constant until `--sessions` total complete.
- **Admission policy** (`--admission`) decides whose turn runs next among the
  active sessions: `interval` (default) orders turns by the sampled think-time
  gaps on a wall-clock timeline; `round-robin` cycles through the active
  sessions in fixed slot order (each session's blocks are maximally stale when
  it returns — the LRU worst case); `random` picks an active session uniformly.
  Under `random`/`round-robin` the sampled intervals are not used; a finished
  session's slot goes to its replacement.
- **Turn count** per conversation is drawn from `--turn-distribution` (or a fixed
  `--num-turns` when that is unset).
- **Rolling context window**: with `--max-model-len` set, a conversation's live
  KV is the trailing `max_model_len` tokens — older blocks slide out, bounding
  its footprint (this is what keeps the trace's deep, ~1M-token conversations
  servable in a 131K window). Unset → unbounded monotonic growth.
- **Shared prefix**: with `--prefix-tokens N`, every conversation opens with one
  common prefix (system prompt, tool definitions). Its `N // block-size` full
  blocks are cached **once** and shared by all sessions; every turn touches them,
  so a session's *first* turn can hit them too. A partial trailing block is
  private (only full blocks are shareable). Under `--max-model-len` the prefix
  stays pinned at the head of the window; only the private context slides.
- **Cross-session sharing**: with `--shared-fraction F`, the leading fraction F
  of each conversation's live context (after the prefix) is identical across
  the sessions of its share group (`--share-groups G`, sessions dealt
  round-robin; default 1 = everyone), e.g. agents reading the same repository.
  It is cached once per group: a turn looks up its shared blocks in the group
  chain, so blocks it never computed still hit if a deeper peer cached them.
  Idealised as front-loaded content (real shared reads are interleaved with
  private turns), so it is an upper bound on prefix sharing. Because a group
  chain is one LRU entity, evicting its tail breaks the hash chain of every
  deeper member — sharing can *raise* misses when the group chains don't fit.
- **Decode emulation** (`--decode`): each turn prefills prior context + the
  prompt, then generates `--avg-gen-tokens` one token per step,
  `--decode-step-s` apart (default 30 ms). Steps are timed events interleaved
  with other sessions, and every step re-reads the whole live context; those
  reads are reported separately (*decode-stage lookups*), so prefill stats stay
  comparable. A context larger than HBM spills its tail on every step and pays
  lower-tier reads per token — real engines would need KV offload during
  attention, or preempt. `--decode-batch K` simulates K steps per event (K=1
  exact; K=16 is within ~2 points of exact on decode-read shares and ~12× faster).
  Interval admission only.
- **Prefill only**: only prompt + generated tokens per turn drives context
  growth, so a prefill-only, single-output-token workload is
  `--avg-prompt-tokens 1761 --avg-gen-tokens 1` (no `--decode`) — identical
  prefill stats to 364 + 1398 at the same 1,762 tokens/turn.
- **Block / prefix model**: a turn touches all of its session's live blocks, so
  they share one recency; eviction takes tail blocks of the tier's
  least-recently-used session first, so a session's chain is always laid out as
  `[HBM head][DRAM middle][SSD tail]`. Reuse follows the hash chain, read top
  tier first: private blocks are reusable only behind an intact shared prefix
  (in whatever tiers it sits). A return turn reuses whatever live blocks
  survived in any tier and recomputes the rest plus the new tokens; a session
  with nothing live resident pays a cold prefill. Beyond the shared prefix
  there is no cross-session sharing.

## Inputs

Two distribution YAMLs, both from `benchmarks/synthetic-gen-by-llm/`:

```bash
cd ../../benchmarks/synthetic-gen-by-llm
# Required: inter-arrival interval distribution
./extract_interval_distribution.py /mnt/certus1/cc-traces-weka-062126.jsonl -o intervals.yaml
# Optional: per-conversation turn-count distribution
./extract_turn_distribution.py     /mnt/certus1/cc-traces-weka-062126.jsonl -o turns.yaml
```

Both share the same histogram shape (`buckets` + `summary`); a bucket is chosen
weighted by its `fraction` and the value drawn **log-uniformly** within it (turn
counts are rounded to an integer ≥ 1). Pass `--seed` for reproducible runs.

Bundled, extracted from the cc-weka 062126 trace, so the tool runs out of the box:

| File | What | Summary |
|------|------|---------|
| `example-intervals.yaml` / `cc-trace-weka-062126-intervals.yaml` | inter-arrival intervals | median 16.9 s, p90 147 s, p99 3098 s |
| `cc-trace-weka-062126-turns.yaml` | turns per conversation | median 67, mean 149, max 3052 |
| `burstgpt-3-intervals.yaml` | inter-arrival intervals, BurstGPT v2.0 `BurstGPT_without_fails_3.csv` (55K chat sessions, 110 days) | median 131 s, p90 2342 s, p99 78825 s |
| `wildchat-intervals.yaml` | inter-arrival intervals, `allenai/WildChat-1M` (838K real human↔ChatGPT conversations) | median 128 s, p90 1253 s, p99 27511 s (~7.6 h), max ~48 days |
| `wildchat-turns.yaml` | turns per conversation, `allenai/WildChat-1M` | median 1, mean 2.3, p90 5, p99 14, max 249 |
| `syfi-coding-intervals.yaml` | inter-arrival intervals, `uw-syfi/TraceLab` v0.0.2 `syfi_coding_trace` (8058 real coding-agent sessions, 52 users) | median 10 s, p90 84 s, p99 1493 s (~25 min), max ~31.8 days |
| `syfi-coding-turns.yaml` | turns per conversation, `uw-syfi/TraceLab` v0.0.2 `syfi_coding_trace` | median 16, mean 83, p90 145, p99 1130, max 21351 |

BurstGPT has many sessions and long think times but short, shallow chats, so it
is used as a **mixed source**: its intervals with the cc-weka turn counts and
per-turn token sizes (`./run-burstgpt-example.sh`, a wrapper that runs `run-cc131k-example.sh` with
`DIST` pointed at the BurstGPT YAML; all its env overrides still apply).
Only the v2.0 `*_3.csv` files carry `Session ID`; regenerate with
`./extract_interval_distribution.py BurstGPT_without_fails_3.csv -o burstgpt-3-intervals.yaml`
(`.csv` input is detected as BurstGPT; `--format` overrides).

WildChat-1M is the only large, public, *human*, timestamped multi-turn chat trace,
so it supplies a **real human inter-turn think-time** tail (median 128 s sits
between cc-weka's 16.9 s and BurstGPT's 131 s, with a long p99 of ~7.6 h). In
WildChat only assistant turns carry a timestamp, so an interval is the gap
between consecutive assistant receipts (think-time + the next turn's generation),
and its chats are shallow (median 1 turn) — the consumer-chat contrast to
cc-weka's median 67. Like BurstGPT it works well as a mixed source: WildChat
intervals with cc-weka turn counts and per-turn token sizes. Regenerate (no HF
token needed) with:

```bash
cd ../../benchmarks/synthetic-gen-by-llm
hf download allenai/WildChat-1M --repo-type dataset --local-dir wc --include 'data/*.parquet'
./wildchat_to_ccweka.py 'wc/data/*.parquet' -o wildchat.jsonl
./extract_interval_distribution.py wildchat.jsonl -o wildchat-intervals.yaml
./extract_turn_distribution.py     wildchat.jsonl -o wildchat-turns.yaml
```

`syfi_coding_trace` (`uw-syfi/TraceLab` v0.0.2) is the **agentic-coding** analog to
WildChat's consumer chat: 665K agent steps across 8058 real coding-agent sessions
(Claude/Codex) from 52 users. Each agent step (a `round`) is one request keyed by
`session_id`, ordered by its earliest `timing_events` timestamp. Its conversations
are **deep** (median 16 rounds, p99 1130, max 21351 — the opposite extreme to
WildChat's median 1), and its intervals are **short** (median 10 s: rapid tool-call
loops) with a long human-gap tail (p99 ~25 min, max ~31.8 days). The tail of giant
sessions makes it the realistic stress case for intra-conversation KV reuse.
Regenerate from the public release (no auth needed; sha256
`11ce51ec…d4b4fb65`) with:

```bash
curl -L -o syfi_coding_trace.jsonl.gz \
  https://github.com/uw-syfi/TraceLab/releases/download/v0.0.2/syfi_coding_trace.jsonl.gz
./syfi_coding_dist.py syfi_coding_trace.jsonl.gz \
  --turns-out syfi-coding-turns.yaml --intervals-out syfi-coding-intervals.yaml
```

## Usage

```bash
pip install -r requirements.txt

# Default Llama-3-8B-class model, 4G HBM / 32G DRAM / unbounded SSD,
# fixed 5 turns, unbounded context
./kv_cache_dynamics.py example-intervals.yaml

# 4K-token shared system prompt, 1 TiB SSD
./kv_cache_dynamics.py example-intervals.yaml --prefix-tokens 4000 --ssd-gb 1024

# Single-tier 8 GiB HBM LRU cache (no offload)
./kv_cache_dynamics.py example-intervals.yaml --hbm-gb 8 --dram-gb 0 --ssd-gb 0

# Sampled turn counts + 131K rolling window
./kv_cache_dynamics.py cc-trace-weka-062126-intervals.yaml \
    --turn-distribution cc-trace-weka-062126-turns.yaml --max-model-len 131072

# FP8 weights, 16 GiB HBM, 128 concurrent users, 64-token blocks
./kv_cache_dynamics.py example-intervals.yaml --bytes-per-element 1 \
    --hbm-gb 16 --concurrent-sessions 128 --block-size 64 \
    --sessions 5000 --seed 1

# DRAM-tier sweep: how much of the HBM miss stream does DRAM absorb?
for gb in 0 8 16 32 64; do
  ./kv_cache_dynamics.py example-intervals.yaml --dram-gb $gb --seed 1 \
    | grep -A5 "TIER HIT RATES"
done
```

### Worked example: cc131k

`run-cc131k-example.sh` invokes the tool with the model attributes and cache
budget of the 131K cc-traces replay (Qwen2.5-14B-Instruct at a 131072-token
window on 2× A100-40G, TP=2), driven by **both** cc-traces-weka-062126
distributions — intervals *and* turn counts — with a 131K rolling window.
Defaults: 40 GiB HBM / 128 GiB DRAM / 2 TiB SSD (sized for the test node's
503 GB DRAM and 2.9 TB NVMe drives), 364 prompt + 1398 gen tokens per turn (the
trace's mean per-turn input growth and mean output), and an assumed 20K-token
shared prefix (the trace's hash ids are conversation-local, so sharing is not
observable in it). Every value is env-overridable:

```bash
./run-cc131k-example.sh                      # ~40 GiB HBM KV budget, 8 concurrent
HBM_GB=24 CONCURRENT=4 ./run-cc131k-example.sh
DRAM_GB=128 SSD_GB=1024 PREFIX_TOKENS=20000 ./run-cc131k-example.sh
TURN_DIST='' NUM_TURNS=20 ./run-cc131k-example.sh   # fixed turns instead
ADMISSION=round-robin ./run-cc131k-example.sh       # LRU worst-case ordering
PREFIX_TOKENS=0 SSD_GB=inf ./run-cc131k-example.sh  # no sharing, unbounded SSD
```

### Interactive dashboard (`dram_dashboard.py`)

A local web dashboard for the DRAM sweep that drives the real simulator. It
serves a page of controls and charts, and on each run invokes
`kv_cache_dynamics.py` (via `--json`) once per DRAM size with the SSD tier on
and once with it off, so every number matches the CLI exactly.

```bash
./dram_dashboard.py            # serve on http://127.0.0.1:8077 and open a browser
./dram_dashboard.py --port 9000 --no-browser
```

Stdlib only (no dependencies, localhost only). Change the workload — interval
trace, DRAM sizes, tiers, tokens, concurrency, admission, shared fraction,
decode — and re-run; the charts show per-tier hit shares, the SSD-vs-recompute
comparison, and (with decode on) decode-stage reads.

### DRAM sweep report: impact of SSD tiering

`sweep_dram_ssd.py` runs `run-burstgpt-example.sh` once per DRAM size (default
8, 16, 32, 64, 128, 256 GiB) plus once per size with SSD disabled, and writes a
self-contained HTML report (`reports/dram-sweep-report.html`): workload summary, the
interval and turn distributions, % of lookups served by HBM / DRAM / SSD vs DRAM
size, and SSD hits vs the no-SSD recompute share.

```bash
./sweep_dram_ssd.py                                   # HBM_GB=10 NUM_TURNS=100 SESSIONS=100
HBM_GB=20 CONCURRENT=64 ./sweep_dram_ssd.py -o c64.html
./sweep_dram_ssd.py --turn-dist cc-trace-weka-062126-turns.yaml --dram-sizes 0,16,64,256
```

`sweep_turns_ssd.py` is the same sweep along the other axis: turns per
conversation (default 8, 16, 32, 64, 128, 256; `--turn-counts` / env
`TURN_COUNTS`) with DRAM held fixed (`--dram-gb` / env `DRAM_GB`, default 32),
writing `reports/turns-sweep-report.html`. Deeper conversations grow a larger context, so
the working set outgrows HBM + DRAM and spills to SSD.

```bash
./sweep_turns_ssd.py
DRAM_GB=128 CONCURRENT=64 ./sweep_turns_ssd.py -o turns-c64.html
```

`sweep_concurrency_ssd.py` sweeps concurrent sessions (default 1, 2, 4, 8, 16,
32, 64, 128; `--concurrency` / env `CONCURRENCY_LEVELS`) with DRAM (32 GiB) and
turns (100) held fixed, writing `reports/concurrency-sweep-report.html`. `SESSIONS`
defaults to 512 here: the first concurrency wave is warmup, so the total must
exceed the largest level (the script refuses otherwise).

```bash
./sweep_concurrency_ssd.py
DRAM_GB=256 ./sweep_concurrency_ssd.py -o conc-dram256.html
```

Decode emulation runs through any of the sweeps via `DECODE=1` (with
`DECODE_STEP_S`, `DECODE_BATCH`); the report then adds a *decode-stage reads*
chart. For example `DECODE=1 DECODE_BATCH=16 ./sweep_concurrency_ssd.py`.

Reports go to `reports/` by default (`-o` overrides). Generated reports are
git-ignored; the bundled ones below were force-added:

| Report | Sweep | Settings (beyond the defaults) |
|--------|-------|--------------------------------|
| `dram-sweep-report.html` | DRAM 8–256 GiB | HBM 10G, 100 turns, 100 sessions |
| `dram-sweep-report-dram256.html` | DRAM 8–256 GiB | `DRAM_GB=256` (ignored: DRAM is swept; same as above) |
| `turns-sweep-report.html`, `-dram32` | turns 8–256 | DRAM 32G |
| `turns-sweep-report-dram256.html` | turns 8–256 | DRAM 256G |
| `concurrency-sweep-report.html` | concurrency 1–128 | DRAM 32G, 512 sessions |
| `*-tok1.html` | all three | 1 token added per turn (working set fits HBM) |
| `*-decode.html` | all three | `DECODE=1 DECODE_BATCH=16` |
| `*-prefill.html` | all three | prefill-only, 1 output token (`AVG_PROMPT_TOKENS=1761 AVG_GEN_TOKENS=1`); same numbers as the defaults |

For the DRAM sweep, `HBM_GB`, `NUM_TURNS`, `SESSIONS`, `TURN_DIST`, `DIST` and `DRAM_SIZES` are
overridable by env or flag; any other `run-cc131k-example.sh` variable passes
through from the environment. Turns are fixed (`NUM_TURNS`) unless
`--turn-dist` is given.

### Key options

| Flag | Default | Meaning |
|------|---------|---------|
| `--num-layers` / `--num-kv-heads` / `--head-dim` | 32 / 8 / 128 | Model geometry (Llama-3-8B class) |
| `--bytes-per-element` | 2 | 2 = BF16/FP16, 1 = FP8 |
| `--block-size` | 64 | KV block size in tokens |
| `--avg-prompt-tokens` / `--avg-gen-tokens` | 512 / 256 | Tokens added per turn |
| `--num-turns` | 5 | Turns per conversation (when `--turn-distribution` unset) |
| `--turn-distribution` | — | Turn-count YAML; sample turns/conv instead of `--num-turns` |
| `--max-model-len` | 0 | Context window (tokens); rolling window beyond it. 0 = unbounded |
| `--prefix-tokens` | 0 | Shared common-prefix tokens (system prompt/tools), cached once for all sessions |
| `--shared-fraction` / `--share-groups` | 0 / 1 | Leading fraction of live context shared within a group; number of groups |
| `--hbm-gb` (alias `--cache-capacity-gb`) | 4 | HBM tier capacity (GiB) |
| `--dram-gb` | 32 | DRAM tier capacity (GiB); 0 disables |
| `--ssd-gb` | inf | SSD tier capacity (GiB); `inf` = unbounded storage, 0 disables |
| `--concurrent-sessions` | 32 | Conversations active at once (held constant) |
| `--sessions` | 2000 | Total conversations to run through the cache |
| `--admission` | interval | Next-turn policy: `interval`, `random` or `round-robin` |
| `--decode` / `--decode-step-s` / `--decode-batch` | off / 0.03 / 1 | Decode emulation: one-token steps re-reading the context; seconds per step; steps per event |
| `--seed` | — | RNG seed for reproducibility |
| `--json PATH` | — | Also write config + raw stats as JSON (used by the sweep) |

## Output

- **Config** — bytes per token/block, each tier's capacity in blocks, context
  window / KV cap, shared prefix, turn-count source, and the working-set
  oversubscription ratio against HBM.
- **Cache reuse (LRU)** — mean simulated turns/conv (cross-check against the turn
  distribution), the full-hit / partial-hit / cold-miss mix across return turns,
  the token reuse rate (from any tier), the recompute (prefill) token volume,
  and (with a shared prefix) first-turn prefix reuse. A warmup of one
  concurrency wave is excluded from these stats.
- **Tier hit rates** — over all measured turns, the prior-context tokens served
  by each tier as a share of all lookups, plus the *local* hit rate (share of
  the lookups that reached that tier which it served), and the residual miss
  (recomputed).
- **Tier occupancy & movement** — per tier, mean (per-turn sampled) and peak
  blocks, blocks promoted from that tier into HBM, blocks demoted from it to
  the next tier down, and blocks dropped off the bottom tier. Movement counts
  cover the whole run, warmup included.

## Assumptions / limitations

- Cross-session sharing is the `--prefix-tokens` prefix plus, optionally, the
  `--shared-fraction` group context; everything else is private.
- Tiers are modelled for capacity and placement only — no transfer bandwidth or
  latency, so promotions are "free" and the tier hit rates are the output, not
  a TTFT estimate. Demotion is write-on-evict (exclusive); there is no
  write-through copy kept in a lower tier.
- A true rolling window matches sliding-window attention. A full-attention model
  (e.g. Qwen2.5) instead rejects/truncates over-window turns, so `--max-model-len`
  is best read as a per-conversation KV-footprint bound, not that model's exact
  reuse mechanics.
- All of a session's blocks share one recency (every turn reads the live window),
  so eviction drains the oldest session first; without a window, partial hits are
  rare by construction (a session is usually fully resident or fully evicted),
  whereas a rolling window makes partial hits common.
- Occupancy is sampled per turn, not time-integrated, to avoid the closed-loop
  drain tail (a lone long-gap session after others finish) distorting the mean.
- Intervals and turn counts are sampled i.i.d.; any per-conversation
  autocorrelation (or interval↔depth correlation) in the source trace is not
  reproduced.
