#!/usr/bin/env bash
# profile_all.sh — run the four KV-offload benchmark variants against the same
# 12-turn ShareGPT replay workload and emit a side-by-side throughput table.
#
# Variants (run in this order):
#   NoOffload      GPU-only baseline                 (image certus-nooffload-bench)
#   CPUOffload     vLLM OffloadingConnector -> host RAM (image certus-cpu-offload-bench)
#   SharedStorage  llmd_fs_backend RAID0/XFS         (image certus-sharedstorage-bench)
#   Certus-SPDK    gRPC client + certus-server-yaml  (image certus-grpc-bench + host server)
#
# Each variant is preflighted independently: ready ones run, the rest are marked
# SKIPPED with a reason. Missing bench images are built only when --build is
# passed. Host RAID/XFS/hugepage/NVMe-unbind setup is NOT automated (see README).
#
# Outputs: <logdir>/<variant>.log per run, <logdir>/results.json, and a table on
# stdout. Never exits non-zero for a per-variant failure — those are reported in
# the table.
#
# Usage:
#   profile_all.sh --help
#   profile_all.sh --only nooffload,cpuoffload
#   profile_all.sh --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 \
#                  --shared-fs /mnt/fs-backend-bench --model-fs /mnt/certus1 --build

set -uo pipefail   # NOT -e: per-variant failures are handled, not fatal.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ── Defaults ──────────────────────────────────────────────────────────────────
MODEL="NousResearch/Meta-Llama-3-8B"
MODEL_FS="/mnt/certus1"
SHARED_FS=""
declare -a DEVICE_PCI=()
NUM_CONVS=450
OUTPUT_TOKENS=150
MAX_MODEL_LEN=8192
MAX_NUM_SEQS=64
GPU_MEM_UTIL=0.90
GPU="all"
MEM_TIER_SIZE="32G"
EVICT_THRESH="0.6"
CPU_BYTES=$((16 * (1 << 30)))
DRAM=$((32 * (1 << 30)))
DISK_DEV=""
SLAB_SIZE_BYTES=2097152
TENSOR_PARALLEL_SIZE=1
SERVER_WAIT=180        # seconds to wait for the Certus-SPDK server port
DO_BUILD=0
ONLY=""
SKIP=""
LOGDIR=""

# Image tags
IMG_NOOFFLOAD="certus-nooffload-bench"
IMG_CPU="certus-cpu-offload-bench"
IMG_SHARED="certus-sharedstorage-bench"
IMG_GRPC="localhost/certus-grpc-bench"

DATASET_HOST="${SCRIPT_DIR}/sharegpt_12turn_450.json"
SERVER_BIN="${REPO_ROOT}/target/release/certus-server-yaml"
# llmd_fs_backend repo (for --build of the SharedStorage image). Empty = auto:
# resolved after --model-fs is parsed, preferring <model-fs>/llm-d-kv-cache/...
# (where it lives on this host) with a $HOME fallback. Override via env.
FS_BACKEND_DIR="${FS_BACKEND_DIR:-}"

usage() {
    cat <<'EOF'
profile_all.sh — run the four KV-offload benchmark variants and print one table.

Flags (all optional; defaults shown):
  --device-pci <DDDD:BB:DD.F>   NVMe PCIe addr for the Certus-SPDK server (repeatable).
                                Absent -> Certus-SPDK is SKIPPED.
  --shared-fs <dir>             Filesystem bind-mounted to /mnt/fs-backend-bench for
                                SharedStorage. Absent -> SharedStorage is SKIPPED.
  --model-fs <dir>              Filesystem for HF cache + gRPC podman store. [/mnt/certus1]
  --model <hf-id>               Model applied to all four variants.
                                [NousResearch/Meta-Llama-3-8B]
  --num-convs <n>               Conversations to replay. [450]
  --output-tokens <n>          Generated tokens per turn (for uniform tok/s). [150]
  --max-model-len <n>          vLLM max model length. [8192]
  --max-num-seqs <n>           vLLM max concurrent sequences. [64]
  --gpu-mem-util <f>           vLLM GPU memory utilization. [0.90]
  --gpu <sel>                  CDI GPU selector (all | 0 | 0,1 | <uuid>). [all]
  --memory-tier-size <sz>      Certus-SPDK server DRAM pool (e.g. 32G). [32G]
  --evict-threshold <f>        Certus-SPDK DRAM->SSD demotion threshold. [0.6]
  --cpu-bytes <n>              CPUOffload host-RAM budget in bytes. [16Gi]
  --dram <n>                   SharedStorage DRAM budget in bytes. [32Gi]
  --disk-dev <name>            SharedStorage block device for io accounting (auto-derived
                               from --shared-fs when omitted).
  --build                      Build any missing bench image before its run
                               (SharedStorage needs FS_BACKEND_DIR; gRPC via Dockerfile).
  --only a,b                   Run only these variants.
  --skip a,b                   Skip these variants.
                               Names: nooffload, cpuoffload, sharedstorage, certus-spdk.
  --logdir <dir>               Output dir. [<model-fs>/kvprofile-<runid>]
  -h, --help                   This help.
EOF
}

