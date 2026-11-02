#!/bin/bash
# run_compare.sh — run certus-connector-bench, cputier-connector-bench,
# lmcache-connector-bench and 3fs-connector-bench with one shared parameter set,
# then render a side-by-side HTML report.
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
# The 3fs arm (vLLM's HF3FSKVConnector) has no memory tier: --dram does not apply,
# every store and load goes to 3FS, and it has no mixed-eviction or contention
# phase. It needs a live 3FS FUSE mount (deploy/3fs/run.sh, as root) and the
# vLLM + 3FS image; without the mount the arm is skipped with a note.
# --h3fs-mock runs it instead on vLLM's file-backed mock client on /raid0, which
# tests the code path but is NOT a 3FS measurement (the report says so).
#
# The cputier-3fs arm is cputier-connector-bench unchanged — vLLM's
# TieringOffloadingSpec with the same --dram CPU tier — but with its fs secondary
# tier on the 3FS FUSE mount (O_DIRECT) instead of /raid0: 3FS WITH a memory tier,
# to set against the memory-tier-less 3fs arm. It reaches 3FS through FUSE (POSIX
# I/O), not the USRBIO path the 3fs arm uses. It runs when the 3FS mount is live.
#
# Artifacts go to <out-without-.html>.d/: params.json, certus.csv, cputier.csv,
# lmcache.csv, 3fs.csv, one <arm>-bench.log per arm, server.log.
#
# Options (defaults in brackets):
#   --out PATH               HTML report path (required; relative to the current dir)
#   --latex                  also write the throughput table as LaTeX (booktabs) to
#                            <out-without-.html>.tex
#   --mode M                 default|pipelined|prefix-miss|contention|scheduler-step|all [all]
#   --bs N [16]  --num-blocks N [128]  --block-bytes N [2097152]
#   --min-duration S [5]  --warmup N [4]  --working-set N [512]
#   --pipeline-depth N [4]  --direction D [both]  --hit-ratio F [0.75]
#   --requests-per-step N [8]  --num-sessions N [32]  --lookup-order O [lookup-first]
#   --gpu N [0]  --tp N [1]
#   --layers N [1]           GPU tensors each block is split into, on EVERY arm (certus
#                            --pieces-per-block, cputier/lmcache --layers). 1 = one
#                            contiguous region per block, the BLHNC layout CertusConnector
#                            requests; 32 = vLLM's default layer-outermost LBHNC
#   --io-threads N [16]      disk-tier I/O threads on the file-based arms (cputier fs
#                            read AND write threads; lmcache fs_native workers)
#   --numa N [GPU's node]    NUMA node every arm's CPUs and host memory are bound to;
#                            -1 disables binding
#   --dram SIZE [4G]         certus --memory-tier-size == cputier --cpu-bytes == lmcache L1
#   --device-pci ADDR        certus NVMe device (repeatable) [the 4 vfio NVMe drives
#                            0000:61:00.0..0000:64:00.0, same as benchmarks/vllm-serve], or
#   --drive-count N          first N discovered NVMe drives (replaces the default list)
#   --eviction-threshold F   certus-server --memory-tier-eviction-threshold [0.9]
#   --workers N              certus connector ThreadPoolExecutor workers [4]
#   --channels N             certus-server mailbox channels [auto: max(16, tp*(1+2*workers))]
#   --fs-tier DIR            cputier fs tier host dir [/raid0/cputier-bench-fs-tier]
#   --server-bin PATH        [target/release/certus-server, rebuilt (cargo, release) when
#                            missing or older than the tree; an explicit PATH is used as-is]
#   --image IMAGE            cputier vLLM image [docker.io/vllm/vllm-openai:v0.30.0]
#   --l2-dir DIR             lmcache L2 (fs_native) host dir [/raid0/lmcache-bench-l2]
#   --lmcache-image IMAGE    lmcache image [localhost/vllm-lmcache-bench:vllm0.30.0]
#   --chunk-size N           lmcache server --chunk-size in tokens [256]
#   --h3fs-mount DIR         3FS FUSE mountpoint for the 3fs arm [/mnt/3fs]
#   --h3fs-image IMAGE       vLLM + 3FS USRBIO image [localhost/vllm-hf3fs-bench:vllm0.30.0]
#   --h3fs-file-size SIZE    3fs connector data file (hf3fs_file_size) [64G]
#   --h3fs-mock              run the 3fs arm on vLLM's mock client (code path only)
#   --h3fs-mock-dir DIR      mock data dir [/raid0/h3fs-mock]
#   --skip-certus | --skip-cputier | --skip-lmcache | --skip-3fs | --skip-cputier-3fs
#   --report-only
#
# Fairness: every arm gets the same workload flags, DRAM tier, KV layout (--layers),
# NUMA node (CPU + memory bound to the GPU's node) and disk I/O parallelism; each
# bench excludes its per-pass disk drain / tier reset from the timed window and
# records it separately as sustained_gbs; and the report flags any row where an arm
# dropped stores, missed loads, or (cold rows) read nothing back from disk.
#
# Disk tiers default to equal hardware: certus gets 4 SPDK NVMe drives, and the
# file-based tiers (cputier fs, lmcache L2) share /raid0, a 4-drive XFS RAID0 on
# separate NVMe drives (so the two do not contend with certus for SSD bandwidth).
# 3FS keeps its storage targets wherever deploy/3fs put them (DATA_DIR, default
# /cache/3fs); for matched hardware start it with DATA_DIR on /raid0 too.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
CALLER_DIR="$PWD"   # relative --out (and --server-bin) resolve against the caller's dir
cd "$REPO_ROOT"

