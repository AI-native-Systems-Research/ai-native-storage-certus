#!/bin/bash
# Low-concurrency think-time CC-weka out1 sweep — the counterpart to
# ccweka-out1-think-sweep-20261117_164441 (600/1000 in-flight, where turn-0 prefill
# saturation made think time invisible). GOAL: at low in-flight caps conversations run
# deep into their turn counts under recorded think_time pacing, so the active working
# set (many conversations' accumulated prefix KV, well over the 64G tier) churns the
# CPU cache via session switching — driving eviction and DRAM->SSD spilling pressure.
#
# STOP RULE (per point, both arms): end the arm when fewer than the point's stream
# count conversations remain actively cycling — operationalized as vLLM running+waiting
# (= client in-flight = conversations actively processing; a thinking conversation
# holds nothing, but its KV stays in the tier as eviction fodder) staying below the
# cap for 10 consecutive minutes after a 15-min startup grace. Below the cap the
# session-switch churn (the thing being measured) is over. Stop = SIGINT to aiperf
# (graceful export), SIGKILL after 90s. Backstop: --benchmark-duration 14400 (4h).
# Implemented by stop-rule-live-watch.sh (standalone: it waits for the matrix to
# appear during preflight, matches aiperf's retitled process, and parses vLLM's
# labeled metric lines — the in-driver version that ran 2026-11-18 had none of that).
#
# Pool: the 391 ORIGINAL conversations (trunc262k-out1, no sim copies). Config
# otherwise identical to the 600/1000 sweep: 64G tier, blk64, Qwen2.5-7B-1M dense
# @ 262144, both backends, cputier wedge watchdog per arm.
#
# Self-contained bundle: all scripts this driver needs live in this directory.
STREAMS="${STREAMS:-32}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WATCHDOG="$SCRIPT_DIR/wedge-watch-live.sh"   # attach-mode watchdog (matches retitled aiperf)
STOP_RULE="$SCRIPT_DIR/stop-rule-live-watch.sh"
TOP="${TOP:-/mnt/certus1/vllm-serve-matrix/ccweka-out1-think-lowconc-sweep-$(date +%Y%m%d_%H%M%S)}"
TIMING="${TIMING:-think}"                    # think | delaycap | intervals | asap (run-aiperf-weka.sh)
DELAY_CAP="${DELAY_CAP:-30}"                 # seconds; TIMING=delaycap clamps each inter-turn gap to this
MAX_SECONDS="${MAX_SECONDS:-14400}"          # 4h per-arm safety cap (stop rule can end earlier)
TRACE="${TRACE:-/mnt/certus1/cc-traces-weka-062126.trunc262k-out1}"
mkdir -p "$TOP"
log(){ echo "[$(date '+%F %T')] $*" | tee -a "$TOP/driver.log"; }
# Match this bundle's matrix, not the original benchmarks/vllm-serve copy.
MATRIX_RE="^/bin/bash ${SCRIPT_DIR}/run-gqa-iobound-matrix\\.sh"

