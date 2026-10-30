#!/bin/bash
# run_compare.sh — run certus-connector-bench, cputier-connector-bench and
# lmcache-connector-bench with one shared parameter set, then render a
# side-by-side HTML report.
#
#   run_compare.sh --out bench-results/connector-compare.html [options]
#
# Every workload flag is given ONCE here and forwarded identically to every
# bench. The DRAM tier size (--dram) becomes certus-server's
# --memory-tier-size, cputier's --cpu-bytes (and the container's CPU_BYTES)
# AND the LMCache server's L1 (--l1-size-gb). The arms run one after the
# other, since they share the GPU.
#
# The lmcache arm has no contention mode and is single-rank: it is skipped
# for --mode contention and for --tp > 1 (with a note), and runs its own
# --mode all (default, pipelined, prefix-miss, scheduler-step) for --mode all.
#
# Artifacts go to <out-without-.html>.d/: params.json, certus.csv, cputier.csv,
# lmcache.csv, certus-bench.log, cputier-bench.log, lmcache-bench.log, server.log.
#
# Options (defaults in brackets):
#   --out PATH               HTML report path (required)
#   --mode M                 default|pipelined|prefix-miss|contention|scheduler-step|all [all]
#   --bs N [16]  --num-blocks N [128]  --block-bytes N [2097152]
#   --min-duration S [5]  --warmup N [4]  --working-set N [512]
#   --pipeline-depth N [4]  --direction D [both]  --hit-ratio F [0.75]
#   --requests-per-step N [8]  --num-sessions N [32]  --lookup-order O [lookup-first]
#   --gpu N [0]  --tp N [1]
#   --dram SIZE [4G]         certus --memory-tier-size == cputier --cpu-bytes == lmcache L1
#   --device-pci ADDR        certus NVMe device (repeatable) [the 4 vfio NVMe drives
#                            0000:61:00.0..0000:64:00.0, same as benchmarks/vllm-serve], or
#   --drive-count N          first N discovered NVMe drives (replaces the default list)
#   --eviction-threshold F   certus-server --memory-tier-eviction-threshold [0.7]
#   --workers N              certus connector ThreadPoolExecutor workers [4]
#   --channels N             certus-server mailbox channels [auto: max(16, tp*(1+2*workers))]
#   --fs-tier DIR            cputier fs tier host dir [/raid0/cputier-bench-fs-tier]
#   --server-bin PATH        [target/release/certus-server]
#   --image IMAGE            cputier vLLM image [docker.io/vllm/vllm-openai:v0.30.0]
#   --l2-dir DIR             lmcache L2 (fs_native) host dir [/raid0/lmcache-bench-l2]
#   --lmcache-image IMAGE    lmcache image [localhost/vllm-lmcache-bench:vllm0.30.0]
#   --chunk-size N           lmcache server --chunk-size in tokens [256]
#   --skip-certus | --skip-cputier | --skip-lmcache | --report-only
#
# Disk tiers default to equal hardware: certus gets 4 SPDK NVMe drives, and the
# file-based tiers (cputier fs, lmcache L2) share /raid0, a 4-drive XFS RAID0 on
# separate NVMe drives (so the two do not contend with certus for SSD bandwidth).
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
cd "$REPO_ROOT"

OUT="" MODE=all BS=16 NUM_BLOCKS=128 BLOCK_BYTES=2097152 MIN_DURATION=5 WARMUP=4
WORKING_SET=512 PIPELINE_DEPTH=4 DIRECTION=both HIT_RATIO=0.75 REQS_PER_STEP=8
NUM_SESSIONS=32 LOOKUP_ORDER=lookup-first GPU=0 TP=1 DRAM=4G DEVICE_PCI=()
DRIVE_COUNT="" EVICT_THRESHOLD=0.7 WORKERS=4 CHANNELS="" FS_TIER=/raid0/cputier-bench-fs-tier
SERVER_BIN=target/release/certus-server IMAGE=docker.io/vllm/vllm-openai:v0.30.0
L2_DIR=/raid0/lmcache-bench-l2 LMCACHE_IMAGE=localhost/vllm-lmcache-bench:vllm0.30.0 CHUNK_SIZE=256
RUN_CERTUS=1 RUN_CPUTIER=1 RUN_LMCACHE=1
SHM_PATH=/dev/shm/certus-shmq
DEFAULT_DEVICE_PCI=(0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0)