# ── Arg parsing ─────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --device-pci)       DEVICE_PCI+=("$2"); shift 2;;
        --shared-fs)        SHARED_FS="$2"; shift 2;;
        --model-fs)         MODEL_FS="$2"; shift 2;;
        --model)            MODEL="$2"; shift 2;;
        --num-convs)        NUM_CONVS="$2"; shift 2;;
        --output-tokens)    OUTPUT_TOKENS="$2"; shift 2;;
        --max-model-len)    MAX_MODEL_LEN="$2"; shift 2;;
        --max-num-seqs)     MAX_NUM_SEQS="$2"; shift 2;;
        --gpu-mem-util)     GPU_MEM_UTIL="$2"; shift 2;;
        --gpu)              GPU="$2"; shift 2;;
        --memory-tier-size) MEM_TIER_SIZE="$2"; shift 2;;
        --evict-threshold)  EVICT_THRESH="$2"; shift 2;;
        --cpu-bytes)        CPU_BYTES="$2"; shift 2;;
        --dram)             DRAM="$2"; shift 2;;
        --disk-dev)         DISK_DEV="$2"; shift 2;;
        --build)            DO_BUILD=1; shift;;
        --only)             ONLY="$2"; shift 2;;
        --skip)             SKIP="$2"; shift 2;;
        --logdir)           LOGDIR="$2"; shift 2;;
        -h|--help)          usage; exit 0;;
        *) echo "error: unknown argument '$1'" >&2; usage >&2; exit 2;;
    esac
done

# ── Derived paths ─────────────────────────────────────────────────────────────
HF_CACHE="${MODEL_FS}/hf-cache"
PODMAN_STORE="${MODEL_FS}/podman/storage"
PODMAN_RUNROOT="${MODEL_FS}/podman/run"
RUNID="$(date +%H%M%S 2>/dev/null || echo run)_$$"
[[ -z "$LOGDIR" ]] && LOGDIR="${MODEL_FS}/kvprofile-${RUNID}"

# Auto-resolve the fs-backend repo if not set: prefer the model-fs copy (this
# host keeps it at <model-fs>/llm-d-kv-cache/kv_connectors/llmd_fs_backend),
# fall back to $HOME.
if [[ -z "$FS_BACKEND_DIR" ]]; then
    if [[ -f "${MODEL_FS}/llm-d-kv-cache/kv_connectors/llmd_fs_backend/Dockerfile.wheel" ]]; then
        FS_BACKEND_DIR="${MODEL_FS}/llm-d-kv-cache/kv_connectors/llmd_fs_backend"
    else
        FS_BACKEND_DIR="$HOME/llm-d-kv-cache/kv_connectors/llmd_fs_backend"
    fi
fi

# ── Result accumulation (parallel arrays keyed by index) ──────────────────────
declare -a R_VARIANT=() R_STATUS=() R_WALL=() R_ROUNDS=() R_GENS=() R_TPS=() R_NATIVE=() R_REASON=() R_LOG=()

log()  { echo "[profile] $*"; }
warn() { echo "[profile] WARN: $*" >&2; }

