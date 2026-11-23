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
- **Watchdog**: every arm runs under a wedge-capture watchdog (0 running, waiting > 0,
  no completion for 300 s → py-spy dumps + `WEDGED` marker + client stop, so the
  sweep continues past a wedged arm).

## Results on the patched scheduler (2026-11-21) — current

Stock vLLM 0.30 deadlocks under this workload: a request whose async KV load has
finished can wait forever behind a queue head that cannot be admitted, because the
admission reserves blocks for loads that already finished (0 running, N waiting,
engine sleep-polling; diagnosed from SIGUSR2 state dumps). Unpatched, both backends
wedged at caps 8 and 16 and cputier also at 32 and 64, so **every number from the
2026-11-18/19/20 unpatched sweeps, including the first-run headline below, is
invalid**. The fix (move finished-load requests to the front of the waiting queue)
lives on its own branch and is baked into the v0.30 image builds there; see
`benchmarks/kv-offload-replay/vllm-fix-kvload-hol/PROVENANCE.md` on that branch.

All 20 patched arms ran their full 3600 s with no stall (64G DRAM tier both):

| cap | certus req/s | cputier req/s | certus lead | TTFT p50 certus / cputier | tier hit certus / cputier |
|---|---|---|---|---|---|
| 4 | 0.65 | 0.60 | +8% | 4.0 / 4.5 s | 93.2 / 93.0% |
| 8 | 0.62 | 0.55 | +13% | 9.9 / 11.4 s | 93.5 / 92.7% |
| 16 | 0.56 | 0.43 | +30% | 24.4 / 31.5 s | 92.8 / 90.5% |
| 32 | 0.58 | 0.37 | +57% | 51.3 / 83.3 s | 92.4 / 85.6% |
| 64 | 0.56 | 0.38 | +47% | 96.2 / 152.0 s | 90.0 / 83.0% |

Full table with sources: `/mnt/certus1/vllm-serve-matrix/ccweka-lowconc-patched-combined.md`.
Charts: `plot-lowconc-patched.py` (PNG beside the combined table). Interactive reports live with the data:
`ccweka-lowconc-patched-20261120_144612/lowconc-sweep.html` (think time) and
`ccweka-lowconc-delaycap600-20261122_191629/lowconc-delaycap-sweep.html` (10-minute gap cap, with a switch
to the think-time results), both under `/mnt/certus1/vllm-serve-matrix/`.
TTFT is mostly vLLM queue time (≈ cap ÷ req/s); the residual "errored" counts are
client-side keep-alive reuse failures that never reach vLLM.

## Headline results (cap 32, first run) — superseded, invalid (unpatched deadlock)

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
| `workload-summary.md` | properties of the replayed trace (sessions, turn counts, think-time and prompt-length distributions) |
| `stop-rule-live-watch.sh` | stop-rule watcher (waits out preflight, matches aiperf's retitled process, parses labeled vLLM metrics) |
| `run-gqa-iobound-stream-sweep.sh` | per-point harness: preflight (hugepages/NVMe), certus-server probe, invokes the matrix |
| `run-gqa-iobound-matrix.sh` | per-arm orchestrator: server launch, vLLM, client, teardown, report |
| `run-aiperf-weka-iobound.sh` | client wrapper: for every timing except `asap`, admits the whole pool as sessions (`--concurrency` = pool size) and caps in-flight requests with `--prefill-concurrency` |
| `run-aiperf-weka.sh` | actual aiperf invocation; maps `TIMING` onto aiperf's delay flags (see *Timing modes*) |
| `wedge-watch-live.sh` | attach-mode wedge watchdog the driver uses — matches aiperf's retitled `system_controller` process by name; watches every arm in turn, reads the arm name from `orchestrator.log`, finds the container in either podman store, and with `DEBUG_DUMP=1` takes two SIGUSR2 state dumps before py-spy |
| `diag/sitecustomize.py` | opt-in engine state-dump hook: mounted at `/opt/certus-debug` and put first on `PYTHONPATH`, it dumps the scheduler's waiting queue, connector jobs, tier lookups and GPU copy queues on SIGUSR2 (never signal an engine without it — SIGUSR2's default action kills the process) |
| `plot-lowconc-patched.py` | PNG chart of the patched results, drawn from the combined table |
| `run-cputier-wedge-capture.sh` | standalone variant: re-runs one arm itself under the watchdog (its attach mode matches the client by results-dir in its cmdline, which the retitled aiperf process no longer carries — use `wedge-watch-live.sh` to watch a live run) |
| `run-serve-certus-shmq-gqa-iobound.sh` / `run-serve-certus-shmq.sh` | certus vLLM serve scripts |
| `run-serve-cputier-gqa-iobound.sh` / `run-serve-cputier.sh` | cputier vLLM serve scripts |
| `poll-kv-metrics.sh` (+ `plot-gpu-vs-io.py`) | kvio.csv poller (2s metrics snapshots) |
| `gen-iobound-report.py` | per-arm report/summary generation |