while (($#)); do
  case "$1" in
    --out) OUT="$2"; shift ;;
    --mode) MODE="$2"; shift ;;
    --bs) BS="$2"; shift ;;
    --num-blocks) NUM_BLOCKS="$2"; shift ;;
    --block-bytes) BLOCK_BYTES="$2"; shift ;;
    --min-duration) MIN_DURATION="$2"; shift ;;
    --warmup) WARMUP="$2"; shift ;;
    --working-set) WORKING_SET="$2"; shift ;;
    --pipeline-depth) PIPELINE_DEPTH="$2"; shift ;;
    --direction) DIRECTION="$2"; shift ;;
    --hit-ratio) HIT_RATIO="$2"; shift ;;
    --requests-per-step) REQS_PER_STEP="$2"; shift ;;
    --num-sessions) NUM_SESSIONS="$2"; shift ;;
    --lookup-order) LOOKUP_ORDER="$2"; shift ;;
    --gpu) GPU="$2"; shift ;;
    --tp) TP="$2"; shift ;;
    --dram) DRAM="$2"; shift ;;
    --device-pci) DEVICE_PCI+=("$2"); shift ;;
    --drive-count) DRIVE_COUNT="$2"; shift ;;
    --eviction-threshold) EVICT_THRESHOLD="$2"; shift ;;
    --workers) WORKERS="$2"; shift ;;
    --channels) CHANNELS="$2"; shift ;;
    --fs-tier) FS_TIER="$2"; shift ;;
    --server-bin) SERVER_BIN="$2"; shift ;;
    --image) IMAGE="$2"; shift ;;
    --l2-dir) L2_DIR="$2"; shift ;;
    --lmcache-image) LMCACHE_IMAGE="$2"; shift ;;
    --chunk-size) CHUNK_SIZE="$2"; shift ;;
    --skip-certus) RUN_CERTUS=0 ;;
    --skip-cputier) RUN_CPUTIER=0 ;;
    --skip-lmcache) RUN_LMCACHE=0 ;;
    --report-only) RUN_CERTUS=0; RUN_CPUTIER=0; RUN_LMCACHE=0 ;;
    -h|--help) sed -n '2,46p' "$0"; exit 0 ;;
    *) echo "error: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

[[ -n "$OUT" ]] || { echo "error: --out PATH is required" >&2; exit 2; }
[[ "$OUT" == *.html ]] || OUT="$OUT.html"
mkdir -p "$(dirname "$OUT")"
OUT="$(cd "$(dirname "$OUT")" && pwd -P)/$(basename "$OUT")"
ART="${OUT%.html}.d"
mkdir -p "$ART"