# Selection helpers
want() {
    local v="$1"
    if [[ -n "$ONLY" ]]; then [[ ",$ONLY," == *",$v,"* ]] || return 1; fi
    if [[ -n "$SKIP" ]]; then [[ ",$SKIP," == *",$v,"* ]] && return 1; fi
    return 0
}

record() {  # variant status wall rounds gens tps native reason log
    R_VARIANT+=("$1"); R_STATUS+=("$2"); R_WALL+=("$3"); R_ROUNDS+=("$4")
    R_GENS+=("$5"); R_TPS+=("$6"); R_NATIVE+=("$7"); R_REASON+=("$8"); R_LOG+=("$9")
}

# Parse a driver log; echoes "wall|gens|rounds|native|tps".
parse_log() {
    local f="$1" wall gens rounds native tps
    wall="$(grep -oE '(wall|elapsed)=[0-9.]+' "$f" 2>/dev/null | tail -1 | cut -d= -f2)"
    gens="$(grep -oE 'generations=[0-9]+' "$f" 2>/dev/null | tail -1 | cut -d= -f2)"
    rounds="$(grep -oE 'rounds=[0-9]+' "$f" 2>/dev/null | tail -1 | cut -d= -f2)"
    # Native throughput line differs per driver: "tok/s=V" or "(V gen/s)".
    native="$(grep -oE 'tok/s=[0-9.]+' "$f" 2>/dev/null | tail -1 | cut -d= -f2)"
    [[ -z "$native" ]] && native="$(grep -oE '\(([0-9.]+) gen/s\)' "$f" 2>/dev/null | tail -1 | grep -oE '[0-9.]+')"
    tps=""
    if [[ -n "$wall" && -n "$gens" ]]; then
        tps="$(awk -v g="$gens" -v t="$OUTPUT_TOKENS" -v w="$wall" 'BEGIN{ if(w>0) printf "%.0f", g*t/w; }')"
    fi
    echo "${wall}|${gens}|${rounds}|${native}|${tps}"
}

finish_variant() {  # variant rc logfile
    local variant="$1" rc="$2" f="$3"
    local parsed wall gens rounds native tps
    parsed="$(parse_log "$f")"
    IFS='|' read -r wall gens rounds native tps <<<"$parsed"
    if [[ "$rc" -eq 0 && -n "$tps" ]]; then
        record "$variant" "OK" "$wall" "$rounds" "$gens" "$tps" "$native" "" "$f"
        log "$variant OK: wall=${wall}s rounds=${rounds} gens=${gens} tokens/s=${tps}"
    else
        record "$variant" "FAILED" "$wall" "$rounds" "$gens" "$tps" "$native" "exit=$rc (see $(basename "$f"))" "$f"
        warn "$variant FAILED (exit=$rc); see $f"
    fi
}

# ── Preflight & environment ───────────────────────────────────────────────────
log "run id ${RUNID}"
log "model=${MODEL} num_convs=${NUM_CONVS} output_tokens=${OUTPUT_TOKENS}"
log "logdir=${LOGDIR}"
mkdir -p "$LOGDIR" "$HF_CACHE" || { echo "error: cannot create logdir/HF cache" >&2; exit 1; }

if [[ ! -f "$DATASET_HOST" ]]; then
    warn "dataset $DATASET_HOST not found on host (images bake their own copy; container runs are unaffected)"
fi

# GPU-free check (informational).
if command -v nvidia-smi >/dev/null 2>&1; then
    used="$(nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits 2>/dev/null | sort -rn | head -1)"
    if [[ -n "$used" && "$used" -gt 1024 ]]; then
        warn "a GPU already has ${used} MiB in use — a stale process may starve the benchmark"
    fi
else
    warn "nvidia-smi not found — cannot verify GPU availability"
fi

# Reap stale bench containers (the earlier GPU-pin foot-gun).
reap() {
    local names ids
    names="$(command podman ps -a --format '{{.ID}} {{.Names}}' 2>/dev/null | grep -E 'certus-(nooffload|cpu-offload|sharedstorage|grpc)-bench|-bench$' | awk '{print $1}')"
    if [[ -n "$names" ]]; then
        warn "reaping stale bench containers: $(echo "$names" | tr '\n' ' ')"
        echo "$names" | xargs -r command podman rm -f >/dev/null 2>&1
    fi
    # Same for the gRPC store.
    ids="$(command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" ps -a --format '{{.ID}} {{.Names}}' 2>/dev/null | grep -E 'grpc-bench|-bench$' | awk '{print $1}')"
    [[ -n "$ids" ]] && echo "$ids" | xargs -r command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" rm -f >/dev/null 2>&1
}
reap