The `*-nointervals` arm labels are inert — `CLIENT_SCRIPT` overrides the client for
every arm, so both labels run the same aiperf CC-weka replay; the pacing comes from
`TIMING`, not the label. The naming is inherited from the
`{certus,cputier}×{nointervals,intervals}` matrix grid.

## Running the sweep

### 1. Prerequisites

Check each of these on the node before starting; the sweep does not build or fix any of them.

| what | check / how |
|---|---|
| GPU free | `nvidia-smi` shows ~0 MiB used; nothing listening on port 8000 (`ss -ltn \| grep :8000`) |
| NVMe devices free | the 4 data drives (`0000:61:00.0`–`0000:64:00.0`) are not held by a stale `certus-server-yaml` (`pgrep -x certus-server-yaml` prints nothing); preflight binds them |
| hugepages | node 0 has at least `CPU_TIER_GB + 4` free 1 GiB hugepages (preflight checks and logs it) |
| certus server | `SERVER_BIN` exists and was built from the branch you mean to test: `cargo build --release -p certus-server-yaml` (the sweep never rebuilds it) |
| vLLM images | `certus-offload:vllm0.30.0` (default podman store) and `certus-shmq-connector:vllm0.30.0` (`/mnt/certus1/podman` store), **with the scheduler fix** (step 2) |
| client | aiperf 0.12.0 on `PATH` (`aiperf --version`) |
| trace | `TRACE` directory with the 391 conversation JSON files (`trunc262k-out1`) |
| model | `/mnt/certus1/hf-cache/qwen7b1m-dense` on the host (the dense repack of Qwen2.5-7B-1M), seen as `/root/.cache/huggingface/qwen7b1m-dense` through the serve scripts' `HF_CACHE` mount |

### 2. Use a vLLM 0.30 with the scheduler fix

**Stock vLLM 0.30.0 deadlocks under this workload** (0 running, N waiting, forever), so
results from stock images are invalid. Either:

- **rebuild the images with the fix baked in** (the fix is in
  `benchmarks/kv-offload-replay/vllm-fix-kvload-hol/`, added by PR #600; once that is
  on your branch, the builds apply it by default):

  ```bash
  ../vllm-serve/rebuild-bench-containers.sh          # shmq + cputier on vLLM 0.30.0
  podman image inspect certus-offload:vllm0.30.0 --format '{{index .Labels "org.certus.vllm-fix-kvload-hol"}}'   # -> 1
  ```

