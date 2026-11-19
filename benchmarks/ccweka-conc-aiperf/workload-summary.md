# Workload summary: cc-traces-weka-062126.trunc262k-out1

Properties of the trace replayed by the low-concurrency think-time sweep
(`ccweka-out1-think-lowconc-sweep-20261118_115054`, cap-32 in-flight point). Computed
directly from the 391 conversation JSONs on 2026-11-18.

## Sessions

- **391 conversations** — WEKA-format Claude Code sessions (recorded against
  claude-opus/haiku models), replayed against Qwen2.5-7B-Instruct-1M dense
  (262,144-token window, `/root/.cache/huggingface/qwen7b1m-dense`).
- All turns have **output length = 1 token** (the "out1" cut): the load is almost
  entirely prefill/prefix-reuse; decode is negligible.
- Request types: 21,330 root-session turns (`s`), 881 subagent, 138 nested (`n`).

## Turns per conversation

| p10 | p25 | p50 | p75 | p90 | p99 | max | mean |
|-----|-----|-----|-----|-----|-----|-----|------|
| 22  | 31  | 50  | 75  | 104 | 172 | 195 | 57.2 |

**22,349 total turns.** Turns are strictly sequential within a conversation — a
conversation has at most one turn in flight, so client in-flight ≤ conversation pool
size (391).

## Turn-interval (think time) distribution

Think time = the recorded idle gap before each turn (arrival `t` minus previous
turn's `t` + `api_time`) — exactly what aiperf replays under `--use-think-time-only`.

| p10  | p25  | p50  | p75  | p90   | p95   | p99    | mean  |
|------|------|------|------|-------|-------|--------|-------|
| 0.8s | 1.5s | 2.6s | 7.7s | 142s  | 232s  | 3,243s | 449s  |

- 87% of gaps > 1s; 23% > 10s; **14% > 1 min; ~3% > 10 min** — heavy-tailed
  (human think time).
- Per-conversation *cumulative* think time: p50 **3,323s (~55 min)**, p75 11,315s,
  p90 **59,600s (~16.6 h)**, p95 101,174s, mean 25,240s.
- Conversation span in the original trace (first to last request arrival): p50
  4,114s, p90 59,891s.

## Prompt lengths (per turn)

| p10  | p25  | p50   | p75    | p90    | p99    | mean   |
|------|------|-------|--------|--------|--------|--------|
| 45k  | 83k  | 135k  | 192k   | 232k   | 259k   | 135k tok |

Prefixes grow deep within a conversation; the mean 135k-token prompt is why external
KV reuse dominates (95%+ external prefix hit rate observed during the cap-32 run):
recomputing a 135k-token prefix per turn is ~100x more expensive than loading its KV
back from the tier.

## Implications for the experiment

- At an in-flight cap of 32, the active working set is ~32 conversations x deep
  prefixes, well over the 64G DRAM tier — producing session-switch churn (evictions)
  and, when reuse reaches a demoted entry, spilling pressure.
- Only **305/391 conversations could complete within 4h** of pure think time (p90
  conversation needs 16.6h), so the 4h `--benchmark-duration` backstop cuts each arm
  before the pool fully drains; the stop rule (in-flight < cap for 10 sustained
  minutes after a 15-min ramp) ends an arm earlier when ready work runs out.
- The think-time tail also explains brief `live < 32` dips: individual turns think
  for minutes at a time.

## Reproduction

```bash
python3 - <<'PY'
import json, glob
files = sorted(glob.glob('/mnt/certus1/cc-traces-weka-062126.trunc262k-out1/*.json'))
convs = [json.load(open(f)) for f in files]
for c in convs:
    rs = sorted(c['requests'], key=lambda r: r['t'])
    # per-turn think gap: t_i - t_{i-1} - api_time_{i-1} (0 for the first turn)
    # turns/conversation = len(rs); prompt lengths = r['in']
PY
```