log "low-concurrency CC-weka out1 sweep (timing=${TIMING}$([[ $TIMING == delaycap ]] && echo " cap=${DELAY_CAP}s")): ${STREAMS} -> ${TOP} (branch $(git -C "$SCRIPT_DIR" branch --show-current) @ $(git -C "$SCRIPT_DIR" rev-parse --short HEAD))"
for n in $STREAMS; do
  log "===== ${n} streams (in-flight cap) ====="
  ( cd "$SCRIPT_DIR" && SWEEP_DIR="$TOP/${n}streams" STREAMS="$n" CPU_TIER_GB="${CPU_TIER_GB:-64}" \
      OFFLOAD_BLOCK_SIZE="${OFFLOAD_BLOCK_SIZE:-64}" \
      ARMS="${ARMS:-certus-nointervals cputier-nointervals}" MAX_SECONDS="$MAX_SECONDS" \
      MODEL="${MODEL:-/root/.cache/huggingface/qwen7b1m-dense}" SERVED_MODEL_NAME="${SERVED_MODEL_NAME:-qwen2.5-7b-1m}" \
      MAX_MODEL_LEN="${MAX_MODEL_LEN:-262144}" ROPE_YARN="${ROPE_YARN:-0}" CLIENT_SCRIPT=run-aiperf-weka-iobound.sh \
      TRACE="$TRACE" TIMING="$TIMING" DELAY_CAP="$DELAY_CAP" \
      SERVER_BIN="${SERVER_BIN:-/mnt/certus1/dwaddington-spdk-tier128/certus/target/release/certus-server-yaml}" \
      ./run-gqa-iobound-stream-sweep.sh > "$TOP/sweep-${n}.out" 2>&1 ) &
  sp=$!
  "$STOP_RULE" "$n" "$TOP/${n}streams-stop-rule.log" "$MATRIX_RE" &
  sr=$!
  # cputier wedge watchdog: attach once the matrix for this point is up. The
  # matrix results nest one deeper than SWEEP_DIR ($TOP/<n>streams/<n>streams),
  # so discover the matrix dir from its orchestrator.log rather than guessing —
  # v1 hardcoded $TOP/<n>streams/orchestrator.log, never found it, and passed a
  # RESULTS_DIR one level too shallow, so no watchdog ever attached.
  mdir=""
  until [[ -n "$mdir" ]]; do
    found=$(ls "$TOP/${n}streams"/*/orchestrator.log 2>/dev/null | head -1)
    [[ -n "$found" ]] && mdir="$(dirname "$found")"
    [[ -n "$mdir" ]] || { kill -0 "$sp" 2>/dev/null || break; sleep 10; }
  done
  mp=""; until [[ -n "$mp" ]]; do mp=$(pgrep -f "$MATRIX_RE" | head -1); [[ -n "$mp" ]] || { kill -0 "$sp" 2>/dev/null || break; sleep 2; }; done
  if [[ -n "$mp" && -n "$mdir" ]]; then
    log "  ${n} streams: wedge watchdog on matrix pid ${mp} (dir ${mdir})"
    ATTACH_PID="$mp" RESULTS_DIR="$mdir" ARM=cputier-nointervals CONCURRENT_STREAMS="$n" \
      STALL_SECONDS="${STALL_SECONDS:-300}" "$WATCHDOG" >> "$TOP/watchdog.log" 2>&1
  fi
  wait "$sp"; rc=$?
  kill "$sr" 2>/dev/null
  log "  ${n} streams done rc=${rc} (stop-rule log: ${TOP}/${n}streams-stop-rule.log)"
  if [[ "$n" != "${STREAMS##* }" && "${COOLDOWN_SECONDS:-600}" -gt 0 ]]; then
    log "  cooldown ${COOLDOWN_SECONDS:-600}s before next point"; sleep "${COOLDOWN_SECONDS:-600}"
  fi
done

# Merge the per-point summaries into one cross-sweep summary (rows sorted by streams).
python3 -I - "$TOP" $STREAMS <<'PY'
import csv, os, sys
top, streams = sys.argv[1], sys.argv[2:]
rows = []
for n in streams:
    p = os.path.join(top, f"{n}streams", "summary.csv")
    if not os.path.exists(p):
        print(f"warning: {p} missing"); continue
    with open(p) as f:
        r = list(csv.reader(f))
        rows += r[1:]
rows.sort(key=lambda r: int(r[0]) if r and r[0].isdigit() else 10**9)
if rows:
    head = ["streams", "arm", "offload block (tok)", "run (s)", "tokens/s", "req/s", "TTFT p50 (s)", "TTFT p99 (s)", "errored", "vLLM queue mean (s)", "ext hit %", "computed %", "KV load (TiB)", "load GB/s, wall clock (busiest 5 min)", "SSD read into host memory (TiB)", "SSD read not delivered to GPU, at least (TiB)", "summed load job time, SSD→GPU (s)"]
    with open(os.path.join(top, "summary.csv"), "w", newline="") as f:
        w = csv.writer(f, quoting=csv.QUOTE_ALL); w.writerow(head); w.writerows(rows)
    md = ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
    md += ["| " + " | ".join(r) + " |" for r in rows]
    open(os.path.join(top, "summary.md"), "w").write("\n".join(md) + "\n")
    print(f"merged {len(rows)} rows -> {top}/summary.md")
PY
log "sweep finished; wedged arms: $(ls "$TOP"/*/*/*/WEDGED 2>/dev/null | wc -l); summary: ${TOP}/summary.md"