- **or mount the patched scheduler over the current images** with `EXTRA_PODMAN_ARGS`
  (step 4). The patched file is
  `benchmarks/kv-offload-replay/vllm-fix-kvload-hol/scheduler.py` (PR #600); a copy is kept at
  `/mnt/certus1/vllm-serve-matrix/vllm-fix-kvload-hol/scheduler.py`
  (md5 `a01271366fcb7ec89237c795b8565151`).

Either way, confirm it in a running container:
`podman exec <id> md5sum /usr/local/lib/python3.12/dist-packages/vllm/v1/core/sched/scheduler.py`
must print `a01271366fcb…` (stock is `849cc72e…`). The server log shows
`[certus-fix] moved …` lines while it is working.

### 3. Launch

Run from this directory, detached, so it survives the terminal (a full 5-cap,
2-arm sweep takes about 12.5 hours):

```bash
cd benchmarks/ccweka-conc-aiperf
TOP=/mnt/certus1/vllm-serve-matrix/ccweka-lowconc-$(date +%Y%m%d_%H%M%S)
STREAMS="4 8 16 32 64" ARMS="certus-nointervals cputier-nointervals" \
MAX_SECONDS=3600 COOLDOWN_SECONDS=600 TIMING=think TOP=$TOP \
  nohup setsid ./ccweka-out1-think-lowconc-sweep.sh > /tmp/lowconc-launch.log 2>&1 < /dev/null &
```

The first line of `$TOP/driver.log` records the caps, timing, branch and commit.
Each point runs its arms in `ARMS` order with a `COOLDOWN_SECONDS` pause between arms
and between points.

### 4. Knobs (environment variables)

| variable | default | meaning |
|---|---|---|
| `STREAMS` | `32` | space-separated in-flight caps, one point each |
| `ARMS` | `certus-nointervals cputier-nointervals` | backends to run at each point, in order |
| `TIMING` | `think` | inter-turn pacing; see *Timing modes* |
| `DELAY_CAP` | `30` | seconds; only used by `TIMING=delaycap` |
| `MAX_SECONDS` | `14400` | per-arm length (aiperf `--benchmark-duration`); `3600` was used for the published results |
| `COOLDOWN_SECONDS` | `600` | pause between arms and between points (`0` disables) |
| `CPU_TIER_GB` | `64` | DRAM tier size for both backends |
| `OFFLOAD_BLOCK_SIZE` | `64` | offload block size in tokens |
| `TRACE` | `/mnt/certus1/cc-traces-weka-062126.trunc262k-out1` | conversation pool; use `…-out1-x3` (1,173 convs) for caps above 391 |
| `TOP` | `/mnt/certus1/vllm-serve-matrix/ccweka-out1-think-lowconc-sweep-<ts>` | results directory |
| `SERVER_BIN` | `/mnt/certus1/dwaddington-spdk-tier128/certus/target/release/certus-server-yaml` | certus server binary |
| `MODEL`, `SERVED_MODEL_NAME`, `MAX_MODEL_LEN` | `qwen7b1m-dense`, `qwen2.5-7b-1m`, `262144` | model served by both backends |
| `STALL_SECONDS` | `300` | watchdog: no completion this long with 0 running = wedge |
| `EXTRA_PODMAN_ARGS` | empty | extra `podman run` flags for both serve scripts (word-split) |
| `DEBUG_DUMP` | `0` | `1`: on a wedge, take two engine state dumps before py-spy (needs the `diag/` hook mounted) |

Mounting the patched scheduler and the diagnostics hook on current images:

```bash
B=$PWD; FIX=/mnt/certus1/vllm-serve-matrix/vllm-fix-kvload-hol/scheduler.py
mkdir -p $TOP/dumps
DEBUG_DUMP=1 EXTRA_PODMAN_ARGS="\
 -v $FIX:/usr/local/lib/python3.12/dist-packages/vllm/v1/core/sched/scheduler.py:ro,z \
 -v $B/diag:/opt/certus-debug:ro,z -v $TOP/dumps:/opt/certus-debug-out:z \
 -e PYTHONPATH=/opt/certus-debug:/workspace/bench:/workspace/benchmarks/kv-offload-replay \
 -e CERTUS_DEBUG_OUT=/opt/certus-debug-out" \
 STREAMS=... TOP=$TOP ./ccweka-out1-think-lowconc-sweep.sh
```

The `PYTHONPATH` keeps both images' own entries (`/workspace/bench` for cputier,
`/workspace/benchmarks/kv-offload-replay` for certus). Never set `DEBUG_DUMP=1`
without the hook mounted: the watchdog checks for it, because SIGUSR2's default
action kills the engine.

