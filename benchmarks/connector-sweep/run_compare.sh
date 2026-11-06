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
# Modes: --mode compare (the default) runs, on every arm, the modes the four
# benches drive the same way: default (store, warm, cold, mixed eviction) and
# prefix-miss, each as its own bench call. --mode default / prefix-miss /
# contention run one of them. scheduler-step is not compared: certus-connector-
# bench's runs vLLM 0.30's scheduler (one load job per request, --inflight, one
# reap per tick), the other three still merge a step's loads and wait inside the
# step, so the row would compare two harnesses, not connectors; run it with
# certus-connector-bench directly. Each bench's own --mode all is unchanged.
#
# The lmcache arm has no contention mode and is single-rank: it is skipped
# for --mode contention and for --tp > 1 (with a note).
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
#   --mode M                 compare|default|prefix-miss|contention [compare]
#                            (compare = default, then prefix-miss, on every arm)
#   --bs N [16]  --num-blocks N [1024]  --block-bytes N [2097152]
#   --min-duration S [5]  --warmup N [4]  --working-set N [512]
#   --job-blocks N [64]  --inflight N [4]  --step-us US [100]: the default mode's store,
#                            warm and cold passes and contention keep --inflight jobs of
#                            --job-blocks blocks outstanding, on EVERY arm
#   --store-job-blocks N  --store-inflight N  the store side, passed only when given
#                            [each bench: default mode --job-blocks/--inflight, contention 32/1]
#   --hit-ratio F [0.75]
#   --no-warm                skip the warm pass on every arm (then --num-blocks may exceed
#                            the memory tier: store and cold run at any size)
#   --allow-warm-overflow    run warm (and contention's loads) on every arm even when
#                            --num-blocks exceeds the memory tier; those rows are marked
#                            "OVERFLOW ... not warm" and flagged in the report
#   --lookup-order O [lookup-first]
#   --gpu N [0]  --tp N [1]
#   --layers N [32]          GPU tensors each block is split into, on EVERY arm (--layers;
#                            certus takes it as --pieces-per-block). 32 = vLLM's default
#                            layer-outermost LBHNC for Llama-3-8B; 1 = one contiguous region
#   --io-threads N [16]      disk-tier I/O threads on the file-based arms (cputier fs
#                            read AND write threads; lmcache fs_native workers)
#   --numa N [GPU's node]    NUMA node every arm's CPUs and host memory are bound to;
#                            -1 disables binding
#   --dram SIZE [12G]        certus --memory-tier-size == cputier --cpu-bytes == lmcache L1
#   --device-pci ADDR        certus NVMe device (repeatable) [the 4 vfio NVMe drives
#                            0000:61:00.0..0000:64:00.0, same as benchmarks/vllm-serve], or
#   --drive-count N          first N discovered NVMe drives (replaces the default list)
#   --eviction-threshold F   certus-server --memory-tier-eviction-threshold [0.8]; the certus
#                            bench gets the same tier (--tier-gib, --eviction-threshold)
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

OUT="" MODE=compare BS=16 NUM_BLOCKS=1024 BLOCK_BYTES=2097152 MIN_DURATION=5 WARMUP=4
WORKING_SET=512 HIT_RATIO=0.75
JOB_BLOCKS=64 INFLIGHT=4 STEP_US=100 STORE_JOB_BLOCKS="" STORE_INFLIGHT=""
LOOKUP_ORDER=lookup-first GPU=0 TP=1 DRAM=12G DEVICE_PCI=()
LAYERS=32 IO_THREADS=16 NUMA=""
DRIVE_COUNT="" EVICT_THRESHOLD=0.8 WORKERS=4 CHANNELS="" FS_TIER=/raid0/cputier-bench-fs-tier
SERVER_BIN="" IMAGE=docker.io/vllm/vllm-openai:v0.30.0
L2_DIR=/raid0/lmcache-bench-l2 LMCACHE_IMAGE=localhost/vllm-lmcache-bench:vllm0.30.0 CHUNK_SIZE=256
NO_WARM=0 ALLOW_WARM_OVERFLOW=0
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
    --job-blocks) JOB_BLOCKS="$2"; shift ;;
    --inflight) INFLIGHT="$2"; shift ;;
    --step-us) STEP_US="$2"; shift ;;
    --store-job-blocks) STORE_JOB_BLOCKS="$2"; shift ;;
    --store-inflight) STORE_INFLIGHT="$2"; shift ;;
    --hit-ratio) HIT_RATIO="$2"; shift ;;
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
    --no-warm) NO_WARM=1 ;;
    --allow-warm-overflow) ALLOW_WARM_OVERFLOW=1 ;;
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
case "$MODE" in
  compare) BENCH_MODES=(default prefix-miss) ;;
  default|prefix-miss|contention) BENCH_MODES=("$MODE") ;;
  scheduler-step|all)
    echo "error: --mode $MODE is not compared across the four benches: certus-connector-bench's" \
         "scheduler-step drives vLLM 0.30's scheduler, the others do not yet. Use --mode compare" \
         "(default + prefix-miss) here, and run scheduler-step with certus-connector-bench." >&2
    exit 2 ;;
  *) echo "error: --mode must be compare, default, prefix-miss or contention" >&2; exit 2 ;;