if ((${#DEVICE_PCI[@]} == 0)) && [[ -z "$DRIVE_COUNT" ]]; then DEVICE_PCI=("${DEFAULT_DEVICE_PCI[@]}"); fi
if [[ -z "$CHANNELS" ]]; then
  CHANNELS=$(( TP * (1 + 2 * WORKERS) )); (( CHANNELS < 16 )) && CHANNELS=16
fi

# GPU list for the TP ranks: certus uses physical indices; the container
# renumbers the exposed GPUs 0..tp-1, so cputier always gets --gpu 0.
GPU_LIST=$(seq -s, "$GPU" $(( GPU + TP - 1 )))

# The one shared workload argument list, passed verbatim to every bench.
SHARED=(--mode "$MODE" --bs "$BS" --num-blocks "$NUM_BLOCKS" --block-bytes "$BLOCK_BYTES"
        --min-duration "$MIN_DURATION" --warmup "$WARMUP" --working-set "$WORKING_SET"
        --pipeline-depth "$PIPELINE_DEPTH" --direction "$DIRECTION" --hit-ratio "$HIT_RATIO"
        --requests-per-step "$REQS_PER_STEP" --num-sessions "$NUM_SESSIONS"
        --lookup-order "$LOOKUP_ORDER" --tp "$TP")

SERVER_ARGS=(--channels "$CHANNELS" --memory-tier-size "$DRAM"
             --memory-tier-eviction-threshold "$EVICT_THRESHOLD" --shm-path "$SHM_PATH" --format)
if ((${#DEVICE_PCI[@]})); then
  for d in "${DEVICE_PCI[@]}"; do SERVER_ARGS+=(--device-pci "$d"); done
else
  SERVER_ARGS+=(--drive-count "$DRIVE_COUNT")
fi

CERTUS_CMD=(python3 benchmarks/certus-connector-bench/bench_connector_lifecycle.py "${SHARED[@]}"
            --gpu "$GPU" --workers "$WORKERS" --shm-path "$SHM_PATH"
            --csv "$ART/certus.csv" --tag certus)
# --csv is resolved inside the container, where the repo is /certus.
REL_ART="${ART#"$REPO_ROOT"/}"
if [[ "$REL_ART" == /* ]]; then CPUTIER_CSV_IN=/fs-tier/cputier.csv; else CPUTIER_CSV_IN="/certus/$REL_ART/cputier.csv"; fi
CPUTIER_CMD=(benchmarks/cputier-connector-bench/run-in-container.sh "${SHARED[@]}"
             --gpu 0 --cpu-bytes "$DRAM" --csv "$CPUTIER_CSV_IN" --tag cputier)

# lmcache: one rank, no contention mode; L1 is --dram in GiB.
LMCACHE_NOTE=""
if [[ "$MODE" == contention ]]; then
  LMCACHE_NOTE="skipped: lmcache-connector-bench has no contention mode"
elif (( TP > 1 )); then
  LMCACHE_NOTE="skipped: lmcache-connector-bench is single-rank (--tp 1)"
fi
[[ -n "$LMCACHE_NOTE" ]] && RUN_LMCACHE=0
L1_GB=$(python3 -c "
import re,sys; m=re.fullmatch(r'(\d+(?:\.\d+)?)([KMGT]?)', '$DRAM'.strip().upper())
print(f'{float(m.group(1)) * (1 << {\"\":0,\"K\":10,\"M\":20,\"G\":30,\"T\":40}[m.group(2)]) / 2**30:g}')")
if [[ "$REL_ART" == /* ]]; then LMCACHE_CSV_IN=/certus/.lmcache-compare-$$.csv; else LMCACHE_CSV_IN="/certus/$REL_ART/lmcache.csv"; fi
LMCACHE_CMD=(benchmarks/lmcache-connector-bench/run-in-container.sh "${SHARED[@]}"
             --gpu 0 --csv "$LMCACHE_CSV_IN" --tag lmcache)

python3 - "$ART/params.json" <<EOF
import json, sys
json.dump({
  "shared": {"mode": "$MODE", "bs": $BS, "num_blocks": $NUM_BLOCKS, "block_bytes": $BLOCK_BYTES,
             "min_duration": $MIN_DURATION, "warmup": $WARMUP, "working_set": $WORKING_SET,
             "pipeline_depth": $PIPELINE_DEPTH, "direction": "$DIRECTION", "hit_ratio": $HIT_RATIO,
             "requests_per_step": $REQS_PER_STEP, "num_sessions": $NUM_SESSIONS,
             "lookup_order": "$LOOKUP_ORDER", "tp": $TP, "gpus": "$GPU_LIST", "dram": "$DRAM"},
  "certus": {"channels": $CHANNELS, "eviction_threshold": $EVICT_THRESHOLD, "workers": $WORKERS,
             "devices": "${DEVICE_PCI[*]:-drive-count=$DRIVE_COUNT}", "server_bin": "$SERVER_BIN",
             "server_args": """${SERVER_ARGS[*]}""", "bench_cmd": """${CERTUS_CMD[*]}"""},
  "cputier": {"image": "$IMAGE", "fs_tier": "$FS_TIER", "spec": "tiering",
              "bench_cmd": """GPU=$GPU_LIST CPU_BYTES=$DRAM FS_TIER_HOST=$FS_TIER IMAGE=$IMAGE ${CPUTIER_CMD[*]}"""},
  "lmcache": {"image": "$LMCACHE_IMAGE", "l2_dir": "$L2_DIR", "l1_gb": $L1_GB, "chunk_size": $CHUNK_SIZE,
              "note": "$LMCACHE_NOTE",
              "bench_cmd": """GPU=$GPU L1_GB=$L1_GB L2_DIR=$L2_DIR CHUNK_SIZE=$CHUNK_SIZE IMAGE=$LMCACHE_IMAGE ${LMCACHE_CMD[*]}"""},
  "git": {"branch": "$(git branch --show-current)", "commit": "$(git rev-parse --short HEAD)",
          "dirty": $( [[ -n "$(git status --porcelain --untracked-files=no)" ]] && echo True || echo False )},
  "host": "$(hostname)", "gpu_name": "$(nvidia-smi -i "$GPU" --query-gpu=name --format=csv,noheader 2>/dev/null | head -1)",
}, open(sys.argv[1], "w"), indent=2)
EOF

STATUS_CERTUS=skipped STATUS_CPUTIER=skipped STATUS_LMCACHE=skipped
SERVER_PID=""

stop_server() {
  [[ -n "$SERVER_PID" ]] || return 0
  kill -TERM "$SERVER_PID" 2>/dev/null
  for _ in $(seq 16); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
  # certus-server can ignore SIGTERM during SPDK teardown.
  kill -KILL "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}
trap stop_server EXIT INT TERM

if ((RUN_CERTUS)); then
  STATUS_CERTUS=failed
  if [[ ! -x "$SERVER_BIN" ]]; then
    echo "[compare] building certus-server (release)"
    cargo build -p certus-server --release || { echo "[compare] build failed" >&2; SERVER_BIN=""; }
  fi
  if [[ -n "$SERVER_BIN" ]]; then
    rm -f "$ART/certus.csv"
    echo "[compare] starting: $SERVER_BIN ${SERVER_ARGS[*]}"
    "$SERVER_BIN" "${SERVER_ARGS[@]}" >"$ART/server.log" 2>&1 &
    SERVER_PID=$!
    ready=0
    for _ in $(seq 240); do
      grep -q "shared-memory IPC path" "$ART/server.log" && { ready=1; break; }
      kill -0 "$SERVER_PID" 2>/dev/null || break
      sleep 0.5
    done
    if ((ready)); then
      sleep 1
      echo "[compare] certus arm: ${CERTUS_CMD[*]}"
      "${CERTUS_CMD[@]}" 2>&1 | tee "$ART/certus-bench.log"
      [[ ${PIPESTATUS[0]} -eq 0 && -s "$ART/certus.csv" ]] && STATUS_CERTUS=ok
    else
      echo "[compare] certus-server did not become ready; see $ART/server.log" >&2
    fi
    stop_server
  fi
fi

if ((RUN_CPUTIER)); then
  STATUS_CPUTIER=failed
  rm -f "$ART/cputier.csv" "$FS_TIER/cputier.csv"
  echo "[compare] cputier arm: GPU=$GPU_LIST CPU_BYTES=$DRAM ${CPUTIER_CMD[*]}"
  GPU="$GPU_LIST" CPU_BYTES="$DRAM" FS_TIER_HOST="$FS_TIER" IMAGE="$IMAGE" \
    "${CPUTIER_CMD[@]}" 2>&1 | tee "$ART/cputier-bench.log"
  rc=${PIPESTATUS[0]}
  [[ "$CPUTIER_CSV_IN" == /fs-tier/* && -f "$FS_TIER/cputier.csv" ]] && mv "$FS_TIER/cputier.csv" "$ART/cputier.csv"
  [[ $rc -eq 0 && -s "$ART/cputier.csv" ]] && STATUS_CPUTIER=ok
fi

if ((RUN_LMCACHE)); then
  STATUS_LMCACHE=failed
  rm -f "$ART/lmcache.csv"
  mkdir -p "$L2_DIR"
  echo "[compare] lmcache arm: GPU=$GPU L1_GB=$L1_GB ${LMCACHE_CMD[*]}"
  GPU="$GPU" L1_GB="$L1_GB" L2_DIR="$L2_DIR" CHUNK_SIZE="$CHUNK_SIZE" IMAGE="$LMCACHE_IMAGE" \
    "${LMCACHE_CMD[@]}" 2>&1 | grep --line-buffered -vE "LMCache (INFO|WARNING)|slapd|Discovered API|CudaIPCTypes" \
    | tee "$ART/lmcache-bench.log"
  rc=${PIPESTATUS[0]}
  tmp_csv="$REPO_ROOT/${LMCACHE_CSV_IN#/certus/}"
  [[ "$LMCACHE_CSV_IN" == /certus/.lmcache-compare-* && -f "$tmp_csv" ]] && mv "$tmp_csv" "$ART/lmcache.csv"
  [[ $rc -eq 0 && -s "$ART/lmcache.csv" ]] && STATUS_LMCACHE=ok
elif [[ -n "$LMCACHE_NOTE" ]]; then
  echo "[compare] lmcache arm $LMCACHE_NOTE"
fi

echo "[compare] certus=$STATUS_CERTUS cputier=$STATUS_CPUTIER lmcache=$STATUS_LMCACHE"
python3 "$SCRIPT_DIR/make_report.py" --artifacts "$ART" --out "$OUT"
echo "[compare] report: $OUT"
[[ "$STATUS_CERTUS" != failed && "$STATUS_CPUTIER" != failed && "$STATUS_LMCACHE" != failed ]]