### 5. Timing modes

`TIMING` picks how long each conversation waits between turns. Every mode except
`asap` admits all 391 conversations and caps requests in flight at the point's
`STREAMS` value.

| `TIMING` | aiperf flags | gap before each turn |
|---|---|---|
| `think` | `--no-fixed-schedule --use-think-time-only` | the recorded think time only |
| `intervals` | `--no-fixed-schedule` | the full recorded inter-request gap (`t_curr − t_prev`: original response time + think time), uncapped |
| `delaycap` | `--no-fixed-schedule --inter-turn-delay-cap-seconds $DELAY_CAP` | the full recorded gap, clamped to `DELAY_CAP` |
| `asap` | `--no-fixed-schedule --ignore-trace-delays` | none (back to back); `--concurrency` = cap |

`intervals` and `delaycap` replay a different gap than `think`, so they send a
different mix of request sizes; compare modes in tokens/s as well as req/s.
With long gaps (`intervals`, large `DELAY_CAP`) fewer conversations may be ready at
high caps, and the stop rule can end an arm early (step 6).

### 6. Monitoring

| where | what to look for |
|---|---|
| `$TOP/driver.log` | point start/done, cooldowns, `sweep finished; wedged arms: N` |
| `$TOP/<n>streams/<n>streams/orchestrator.log` | per arm: `===== ARM …`, `vLLM ready`, `arm … done` |
| `$TOP/<n>streams-stop-rule.log` | `elapsed=… live=… cap=…` every 15 s; `STOP-RULE:` when it ends an arm |
| `$TOP/<n>streams/<n>streams/watchdog.log` | `no stall detected while the <arm> client ran`, or `STALL` + capture |
| `$TOP/<n>streams/<n>streams/<arm>/WEDGED` | present only if that arm wedged; its results are invalid |
| `curl -s http://<host>:8000/metrics` | `vllm:num_requests_running`, `…_waiting`, `vllm:request_success_total` rising |

The stop rule ends an arm when requests in flight stay below the cap for 600 s after
a 900 s ramp (both fixed in `stop-rule-live-watch.sh`); otherwise `MAX_SECONDS` ends
it. A brief `running=0` between engine steps is normal; a wedge is `running=0`,
`waiting>0`, and no completions for `STALL_SECONDS`.

### 7. Stopping a sweep cleanly

Kill by PID; do not `pkill -f` a pattern that also matches your own shell's command line.

```bash
ps -eo pid,args | grep -E 'ccweka-out1-think-lowconc-sweep|run-gqa-iobound|stop-rule-live|wedge-watch|run-aiperf|aiperf (profile|system)' | grep -v grep
kill -INT <aiperf system_controller pid>            # lets aiperf export results
kill -TERM <driver, matrix, stream-sweep, watchdog, stop-rule pids>
podman ps -q | xargs -r podman stop -t 30
podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run ps -q | \
  xargs -r podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run stop -t 30
pkill -TERM -x certus-server-yaml                    # releases the NVMe devices
```

Then confirm the GPU is at ~0 MiB, port 8000 is free and no `certus-server-yaml` is left.

### 8. Results

```
$TOP/
  driver.log  summary.{md,csv}          # summary merges every point's rows
  <n>streams-stop-rule.log
  <n>streams/summary.{md,csv}
  <n>streams/<n>streams/{orchestrator.log, watchdog.log, certus-server.log, report.html}
  <n>streams/<n>streams/<arm>/{client.log, server.log, kvio.csv, aiperf/…, WEDGE-*/, WEDGED}
```

`summary.md` has, per arm: run time, tokens/s, req/s, TTFT p50/p99, errored requests,
vLLM queue time, external-tier hit %, recomputed %, and KV loaded. "Errored" requests
are almost always client-side connection-reuse failures (`ServerDisconnectedError` /
`ClientOSError` within a few ms of sending) that never reach vLLM; check
`aiperf/profile_export.jsonl` for the type. `plot-lowconc-patched.py` draws a PNG
from a combined summary table.

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