img_exists()      { command podman image exists "$1" >/dev/null 2>&1; }
img_exists_grpc() { command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" image exists "$1" >/dev/null 2>&1; }

# Common container run for the three self-contained images (default podman store).
run_container_bench() {  # variant image extra-args...
    local variant="$1" image="$2"; shift 2
    local extra=("$@") f="${LOGDIR}/${variant}.log"
    log "starting ${variant} (image ${image})"
    command podman run --rm \
        --pull=never \
        --device "nvidia.com/gpu=${GPU}" \
        -e "MODEL=${MODEL}" \
        -e "NUM_CONVS=${NUM_CONVS}" \
        -e "OUTPUT_TOKENS=${OUTPUT_TOKENS}" \
        -e "MAX_MODEL_LEN=${MAX_MODEL_LEN}" \
        -e "MAX_NUM_SEQS=${MAX_NUM_SEQS}" \
        -e "GPU_MEM_UTIL=${GPU_MEM_UTIL}" \
        -e "HF_HUB_OFFLINE=0" \
        -v "${HF_CACHE}:/root/.cache/huggingface" \
        "${extra[@]}" \
        "$image" 2>&1 | tee "$f"
    local rc="${PIPESTATUS[0]}"
    finish_variant "$variant" "$rc" "$f"
}

# ══ NoOffload ═════════════════════════════════════════════════════════════════
if want nooffload; then
    if ! img_exists "$IMG_NOOFFLOAD"; then
        record "NoOffload" "SKIPPED" "" "" "" "" "" "image ${IMG_NOOFFLOAD} missing (build it: podman build -f benchmarks/kv-offload-replay/Dockerfile.nooffload -t ${IMG_NOOFFLOAD} .)" ""
        warn "NoOffload SKIPPED: image ${IMG_NOOFFLOAD} not found"
    else
        run_container_bench "NoOffload" "$IMG_NOOFFLOAD"
    fi
fi

# ══ CPUOffload ════════════════════════════════════════════════════════════════
if want cpuoffload; then
    if ! img_exists "$IMG_CPU"; then
        record "CPUOffload" "SKIPPED" "" "" "" "" "" "image ${IMG_CPU} missing" ""
        warn "CPUOffload SKIPPED: image ${IMG_CPU} not found"
    else
        run_container_bench "CPUOffload" "$IMG_CPU" -e "CPU_BYTES=${CPU_BYTES}"
    fi
fi

# ══ SharedStorage ═════════════════════════════════════════════════════════════
if want sharedstorage; then
    ss_skip=""
    if [[ -z "$SHARED_FS" ]]; then
        ss_skip="no --shared-fs given (bind target /mnt/fs-backend-bench)"
    elif [[ ! -d "$SHARED_FS" ]]; then
        ss_skip="--shared-fs '$SHARED_FS' is not a directory"
    fi
    if [[ -z "$ss_skip" ]] && ! img_exists "$IMG_SHARED"; then
        if [[ "$DO_BUILD" -eq 1 ]]; then
            if [[ -f "${FS_BACKEND_DIR}/Dockerfile.wheel" ]]; then
                log "building ${IMG_SHARED} (FS_BACKEND_DIR=${FS_BACKEND_DIR})"
                ( cd "$SCRIPT_DIR" && FS_BACKEND_DIR="$FS_BACKEND_DIR" bash build-sharedstorage.sh ) \
                    > "${LOGDIR}/build-sharedstorage.log" 2>&1 \
                    || ss_skip="build failed (see build-sharedstorage.log)"
            else
                ss_skip="image missing and FS_BACKEND_DIR '${FS_BACKEND_DIR}' has no Dockerfile.wheel"
            fi
        else
            ss_skip="image ${IMG_SHARED} missing (pass --build with FS_BACKEND_DIR set)"
        fi
    fi
    if [[ -n "$ss_skip" ]]; then
        record "SharedStorage" "SKIPPED" "" "" "" "" "" "$ss_skip" ""
        warn "SharedStorage SKIPPED: $ss_skip"
    else
        mkdir -p "${SHARED_FS}/shared-kv"
        dev="$DISK_DEV"
        if [[ -z "$dev" ]]; then
            dev="$(findmnt -no SOURCE --target "$SHARED_FS" 2>/dev/null | xargs -r basename)"
            [[ -z "$dev" ]] && dev="md0"
        fi
        run_container_bench "SharedStorage" "$IMG_SHARED" \
            -v "${SHARED_FS}:/mnt/fs-backend-bench" \
            -e "DRAM=${DRAM}" \
            -e "DISK_DEV=${dev}" \
            -e "SKIP_PREFLIGHT=1"
    fi
fi

# ══ Certus-SPDK ═══════════════════════════════════════════════════════════════
SERVER_PID=""
stop_server() {
    [[ -z "$SERVER_PID" ]] && return 0
    kill -0 "$SERVER_PID" 2>/dev/null || { SERVER_PID=""; return 0; }
    log "stopping Certus-SPDK server (pid ${SERVER_PID})"
    kill -TERM "$SERVER_PID" 2>/dev/null
    for _ in $(seq 1 8); do
        kill -0 "$SERVER_PID" 2>/dev/null || { SERVER_PID=""; return 0; }
        sleep 1
    done
    warn "server ignored SIGTERM (SPDK teardown) — escalating to SIGKILL"
    kill -9 "$SERVER_PID" 2>/dev/null
    wait "$SERVER_PID" 2>/dev/null
    SERVER_PID=""
}
trap stop_server EXIT

if want certus-spdk; then
    cs_skip=""
    if [[ ${#DEVICE_PCI[@]} -eq 0 ]]; then
        cs_skip="no --device-pci given (Certus-SPDK server needs an NVMe device)"
    elif [[ ! -x "$SERVER_BIN" ]]; then
        cs_skip="server binary not built at ${SERVER_BIN} (cargo build --release -p certus-server-yaml)"
    fi
    if [[ -z "$cs_skip" ]] && ! img_exists_grpc "$IMG_GRPC"; then
        if [[ "$DO_BUILD" -eq 1 ]]; then
            log "building ${IMG_GRPC} into ${PODMAN_STORE}"
            command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" build \
                -f "${REPO_ROOT}/certus-grpc-connector/Dockerfile" \
                -t certus-grpc-bench "$REPO_ROOT" \
                > "${LOGDIR}/build-grpc.log" 2>&1 \
                || cs_skip="gRPC image build failed (see build-grpc.log)"
        else
            cs_skip="image ${IMG_GRPC} missing (pass --build)"
        fi
    fi

    if [[ -n "$cs_skip" ]]; then
        record "Certus-SPDK" "SKIPPED" "" "" "" "" "" "$cs_skip" ""
        warn "Certus-SPDK SKIPPED: $cs_skip"
    else
        dev_flags=()
        for d in "${DEVICE_PCI[@]}"; do dev_flags+=(--device-pci "$d"); done
        log "starting Certus-SPDK server: ${dev_flags[*]} --memory-tier-size ${MEM_TIER_SIZE}"
        "$SERVER_BIN" "${dev_flags[@]}" \
            --memory-tier-size "$MEM_TIER_SIZE" \
            --memory-tier-eviction-threshold "$EVICT_THRESH" \
            --listen 0.0.0.0:50051 \
            --format \
            > "${LOGDIR}/server.log" 2>&1 &
        SERVER_PID=$!

        # Poll for the listen port.
        up=0
        for _ in $(seq 1 "$SERVER_WAIT"); do
            if ! kill -0 "$SERVER_PID" 2>/dev/null; then break; fi
            if (exec 3<>/dev/tcp/127.0.0.1/50051) 2>/dev/null; then exec 3>&- 3<&-; up=1; break; fi
            sleep 1
        done

        if [[ "$up" -ne 1 ]]; then
            record "Certus-SPDK" "SKIPPED" "" "" "" "" "" "server :50051 did not come up within ${SERVER_WAIT}s (see server.log)" "${LOGDIR}/server.log"
            warn "Certus-SPDK SKIPPED: server did not come up"
            stop_server
        else
            log "server up on :50051 — launching gRPC client"
            f="${LOGDIR}/certus-spdk.log"
            IMAGE="$IMG_GRPC" \
            GPU="$GPU" \
            CERTUS_SERVER="host.containers.internal:50051" \
            NUM_CONVS="$NUM_CONVS" \
            MODEL="$MODEL" \
            SLAB_SIZE_BYTES="$SLAB_SIZE_BYTES" \
            TENSOR_PARALLEL_SIZE="$TENSOR_PARALLEL_SIZE" \
            HF_CACHE="$HF_CACHE" \
            PODMAN_STORE="$PODMAN_STORE" \
            PODMAN_RUNROOT="$PODMAN_RUNROOT" \
                bash "${REPO_ROOT}/certus-grpc-connector/run-bench.sh" 2>&1 | tee "$f"
            rc="${PIPESTATUS[0]}"
            finish_variant "Certus-SPDK" "$rc" "$f"
            stop_server
        fi
    fi
fi

# ── Emit results.json ─────────────────────────────────────────────────────────
json="${LOGDIR}/results.json"
{
    echo "{"
    echo "  \"model\": \"${MODEL}\","
    echo "  \"num_convs\": ${NUM_CONVS},"
    echo "  \"output_tokens\": ${OUTPUT_TOKENS},"
    echo "  \"logdir\": \"${LOGDIR}\","
    echo "  \"variants\": ["
    n=${#R_VARIANT[@]}
    for i in "${!R_VARIANT[@]}"; do
        wall="${R_WALL[$i]:-null}";   [[ -z "$wall"   || "$wall" == "null" ]]   && wall=null
        rounds="${R_ROUNDS[$i]:-null}"; [[ -z "$rounds" || "$rounds" == "null" ]] && rounds=null
        gens="${R_GENS[$i]:-null}";   [[ -z "$gens"   || "$gens" == "null" ]]   && gens=null
        tps="${R_TPS[$i]:-null}";     [[ -z "$tps"    || "$tps" == "null" ]]    && tps=null
        native="${R_NATIVE[$i]:-null}"; [[ -z "$native" || "$native" == "null" ]] && native=null
        reason="${R_REASON[$i]//\"/\\\"}"
        logp="${R_LOG[$i]}"
        printf '    {"variant": "%s", "status": "%s", "wall_s": %s, "rounds": %s, "generations": %s, "tokens_per_sec": %s, "native_metric": %s, "reason": "%s", "log": "%s"}' \
            "${R_VARIANT[$i]}" "${R_STATUS[$i]}" "$wall" "$rounds" "$gens" "$tps" "$native" "$reason" "$logp"
        [[ $((i+1)) -lt $n ]] && echo "," || echo ""
    done
    echo "  ]"
    echo "}"
} > "$json"

# ── Human table ───────────────────────────────────────────────────────────────
echo ""
echo "=============================== KV-Offload Profile ==============================="
echo "model=${MODEL}  num_convs=${NUM_CONVS}  output_tokens=${OUTPUT_TOKENS}"
echo "logdir=${LOGDIR}"
echo ""
printf "%-15s %-10s %10s %7s %8s %10s\n" "Variant" "Status" "wall(s)" "rounds" "gens" "tokens/s"
printf "%-15s %-10s %10s %7s %8s %10s\n" "---------------" "----------" "----------" "-------" "--------" "----------"
for i in "${!R_VARIANT[@]}"; do
    if [[ "${R_STATUS[$i]}" == "OK" ]]; then
        printf "%-15s %-10s %10s %7s %8s %10s\n" \
            "${R_VARIANT[$i]}" "OK" "${R_WALL[$i]:--}" "${R_ROUNDS[$i]:--}" "${R_GENS[$i]:--}" "${R_TPS[$i]:--}"
    else
        printf "%-15s %-10s %s\n" "${R_VARIANT[$i]}" "${R_STATUS[$i]}" "(${R_REASON[$i]})"
    fi
done
echo "================================================================================="
echo "results.json -> ${json}"