OUT="" MODE=all BS=16 NUM_BLOCKS=128 BLOCK_BYTES=2097152 MIN_DURATION=5 WARMUP=4
WORKING_SET=512 PIPELINE_DEPTH=4 DIRECTION=both HIT_RATIO=0.75 REQS_PER_STEP=8
NUM_SESSIONS=32 LOOKUP_ORDER=lookup-first GPU=0 TP=1 DRAM=4G DEVICE_PCI=()
LAYERS=1 IO_THREADS=16 NUMA=""
DRIVE_COUNT="" EVICT_THRESHOLD=0.9 WORKERS=4 CHANNELS="" FS_TIER=/raid0/cputier-bench-fs-tier
SERVER_BIN="" IMAGE=docker.io/vllm/vllm-openai:v0.30.0
L2_DIR=/raid0/lmcache-bench-l2 LMCACHE_IMAGE=localhost/vllm-lmcache-bench:vllm0.30.0 CHUNK_SIZE=256
RUN_CERTUS=1 RUN_CPUTIER=1 RUN_LMCACHE=1 RUN_H3FS=1 RUN_CPUTIER_3FS=1 REPORT_ONLY=0 LATEX=0
H3FS_MOUNT=/mnt/3fs H3FS_IMAGE=localhost/vllm-hf3fs-bench:vllm0.30.0 H3FS_FILE_SIZE=64G
H3FS_MOCK=0 H3FS_MOCK_DIR=/raid0/h3fs-mock
SHM_PATH=/dev/shm/certus-shmq
DEFAULT_DEVICE_PCI=(0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0)

while (($#)); do
  case "$1" in
    --out) OUT="$2"; shift ;;
    --latex) LATEX=1 ;;
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
    --layers) LAYERS="$2"; shift ;;
    --io-threads) IO_THREADS="$2"; shift ;;
    --numa) NUMA="$2"; shift ;;
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
    --skip-3fs) RUN_H3FS=0 ;;
    --skip-cputier-3fs) RUN_CPUTIER_3FS=0 ;;
    --h3fs-mount) H3FS_MOUNT="$2"; shift ;;
    --h3fs-image) H3FS_IMAGE="$2"; shift ;;
    --h3fs-file-size) H3FS_FILE_SIZE="$2"; shift ;;
    --h3fs-mock) H3FS_MOCK=1 ;;
    --h3fs-mock-dir) H3FS_MOCK_DIR="$2"; shift ;;
    --report-only) RUN_CERTUS=0; RUN_CPUTIER=0; RUN_LMCACHE=0; RUN_H3FS=0; RUN_CPUTIER_3FS=0; REPORT_ONLY=1 ;;
    -h|--help) sed -n '2,/^set -uo pipefail/{/^#/p}' "$0"; exit 0 ;;
    *) echo "error: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

