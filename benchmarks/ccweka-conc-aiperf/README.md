# ccweka-conc-aiperf — low-concurrency think-time CC-weka sweep

Self-contained bundle for the 1-output-token CC-weka trace replay under **recorded
think-time pacing at low in-flight concurrency**, comparing the **certus** (SHMQ +
SPDK NVMe tier) and **cputier** (vLLM native CPU/fs tiering) KV-offload backends.

First run: 2026-11-18, cap 32, results in
`/mnt/certus1/vllm-serve-matrix/ccweka-out1-think-lowconc-sweep-20261118_115054`
(plus `workload-summary.md`, `reuse-distance-summary.md`, `reuse-distance-sim.py`
in `/mnt/certus1/vllm-serve-matrix/`).

## What it measures

Conversations from a real Claude Code trace (`cc-traces-weka-062126`, truncated to
262k, all turns output 1 token) are replayed with each turn's **recorded think time**
(human idle gap between turns). At low in-flight caps the server keeps up, so
conversations run deep into their turn counts (p50 50 turns) and their accumulated
prefix KV — far larger than the 64G CPU tier — churns the tier via session
switching. This drives DRAM-tier eviction and DRAM→SSD spilling pressure, the
regime the sweep is designed to create.

- **Pool**: 391 original conversations, 22,349 turns, mean prompt 135k tokens,
  think-time p50 2.6s / p90 142s (heavy human tail). Details: `workload-summary.md`.
- **Slot semantics**: aiperf's `--concurrency` is a *session* slot held for a
  conversation's lifetime including think gaps, so a thinking conversation would
  block others. Instead all 391 sessions are admitted and `--prefill-concurrency
  N` (released at TTFT ≈ completion for out=1) caps in-flight requests.
- **Stop rule**: an arm ends when in-flight (vLLM running+waiting) stays below the
  cap for 10 minutes after a 15-minute ramp — i.e. when fewer than N conversations
  remain actively cycling. Backstop: 4h (`--benchmark-duration 14400`) per arm.
- **Watchdog**: the cputier arm runs under a wedge-capture watchdog (py-spy dumps +
  `WEDGED` marker + client stop, so the sweep continues past a wedged arm).

## Headline results (cap 32, first run)

| | certus | cputier |
|---|---|---|
| requests (4h) | 4,009 | ~600 pace (10x slower) |
| external tier hit | 94.2% | ~42% |
| computed share | 5.7% | ~59% |
| SSD traffic | ~19.6 TB each way (SPDK) | fs tier, 2 GB/s while active |

Both arms are in **full spill regime** at cap 32: the 64G DRAM tier turns over
every ~33s while conversations cycle every ~145s, so essentially every prefix
reload promotes SSD→DRAM→GPU at ~2 GB/s each direction. The certus tier keeps a 94%
lookup hit rate (DRAM+SSD union) and 0.42 req/s; cputier's inline-eviction tiering
serves only ~42% of lookups and pays ~10x in throughput.

The reuse-distance simulator (`/mnt/certus1/vllm-serve-matrix/reuse-distance-sim.py`)
reproduces this run within ~10–15% and shows the DRAM-hit cliff sits at **cap ≈
8–10** for a 64G tier (below that, hits recover; above it, the tier is pure staging
regardless of size up to 128G — 256G is needed at cap 32). If extending the sweep,
the interesting hardware points are caps **4, 8, 12**.

## Files

| file | role |
|---|---|
| `ccweka-out1-think-lowconc-sweep.sh` | **driver** — per-point loop over `STREAMS`, launches the sweep, stop rule, wedge watchdog, merges summaries |
| `stop-rule-live-watch.sh` | stop-rule watcher (waits out preflight, matches aiperf's retitled process, parses labeled vLLM metrics) |
| `run-gqa-iobound-stream-sweep.sh` | per-point harness: preflight (hugepages/NVMe), certus-server probe, invokes the matrix |
| `run-gqa-iobound-matrix.sh` | per-arm orchestrator: server launch, vLLM, client, teardown, report |
| `run-aiperf-weka-iobound.sh` | client wrapper: maps `TIMING=think` onto aiperf `--use-think-time-only` + pool-size `--concurrency` + `--prefill-concurrency` cap |
| `run-aiperf-weka.sh` | actual aiperf invocation |
| `run-cputier-wedge-capture.sh` | cputier wedge watchdog |
| `run-serve-certus-shmq-gqa-iobound.sh` / `run-serve-certus-shmq.sh` | certus vLLM serve scripts |
| `run-serve-cputier-gqa-iobound.sh` / `run-serve-cputier.sh` | cputier vLLM serve scripts |
| `poll-kv-metrics.sh` (+ `plot-gpu-vs-io.py`) | kvio.csv poller (2s metrics snapshots) |
| `gen-iobound-report.py` | per-arm report/summary generation |

The `*-nointervals` arm labels are inert — `CLIENT_SCRIPT` overrides the client for
every arm; both labels run the same think-time aiperf replay. The naming is
inherited from the `{certus,cputier}×{nointervals,intervals}` matrix grid.

## Re-running

Requirements: node with SPDK-built certus (server binary below), the 4 NVMe devices
free, hugepages, podman image with vLLM 0.30 + tiering patches, aiperf 0.12.0, and
the trace pool.

```bash
# this point (cap 32, both arms, 4h/arm backstop):
./ccweka-out1-think-lowconc-sweep.sh

# several caps (16/64/128 were the originally planned set; 4/8/12 straddle the
# DRAM-hit cliff per the simulator):
STREAMS="4 8 12" ./ccweka-out1-think-lowconc-sweep.sh

# detached:
nohup setsid ./ccweka-out1-think-lowconc-sweep.sh > /tmp/lowconc.log 2>&1 &
```

Results land in `TOP` (default
`/mnt/certus1/vllm-serve-matrix/ccweka-out1-think-lowconc-sweep-<timestamp>/`):
per-arm dirs with `aiperf/profile_export.jsonl`, `kvio.csv`, server/client logs,
`report.md`, per-point `summary.{md,csv}`, plus `driver.log`,
`<n>streams-stop-rule.log`, and `watchdog.log`.

Defaults (override via env): `CPU_TIER_GB=64`, `OFFLOAD_BLOCK_SIZE=64`,
`MAX_SECONDS=14400`, `MODEL=/root/.cache/huggingface/qwen7b1m-dense`,
`SERVED_MODEL_NAME=qwen2.5-7b-1m`, `MAX_MODEL_LEN=262144`,
`TRACE=/mnt/certus1/cc-traces-weka-062126.trunc262k-out1`,
`SERVER_BIN=/mnt/certus1/dwaddington-spdk-tier128/certus/target/release/certus-server-yaml`.

Notes:
- The driver is self-contained (scripts resolve siblings via `SCRIPT_DIR`); it does
  not need `benchmarks/vllm-serve` on the path.
- `certus-server-yaml` is NOT rebuilt by the sweep — rebuild it after switching
  branches before re-running.
- For caps above the 391-conversation pool, use the x3 sim-copy pool
  (`trunc262k-out1-x3`, 1173 conversations); in-flight can never exceed pool size.
- Post-run analysis: kvio.csv columns are
  `wall_s,load_time_s,store_time_s,load_bytes,store_bytes,prefill_sum_s,infer_sum_s,prompt_tok,gen_tok,computed_tok,local_hit_tok,ext_hit_tok,tier_read_s,tier_read_bytes`.
  On the certus backend `tier_read_bytes` is not wired (reads go via SPDK SHMQ);
  infer certus SSD traffic from `promotions/evictions` tier-events lines in
  `certus-server.log` instead.