esac
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
# --mode is appended per bench call (run_modes), one call per entry of BENCH_MODES.
SHARED=(--bs "$BS" --num-blocks "$NUM_BLOCKS" --block-bytes "$BLOCK_BYTES"
        --min-duration "$MIN_DURATION" --warmup "$WARMUP" --working-set "$WORKING_SET"
        --job-blocks "$JOB_BLOCKS" --inflight "$INFLIGHT" --step-us "$STEP_US"
        --hit-ratio "$HIT_RATIO"
        --lookup-order "$LOOKUP_ORDER" --tp "$TP" --layers "$LAYERS")
# The store side only when given: otherwise every bench applies the same per-mode
# defaults (default mode: --job-blocks / --inflight; contention: 32 / 1).
[[ -n "$STORE_JOB_BLOCKS" ]] && SHARED+=(--store-job-blocks "$STORE_JOB_BLOCKS")
[[ -n "$STORE_INFLIGHT" ]] && SHARED+=(--store-inflight "$STORE_INFLIGHT")
((NO_WARM)) && SHARED+=(--no-warm)
((ALLOW_WARM_OVERFLOW)) && SHARED+=(--allow-warm-overflow)
# --dram in GiB: the certus bench sizes and checks its working set against the tier
# the server starts with (--tier-gib); lmcache L1 below uses the same number.
TIER_GIB=$(python3 -c "
import re; m=re.fullmatch(r'(\d+(?:\.\d+)?)([KMGT]?)', '$DRAM'.strip().upper())
print(f'{float(m.group(1)) * (1 << {\"\":0,\"K\":10,\"M\":20,\"G\":30,\"T\":40}[m.group(2)]) / 2**30:g}')")

SERVER_ARGS=(--channels "$CHANNELS" --memory-tier-size "$DRAM"
             --memory-tier-eviction-threshold "$EVICT_THRESHOLD" --shm-path "$SHM_PATH" --format)
if ((${#DEVICE_PCI[@]})); then
  for d in "${DEVICE_PCI[@]}"; do SERVER_ARGS+=(--device-pci "$d"); done
else
  SERVER_ARGS+=(--drive-count "$DRIVE_COUNT")
fi

CERTUS_CMD=(python3 benchmarks/certus-connector-bench/bench_connector_lifecycle.py "${SHARED[@]}"
            --tier-gib "$TIER_GIB" --eviction-threshold "$EVICT_THRESHOLD"
            --gpu "$GPU" --workers "$WORKERS" --shm-path "$SHM_PATH"
            --csv "$ART/certus.csv" --tag certus)
# --csv is resolved inside the container, where the repo is /certus.
REL_ART="${ART#"$REPO_ROOT"/}"
if [[ "$REL_ART" == /* ]]; then CPUTIER_CSV_IN=/fs-tier/cputier.csv; else CPUTIER_CSV_IN="/certus/$REL_ART/cputier.csv"; fi
CPUTIER_CMD=(benchmarks/cputier-connector-bench/run-in-container.sh "${SHARED[@]}"
             --fs-read-threads "$IO_THREADS" --fs-write-threads "$IO_THREADS"
             --gpu 0 --cpu-bytes "$DRAM" --csv "$CPUTIER_CSV_IN" --tag cputier)

# lmcache: one rank, no contention mode; L1 is --dram in GiB.
LMCACHE_NOTE=""
if [[ "$MODE" == contention ]]; then
  LMCACHE_NOTE="skipped: lmcache-connector-bench has no contention mode"
elif (( TP > 1 )); then
  LMCACHE_NOTE="skipped: lmcache-connector-bench is single-rank (--tp 1)"
fi
[[ -n "$LMCACHE_NOTE" ]] && RUN_LMCACHE=0
L1_GB=$TIER_GIB
if [[ "$REL_ART" == /* ]]; then LMCACHE_CSV_IN=/certus/.lmcache-compare-$$.csv; else LMCACHE_CSV_IN="/certus/$REL_ART/lmcache.csv"; fi
LMCACHE_CMD=(benchmarks/lmcache-connector-bench/run-in-container.sh "${SHARED[@]}"
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
          --numjobs "$IO_THREADS" --file-size "$H3FS_FILE_SIZE"
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
        --fs-read-threads "$IO_THREADS" --fs-write-threads "$IO_THREADS"
        --gpu 0 --cpu-bytes "$DRAM" --csv "$C3_CSV_IN" --tag cputier-3fs)

# --report-only re-renders an existing run: keep the params.json that run wrote
# (it records the run's real flags), rather than this invocation's defaults.
if (( REPORT_ONLY )) && [[ -s "$ART/params.json" ]]; then
  echo "[compare] --report-only: keeping $ART/params.json from the original run"
else
python3 - "$ART/params.json" <<EOF
import json, sys
json.dump({
  "shared": {"mode": "$MODE", "bench_modes": "${BENCH_MODES[*]}", "bs": $BS, "num_blocks": $NUM_BLOCKS, "block_bytes": $BLOCK_BYTES,
             "min_duration": $MIN_DURATION, "warmup": $WARMUP, "working_set": $WORKING_SET,
             "job_blocks": $JOB_BLOCKS, "inflight": $INFLIGHT, "step_us": $STEP_US,
             "store_job_blocks": "${STORE_JOB_BLOCKS:-per-mode default}",
             "store_inflight": "${STORE_INFLIGHT:-per-mode default}", "hit_ratio": $HIT_RATIO,
             "no_warm": bool($NO_WARM), "allow_warm_overflow": bool($ALLOW_WARM_OVERFLOW),
             "lookup_order": "$LOOKUP_ORDER", "tp": $TP, "gpus": "$GPU_LIST", "dram": "$DRAM",
             "layers": $LAYERS, "io_threads": $IO_THREADS, "numa_node": $NUMA, "numa_cpus": "$NUMA_CPUS"},
  "certus": {"channels": $CHANNELS, "eviction_threshold": $EVICT_THRESHOLD, "tier_gib": $TIER_GIB,
             "workers": $WORKERS,
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
# Each arm's container is named after this sweep, so an interrupt can stop it.
export CONTAINER_NAME="connector-sweep-$$"
INTERRUPTED=0
on_interrupt() {
  # Stop the whole sweep, not just the current arm: without this the shell
  # carried on with the next arm after the running bench was killed.
  ((INTERRUPTED)) && return
  INTERRUPTED=1
  echo "[compare] interrupted: stopping the current arm and exiting" >&2
  trap '' INT TERM
  for c in $(podman ps -q --filter "name=^${CONTAINER_NAME}" 2>/dev/null); do
    podman stop -t 10 "$c" >/dev/null 2>&1 || true
  done
  if [[ -n "${BENCH_PID:-}" ]]; then
    pkill -TERM -P "$BENCH_PID" 2>/dev/null       # the subshell running the arm
    for p in $(pgrep -P "$BENCH_PID" 2>/dev/null); do pkill -TERM -P "$p" 2>/dev/null; done
    kill -TERM "$BENCH_PID" 2>/dev/null
  fi
  # A killed bench cannot finish its own exit cleanup (podman stop gives it 10 s,
  # and an fs tier can hold hundreds of GB), so remove this sweep's per-run data
  # here: each wrapper makes a fresh run directory inside these, and no other
  # sweep can be running alongside (the arms share the GPU).
  local d
  for d in "$FS_TIER" "${C3_FS_TIER:-}"; do
    [[ -n "$d" && -d "$d" ]] || continue
    rm -rf --one-file-system "$d"/cputier-bench-* 2>/dev/null
    rm -f "$d"/cputier.csv "$d"/cputier-3fs.csv
  done
  [[ -d "$L2_DIR" ]] && rm -rf --one-file-system "$L2_DIR"/run-* 2>/dev/null
  [[ -d "$H3FS_MOCK_DIR" ]] && rm -rf --one-file-system "$H3FS_MOCK_DIR"/run-* 2>/dev/null
  if mountpoint -q "$H3FS_MOUNT" 2>/dev/null; then
    rm -rf "$H3FS_MOUNT"/vllm/bench-* 2>/dev/null
  fi
  echo "[compare] removed this sweep's per-run disk-tier data" >&2
  stop_server
  exit 130
}
trap stop_server EXIT
trap on_interrupt INT TERM

# Run one arm in the background and wait for it, so INT/TERM reach on_interrupt
# at once (bash defers traps while a foreground command runs). The arm's own
# process group gets the signal from on_interrupt via BENCH_PID. Returns the
# arm's exit code; stdout goes to the log via tee, as before.
run_arm() {  # run_arm <log> <filter-regex-or-empty> <cmd...>
  local log="$1" filt="$2"; shift 2
  local rcf; rcf=$(mktemp)
  if [[ -n "$filt" ]]; then
    ( "$@" 2>&1; echo $? > "$rcf" ) | grep --line-buffered -vE "$filt" | tee -a "$log" &
  else
    ( "$@" 2>&1; echo $? > "$rcf" ) | tee -a "$log" &
  fi
  BENCH_PID=$!
  wait "$BENCH_PID"
  BENCH_PID=""
  local rc; rc=$(cat "$rcf" 2>/dev/null || echo 1); rm -f "$rcf"
  return "${rc:-1}"
}

# One bench call per mode of BENCH_MODES, appending to one log. Each mode's rows have
# their own columns, so each call writes the bench's CSV afresh; it is set aside after
# the call and the parts are merged into that one CSV (every column of every part),
# as a bench's own --mode all writes it. <host-csv> is where the bench's --csv lands
# on the host. Returns the first failing call's exit code; stops there.
run_modes() {  # run_modes <log> <filter-regex-or-empty> <host-csv> <cmd...>
  local log="$1" filt="$2" csv="$3"; shift 3
  : > "$log"
  local m rc=0 parts=()
  for m in "${BENCH_MODES[@]}"; do
    echo "[compare] --mode $m" | tee -a "$log"
    rm -f "$csv"
    run_arm "$log" "$filt" "$@" --mode "$m"; rc=$?
    if [[ -s "$csv" ]]; then mv "$csv" "$csv.$m.part"; parts+=("$csv.$m.part"); fi
    ((rc)) && break
    ((INTERRUPTED)) && { rc=130; break; }
  done
  if ((${#parts[@]})); then
    python3 - "$csv" "${parts[@]}" <<'PY'
import csv, sys
out, parts = sys.argv[1], sys.argv[2:]
rows, cols = [], []
for p in parts:
    with open(p, newline="") as f:
        r = csv.DictReader(f)
        cols += [c for c in r.fieldnames or [] if c not in cols]
        rows += list(r)
with open(out, "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=cols)
    w.writeheader()
    w.writerows(rows)
PY
    rm -f "${parts[@]}"
  fi
  return "$rc"
}

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
      run_modes "$ART/certus-bench.log" "" "$ART/certus.csv" "${NUMA_CMD[@]}" "${CERTUS_CMD[@]}"
      [[ $? -eq 0 && -s "$ART/certus.csv" ]] && STATUS_CERTUS=ok
    else
      echo "[compare] certus-server did not become ready; see $ART/server.log" >&2
    fi
    stop_server
  fi
fi

if ((RUN_CPUTIER)); then
  STATUS_CPUTIER=failed
  rm -f "$ART/cputier.csv" "$FS_TIER/cputier.csv"
  CPUTIER_CSV_HOST=$([[ "$CPUTIER_CSV_IN" == /fs-tier/* ]] && echo "$FS_TIER/cputier.csv" || echo "$ART/cputier.csv")
  echo "[compare] cputier arm: GPU=$GPU_LIST CPU_BYTES=$DRAM ${CPUTIER_CMD[*]}"
  GPU="$GPU_LIST" CPU_BYTES="$DRAM" FS_TIER_HOST="$FS_TIER" IMAGE="$IMAGE" CPUS="$NUMA_CPUS" \
    run_modes "$ART/cputier-bench.log" "" "$CPUTIER_CSV_HOST" "${NUMA_CMD[@]}" "${CPUTIER_CMD[@]}"
  rc=$?
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
    run_modes "$ART/lmcache-bench.log" "LMCache (INFO|WARNING)|slapd|Discovered API|CudaIPCTypes" \
      "$REPO_ROOT/${LMCACHE_CSV_IN#/certus/}" "${NUMA_CMD[@]}" "${LMCACHE_CMD[@]}"
  rc=$?
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
    run_modes "$ART/3fs-bench.log" "CUDA_HOME|Triton|cpp_extension|frombuffer|UserWarning|HF3FS Connector Completed" \
      "$REPO_ROOT/${H3FS_CSV_IN#/certus/}" "${NUMA_CMD[@]}" "${H3FS_CMD[@]}"
  rc=$?
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
    run_modes "$ART/cputier-3fs-bench.log" "" \
      "$([[ "$C3_CSV_IN" == /fs-tier/* ]] && echo "$C3_FS_TIER/cputier-3fs.csv" || echo "$ART/cputier-3fs.csv")" \
      "${NUMA_CMD[@]}" "${C3_CMD[@]}"
  rc=$?
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