[[ -n "$OUT" ]] || { echo "error: --out PATH is required" >&2; exit 2; }
[[ "$OUT" == *.html ]] || OUT="$OUT.html"
[[ "$OUT" == /* ]] || OUT="$CALLER_DIR/$OUT"
mkdir -p "$(dirname "$OUT")"
OUT="$(cd "$(dirname "$OUT")" && pwd -P)/$(basename "$OUT")"
ART="${OUT%.html}.d"
mkdir -p "$ART"

# Default server: the tree's own release build, rebuilt below when stale. An
# explicit --server-bin is taken as given (relative to the caller's dir).
SERVER_AUTO=0
if [[ -z "$SERVER_BIN" ]]; then
  SERVER_BIN="$REPO_ROOT/target/release/certus-server"; SERVER_AUTO=1
elif [[ "$SERVER_BIN" != /* ]]; then
  SERVER_BIN="$CALLER_DIR/$SERVER_BIN"
fi

if ((${#DEVICE_PCI[@]} == 0)) && [[ -z "$DRIVE_COUNT" ]]; then DEVICE_PCI=("${DEFAULT_DEVICE_PCI[@]}"); fi
if [[ -z "$CHANNELS" ]]; then
  CHANNELS=$(( TP * (1 + 2 * WORKERS) )); (( CHANNELS < 16 )) && CHANNELS=16
fi

# NUMA: bind every arm's CPUs and memory to the GPU's node (the certus drives,
# 61-64, are on node 0 with GPU 0). certus-server and the certus bench run under
# numactl. The podman arms inherit numactl's memory policy, but rootless podman has
# no cpuset controller and does not carry CPU affinity into the container, so they
# also get CPUS= and pin themselves with taskset inside the container.
if [[ -z "$NUMA" ]]; then
  gpu_bus=$(nvidia-smi -i "$GPU" --query-gpu=pci.bus_id --format=csv,noheader 2>/dev/null | head -1)
  gpu_bus=${gpu_bus,,}; gpu_bus=${gpu_bus#0000}
  NUMA=$(cat "/sys/bus/pci/devices/${gpu_bus}/numa_node" 2>/dev/null || echo -1)
fi
NUMA_CMD=() NUMA_CPUS=""
if (( NUMA >= 0 )); then
  NUMA_CPUS=$(cat "/sys/devices/system/node/node${NUMA}/cpulist")
  NUMA_CMD=(numactl --cpunodebind="$NUMA" --membind="$NUMA")
  command -v numactl >/dev/null || { echo "error: numactl not found (--numa -1 to skip binding)" >&2; exit 2; }
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
            --pieces-per-block "$LAYERS"
            --gpu "$GPU" --workers "$WORKERS" --shm-path "$SHM_PATH"
            --csv "$ART/certus.csv" --tag certus)
# --csv is resolved inside the container, where the repo is /certus.
REL_ART="${ART#"$REPO_ROOT"/}"
if [[ "$REL_ART" == /* ]]; then CPUTIER_CSV_IN=/fs-tier/cputier.csv; else CPUTIER_CSV_IN="/certus/$REL_ART/cputier.csv"; fi
CPUTIER_CMD=(benchmarks/cputier-connector-bench/run-in-container.sh "${SHARED[@]}"
             --layers "$LAYERS" --fs-read-threads "$IO_THREADS" --fs-write-threads "$IO_THREADS"
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
             --layers "$LAYERS"
             --gpu 0 --csv "$LMCACHE_CSV_IN" --tag lmcache)

# 3fs: one rank, no contention mode, no memory tier. Real 3FS needs the mount.
H3FS_NOTE=""
if [[ "$MODE" == contention ]]; then
  H3FS_NOTE="skipped: 3fs-connector-bench has no contention mode"
elif (( TP > 1 )); then
  H3FS_NOTE="skipped: 3fs-connector-bench is single-rank (--tp 1)"
elif [[ "$H3FS_MOCK" != 1 ]] && ! mountpoint -q "$H3FS_MOUNT" 2>/dev/null; then
  H3FS_NOTE="skipped: no 3FS mount at $H3FS_MOUNT (sudo deploy/3fs/run.sh; or --h3fs-mock to test the code path)"
fi
[[ -n "$H3FS_NOTE" ]] && ((RUN_H3FS)) && RUN_H3FS=0
H3FS_CLIENT=$([[ "$H3FS_MOCK" == 1 ]] && echo mock || echo usrbio)
[[ "$H3FS_MOCK" == 1 ]] && H3FS_RUN_IMAGE="${H3FS_MOCK_IMAGE:-docker.io/vllm/vllm-openai:v0.30.0}" || H3FS_RUN_IMAGE="$H3FS_IMAGE"
if [[ "$REL_ART" == /* ]]; then H3FS_CSV_IN=/certus/.3fs-compare-$$.csv; else H3FS_CSV_IN="/certus/$REL_ART/3fs.csv"; fi
# The 3FS connector's I/O jobs (hf3fs_client_numjobs) are its disk parallelism,
# set like the file-based arms' threads.
H3FS_CMD=(benchmarks/3fs-connector-bench/run-in-container.sh "${SHARED[@]}"
          --layers "$LAYERS" --numjobs "$IO_THREADS" --file-size "$H3FS_FILE_SIZE"
          --gpu 0 --csv "$H3FS_CSV_IN" --tag 3fs)

# cputier-3fs: the cputier bench with its fs tier on 3FS. Needs a live 3FS mount
# (the mock is a plain directory, so it would just be cputier again).
C3_FS_TIER="$H3FS_MOUNT/vllm/cputier-bench-fs-tier"
C3_NOTE=""
if ! mountpoint -q "$H3FS_MOUNT" 2>/dev/null; then
  C3_NOTE="skipped: no 3FS mount at $H3FS_MOUNT (sudo deploy/3fs/run.sh)"
elif [[ ! -w "$H3FS_MOUNT/vllm" ]]; then
  C3_NOTE="skipped: $H3FS_MOUNT/vllm is not writable"
fi
[[ -n "$C3_NOTE" ]] && ((RUN_CPUTIER_3FS)) && RUN_CPUTIER_3FS=0
if [[ "$REL_ART" == /* ]]; then C3_CSV_IN=/fs-tier/cputier-3fs.csv; else C3_CSV_IN="/certus/$REL_ART/cputier-3fs.csv"; fi
C3_CMD=(benchmarks/cputier-connector-bench/run-in-container.sh "${SHARED[@]}"
        --layers "$LAYERS" --fs-read-threads "$IO_THREADS" --fs-write-threads "$IO_THREADS"
        --gpu 0 --cpu-bytes "$DRAM" --csv "$C3_CSV_IN" --tag cputier-3fs)

# --report-only re-renders an existing run: keep the params.json that run wrote
# (it records the run's real flags), rather than this invocation's defaults.
if (( REPORT_ONLY )) && [[ -s "$ART/params.json" ]]; then
  echo "[compare] --report-only: keeping $ART/params.json from the original run"
else
python3 - "$ART/params.json" <<EOF
import json, sys
json.dump({
  "shared": {"mode": "$MODE", "bs": $BS, "num_blocks": $NUM_BLOCKS, "block_bytes": $BLOCK_BYTES,
             "min_duration": $MIN_DURATION, "warmup": $WARMUP, "working_set": $WORKING_SET,
             "pipeline_depth": $PIPELINE_DEPTH, "direction": "$DIRECTION", "hit_ratio": $HIT_RATIO,
             "requests_per_step": $REQS_PER_STEP, "num_sessions": $NUM_SESSIONS,
             "lookup_order": "$LOOKUP_ORDER", "tp": $TP, "gpus": "$GPU_LIST", "dram": "$DRAM",
             "layers": $LAYERS, "io_threads": $IO_THREADS, "numa_node": $NUMA, "numa_cpus": "$NUMA_CPUS"},
  "certus": {"channels": $CHANNELS, "eviction_threshold": $EVICT_THRESHOLD, "workers": $WORKERS,
             "devices": "${DEVICE_PCI[*]:-drive-count=$DRIVE_COUNT}", "server_bin": "$SERVER_BIN",
             "server_args": """${SERVER_ARGS[*]}""", "bench_cmd": """${NUMA_CMD[*]} ${CERTUS_CMD[*]}"""},
  "cputier": {"image": "$IMAGE", "fs_tier": "$FS_TIER", "spec": "tiering",
              "bench_cmd": """GPU=$GPU_LIST CPU_BYTES=$DRAM FS_TIER_HOST=$FS_TIER IMAGE=$IMAGE CPUS=$NUMA_CPUS ${NUMA_CMD[*]} ${CPUTIER_CMD[*]}"""},
  "lmcache": {"image": "$LMCACHE_IMAGE", "l2_dir": "$L2_DIR", "l1_gb": $L1_GB, "chunk_size": $CHUNK_SIZE,
              "note": "$LMCACHE_NOTE",
              "bench_cmd": """GPU=$GPU L1_GB=$L1_GB L2_DIR=$L2_DIR CHUNK_SIZE=$CHUNK_SIZE IMAGE=$LMCACHE_IMAGE CPUS=$NUMA_CPUS L2_THREADS=$IO_THREADS ${NUMA_CMD[*]} ${LMCACHE_CMD[*]}"""},
  "cputier-3fs": {"image": "$IMAGE", "fs_tier": "$C3_FS_TIER", "spec": "tiering", "note": "$C3_NOTE",
                  "bench_cmd": """GPU=$GPU_LIST CPU_BYTES=$DRAM FS_TIER_HOST=$C3_FS_TIER IMAGE=$IMAGE CPUS=$NUMA_CPUS ${NUMA_CMD[*]} ${C3_CMD[*]}"""},
  "3fs": {"image": "$H3FS_RUN_IMAGE", "client": "$H3FS_CLIENT",
          "storage": "$([[ "$H3FS_MOCK" == 1 ]] && echo "$H3FS_MOCK_DIR (mock file)" || echo "$H3FS_MOUNT")",
          "file_size": "$H3FS_FILE_SIZE", "numjobs": $IO_THREADS, "note": "$H3FS_NOTE",
          "bench_cmd": """GPU=$GPU MOCK=$H3FS_MOCK IMAGE=$H3FS_RUN_IMAGE CPUS=$NUMA_CPUS ${NUMA_CMD[*]} ${H3FS_CMD[*]}"""},
  "git": {"branch": "$(git branch --show-current)", "commit": "$(git rev-parse --short HEAD)",
          "dirty": $( [[ -n "$(git status --porcelain --untracked-files=no)" ]] && echo True || echo False )},
  "host": "$(hostname)", "gpu_name": "$(nvidia-smi -i "$GPU" --query-gpu=name --format=csv,noheader 2>/dev/null | head -1)",
}, open(sys.argv[1], "w"), indent=2)
EOF
fi

STATUS_CERTUS=skipped STATUS_CPUTIER=skipped STATUS_LMCACHE=skipped STATUS_H3FS=skipped STATUS_C3=skipped
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
  # cargo rebuilds only what changed, so this is cheap when the binary is current.
  # Without it a months-old binary from an earlier tree is used silently and can
  # reject flags this script passes (e.g. --channels).
  if ((SERVER_AUTO)) || [[ ! -x "$SERVER_BIN" ]]; then
    echo "[compare] building certus-server (release; no-op when up to date)"
    # rw-telemetry: per-drive SSD read/write counters, so the cold rows can show
    # that certus really read from SSD (the other arms report it from their own
    # counters).
    cargo build -p certus-server --release --features rw-telemetry \
      || { echo "[compare] build failed" >&2; SERVER_BIN=""; }
  fi
  if [[ -n "$SERVER_BIN" ]]; then
    rm -f "$ART/certus.csv"
    echo "[compare] starting: ${NUMA_CMD[*]} $SERVER_BIN ${SERVER_ARGS[*]}"
    "${NUMA_CMD[@]}" "$SERVER_BIN" "${SERVER_ARGS[@]}" >"$ART/server.log" 2>&1 &
    SERVER_PID=$!
    ready=0
    for _ in $(seq 240); do
      grep -q "shared-memory IPC path" "$ART/server.log" && { ready=1; break; }
      kill -0 "$SERVER_PID" 2>/dev/null || break
      sleep 0.5
    done
    if ((ready)); then
      sleep 1
      echo "[compare] certus arm: ${NUMA_CMD[*]} ${CERTUS_CMD[*]}"
      "${NUMA_CMD[@]}" "${CERTUS_CMD[@]}" 2>&1 | tee "$ART/certus-bench.log"
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
  GPU="$GPU_LIST" CPU_BYTES="$DRAM" FS_TIER_HOST="$FS_TIER" IMAGE="$IMAGE" CPUS="$NUMA_CPUS" \
    "${NUMA_CMD[@]}" "${CPUTIER_CMD[@]}" 2>&1 | tee "$ART/cputier-bench.log"
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
    CPUS="$NUMA_CPUS" L2_THREADS="$IO_THREADS" \
    "${NUMA_CMD[@]}" "${LMCACHE_CMD[@]}" 2>&1 | grep --line-buffered -vE "LMCache (INFO|WARNING)|slapd|Discovered API|CudaIPCTypes" \
    | tee "$ART/lmcache-bench.log"
  rc=${PIPESTATUS[0]}
  tmp_csv="$REPO_ROOT/${LMCACHE_CSV_IN#/certus/}"
  [[ "$LMCACHE_CSV_IN" == /certus/.lmcache-compare-* && -f "$tmp_csv" ]] && mv "$tmp_csv" "$ART/lmcache.csv"
  [[ $rc -eq 0 && -s "$ART/lmcache.csv" ]] && STATUS_LMCACHE=ok
elif [[ -n "$LMCACHE_NOTE" ]]; then
  echo "[compare] lmcache arm $LMCACHE_NOTE"
fi

if ((RUN_H3FS)); then
  STATUS_H3FS=failed
  rm -f "$ART/3fs.csv"
  echo "[compare] 3fs arm (${H3FS_CLIENT} client): GPU=$GPU ${H3FS_CMD[*]}"
  GPU="$GPU" MOCK="$H3FS_MOCK" IMAGE="$H3FS_RUN_IMAGE" H3FS_MOUNT="$H3FS_MOUNT" H3FS_DIR="$H3FS_MOCK_DIR" \
    CPUS="$NUMA_CPUS" \
    "${NUMA_CMD[@]}" "${H3FS_CMD[@]}" 2>&1 \
    | grep --line-buffered -vE "CUDA_HOME|Triton|cpp_extension|frombuffer|UserWarning|HF3FS Connector Completed" \
    | tee "$ART/3fs-bench.log"
  rc=${PIPESTATUS[0]}
  tmp_csv="$REPO_ROOT/${H3FS_CSV_IN#/certus/}"
  [[ "$H3FS_CSV_IN" == /certus/.3fs-compare-* && -f "$tmp_csv" ]] && mv "$tmp_csv" "$ART/3fs.csv"
  [[ $rc -eq 0 && -s "$ART/3fs.csv" ]] && STATUS_H3FS=ok
elif [[ -n "$H3FS_NOTE" ]]; then
  echo "[compare] 3fs arm $H3FS_NOTE"
fi

if ((RUN_CPUTIER_3FS)); then
  STATUS_C3=failed
  mkdir -p "$C3_FS_TIER"
  rm -f "$ART/cputier-3fs.csv" "$C3_FS_TIER/cputier-3fs.csv"
  echo "[compare] cputier-3fs arm (fs tier on 3FS): GPU=$GPU_LIST CPU_BYTES=$DRAM ${C3_CMD[*]}"
  GPU="$GPU_LIST" CPU_BYTES="$DRAM" FS_TIER_HOST="$C3_FS_TIER" IMAGE="$IMAGE" CPUS="$NUMA_CPUS" \
    "${NUMA_CMD[@]}" "${C3_CMD[@]}" 2>&1 | tee "$ART/cputier-3fs-bench.log"
  rc=${PIPESTATUS[0]}
  [[ "$C3_CSV_IN" == /fs-tier/* && -f "$C3_FS_TIER/cputier-3fs.csv" ]] && mv "$C3_FS_TIER/cputier-3fs.csv" "$ART/cputier-3fs.csv"
  [[ $rc -eq 0 && -s "$ART/cputier-3fs.csv" ]] && STATUS_C3=ok
elif [[ -n "$C3_NOTE" ]]; then
  echo "[compare] cputier-3fs arm $C3_NOTE"
fi

echo "[compare] certus=$STATUS_CERTUS cputier=$STATUS_CPUTIER lmcache=$STATUS_LMCACHE 3fs=$STATUS_H3FS cputier-3fs=$STATUS_C3"
REPORT_ARGS=(--artifacts "$ART" --out "$OUT")
((LATEX)) && REPORT_ARGS+=(--latex "${OUT%.html}.tex")
python3 "$SCRIPT_DIR/make_report.py" "${REPORT_ARGS[@]}"
echo "[compare] report: $OUT"
[[ "$STATUS_CERTUS" != failed && "$STATUS_CPUTIER" != failed && "$STATUS_LMCACHE" != failed && "$STATUS_H3FS" != failed && "$STATUS_C3" != failed ]]
