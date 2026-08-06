#!/usr/bin/env bash
# profile_all.sh — run the four KV-offload benchmark variants against the same
# 12-turn ShareGPT replay workload and emit a side-by-side throughput table.
#
# Variants (run in this order):
#   NoOffload      GPU-only baseline                 (image certus-nooffload-bench)
#   Certus-SPDK    gRPC client + certus-server-yaml  (image certus-grpc-bench + host server)
#   CPUOffload     vLLM OffloadingConnector -> host RAM (image certus-cpu-offload-bench)
#   SharedStorage  llmd_fs_backend RAID0/XFS         (image certus-sharedstorage-bench)
#
# Certus-SPDK runs first (of the storage backends) on purpose: it consumes the
# boot-reserved 1G hugepage pool while it is still intact (no runtime realloc, no
# reboot). Once it has run, the pool is released back to normal RAM (see
# free_1g_hugepages) so the host-RAM backends (CPUOffload) and the XFS page cache
# (SharedStorage) get that ~16 GiB back under mem=32G.
#
# Each variant is preflighted independently: ready ones run, the rest are marked
# SKIPPED with a reason. Missing bench images are built only when --build is
# passed. The shared NVMe group (--device-pci) is reconfigured in-run between the
# SharedStorage (kernel nvme + RAID0/XFS) and Certus-SPDK (vfio-pci + 1G hugepages)
# phases via tools/configure-bench.sh — so all storage backends use the SAME drives.
# This is runtime-only (no reboot); a reboot is requested only if the 1G-hugepage
# allocation falls short. Needs sudo (cached once, up front).
#
# Outputs: <logdir>/<variant>.log per run, <logdir>/result-<variant>.json flushed as
# each backend finishes (survives a crash or a --only subset), <logdir>/results.json
# aggregate at the end, and a table on stdout. Never exits non-zero for a per-variant
# failure — those are reported in the table.
#
# Usage:
#   profile_all.sh --help
#   profile_all.sh --only nooffload,cpuoffload
#   profile_all.sh --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 \
#                  --model-fs /mnt/fs-backend-bench --build

set -uo pipefail   # NOT -e: per-variant failures are handled, not fatal.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ── Defaults ──────────────────────────────────────────────────────────────────
MODEL="NousResearch/Meta-Llama-3-8B"
MODEL_FS="/mnt/certus1"
declare -a DEVICE_PCI=()
NUM_CONVS=450
MAX_ROUNDS=0           # 0 = replay all turns; N caps every backend at N rounds/turns
OUTPUT_TOKENS=150
MAX_MODEL_LEN=8192
MAX_NUM_SEQS=64
GPU_MEM_UTIL=0.90
GPU="all"
MEM_TIER_SIZE="32G"
EVICT_THRESH="0.6"
CPU_BYTES=$((16 * (1 << 30)))
DRAM=$((32 * (1 << 30)))
SLAB_SIZE_BYTES=2097152
TENSOR_PARALLEL_SIZE=1
SERVER_WAIT=180        # seconds to wait for the Certus-SPDK server port
DO_BUILD=0
VLLM_VERSION=""        # pin the vLLM base-image version for ALL four backends
ONLY=""
SKIP=""
LOGDIR=""

# Image tags. Env-overridable (a caller can point this at externally-built
# images). With --vllm-version set, an untagged name here gets a :vllm<ver> tag
# appended below so multiple versions coexist.
IMG_NOOFFLOAD="${IMG_NOOFFLOAD:-certus-nooffload-bench}"
IMG_CPU="${IMG_CPU:-certus-cpu-offload-bench}"
IMG_SHARED="${IMG_SHARED:-certus-sharedstorage-bench}"
IMG_GRPC="${IMG_GRPC:-localhost/certus-grpc-bench}"

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
  --device-pci <DDDD:BB:DD.F>   NVMe PCIe addr of the SHARED drive group (repeatable).
                                Used for BOTH SharedStorage (RAID0/XFS) and Certus-SPDK
                                (vfio/SPDK): the host is reconfigured onto this group
                                between the two phases via tools/configure-bench.sh, so
                                the storage backends compare on identical devices.
                                [default 0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0]
  --model-fs <dir>              Filesystem for HF cache + gRPC podman store. [/mnt/certus1]
  --model <hf-id>               Model applied to all four variants.
                                [NousResearch/Meta-Llama-3-8B]
  --num-convs <n>               Conversations to replay. [450]
  --max-rounds <n>              Cap every backend at N rounds/turns (MAX_ROUNDS env).
                                0 = replay all 12 turns. [0]
  --output-tokens <n>          Generated tokens per turn (for uniform tok/s). [150]
  --max-model-len <n>          vLLM max model length. [8192]
  --max-num-seqs <n>           vLLM max concurrent sequences. [64]
  --gpu-mem-util <f>           vLLM GPU memory utilization. [0.90]
  --gpu <sel>                  CDI GPU selector (all | 0 | 0,1 | <uuid>). [all]
  --memory-tier-size <sz>      Certus-SPDK server DRAM pool (e.g. 32G). [32G]
  --evict-threshold <f>        Certus-SPDK DRAM->SSD demotion threshold. [0.6]
  --cpu-bytes <n>              CPUOffload host-RAM budget in bytes. [16Gi]
  --dram <n>                   SharedStorage DRAM budget in bytes. [32Gi]
  --build                      Build any missing bench image before its run
                               (SharedStorage needs FS_BACKEND_DIR; gRPC via Dockerfile).
  --vllm-version <x.y.z>       Pin the vLLM base-image version for ALL four backends
                               (--build-arg VLLM_VERSION). Images are tagged
                               :vllm<x.y.z> so versions coexist. Implies the images
                               must be built at that version — pass --build too (or
                               pre-build them). SharedStorage's compiled wheel ABI is
                               auto-matched to the base image's torch on build.
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
        --model-fs)         MODEL_FS="$2"; shift 2;;
        --model)            MODEL="$2"; shift 2;;
        --num-convs)        NUM_CONVS="$2"; shift 2;;
        --max-rounds)       MAX_ROUNDS="$2"; shift 2;;
        --output-tokens)    OUTPUT_TOKENS="$2"; shift 2;;
        --max-model-len)    MAX_MODEL_LEN="$2"; shift 2;;
        --max-num-seqs)     MAX_NUM_SEQS="$2"; shift 2;;
        --gpu-mem-util)     GPU_MEM_UTIL="$2"; shift 2;;
        --gpu)              GPU="$2"; shift 2;;
        --memory-tier-size) MEM_TIER_SIZE="$2"; shift 2;;
        --evict-threshold)  EVICT_THRESH="$2"; shift 2;;
        --cpu-bytes)        CPU_BYTES="$2"; shift 2;;
        --dram)             DRAM="$2"; shift 2;;
        --build)            DO_BUILD=1; shift;;
        --vllm-version)     VLLM_VERSION="$2"; shift 2;;
        --only)             ONLY="$2"; shift 2;;
        --skip)             SKIP="$2"; shift 2;;
        --logdir)           LOGDIR="$2"; shift 2;;
        -h|--help)          usage; exit 0;;
        *) echo "error: unknown argument '$1'" >&2; usage >&2; exit 2;;
    esac
done

# Reject unknown --only/--skip tokens up front. want() does substring-on-comma
# matching, so a typo (e.g. --only cpu, --only certus) silently selects nothing and
# that variant just never runs — fail loudly instead.
VALID_VARIANTS="nooffload cpuoffload certus-spdk sharedstorage"
for _list in "$ONLY" "$SKIP"; do
    [[ -z "$_list" ]] && continue
    IFS=',' read -ra _toks <<<"$_list"
    for _t in "${_toks[@]}"; do
        [[ -z "$_t" ]] && continue
        [[ " $VALID_VARIANTS " == *" $_t "* ]] || {
            echo "error: unknown variant '${_t}' in --only/--skip" >&2
            echo "       valid variants: ${VALID_VARIANTS// /, }" >&2
            exit 2
        }
    done
done

# ── Derived paths ─────────────────────────────────────────────────────────────
# HF cache defaults under the model-fs but is env-overridable (this host keeps
# the populated cache at ~/.cache/huggingface, not on the model-fs).
HF_CACHE="${HF_CACHE:-${MODEL_FS}/hf-cache}"
PODMAN_STORE="${MODEL_FS}/podman/storage"
PODMAN_RUNROOT="${MODEL_FS}/podman/run"
RUNID="$(date +%H%M%S 2>/dev/null || echo run)_$$"

# ── vLLM version pinning ──────────────────────────────────────────────────────
# When --vllm-version is set, all builds get --build-arg VLLM_VERSION and each
# untagged image name gets a :vllm<ver> tag so versions don't collide.
declare -a BUILD_ARGS=()
VER_LABEL=""
if [[ -n "$VLLM_VERSION" ]]; then
    BUILD_ARGS=(--build-arg "VLLM_VERSION=${VLLM_VERSION}")
    VER_LABEL="vllm${VLLM_VERSION}-"
    _tag=":vllm${VLLM_VERSION}"
    [[ "$IMG_NOOFFLOAD" != *:* ]] && IMG_NOOFFLOAD+="$_tag"
    [[ "$IMG_CPU"       != *:* ]] && IMG_CPU+="$_tag"
    [[ "$IMG_SHARED"    != *:* ]] && IMG_SHARED+="$_tag"
    [[ "$IMG_GRPC"      != *:* ]] && IMG_GRPC+="$_tag"
fi

[[ -z "$LOGDIR" ]] && LOGDIR="${MODEL_FS}/kvprofile-${VER_LABEL}${RUNID}"

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

# ── Shared NVMe group + host reconfiguration ──────────────────────────────────
# All storage backends share ONE NVMe group. A device is either kernel-bound with
# XFS (SharedStorage) or vfio-bound for SPDK (Certus-SPDK) — never both at once —
# so the host is flipped between the two via tools/configure-bench.sh, in-run.
# Default group = configure-bench.sh's node-0 set. MEM_METHOD is left to that
# script's default (kernel; mem=32G is already active on this host, no reboot);
# forward it only if the caller set it.
if [[ ${#DEVICE_PCI[@]} -eq 0 ]]; then
    DEVICE_PCI=(0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0)
fi
NVME_BDFS="${DEVICE_PCI[*]}"
CONFIG_SH="${REPO_ROOT}/tools/configure-bench.sh"
HUGEPAGES_1G_TARGET="${CERTUS_HUGEPAGES:-16}"   # configure-bench.sh certus default
HUGEPAGES_1G_NODE="${RESOURCE_NUMA:-0}"
# The shared NVMe group ($NVME_BDFS) is reconfigured into a RAID0/XFS for
# SharedStorage and vfio for Certus-SPDK. The RAID's device/mount/label are chosen
# automatically (no flags) so they can never collide with another array on the host
# — e.g. a separate persistent model-fs RAID at /dev/md0 // /mnt/fs-backend-bench:
#   device — reuse whatever is already mounted at the SharedStorage mountpoint (a
#            leftover from an interrupted run), else the lowest /dev/mdN that does
#            not yet exist (md0 taken by model-fs -> md1, and so on);
#   mount  — a dedicated SharedStorage mountpoint nothing else uses;
#   label  — a distinct XFS label.
# certus' teardown_raid_if_active acts on these same values (forwarded below), so it
# only ever stops the shared-group RAID — never the model-fs array.
SHARED_FS="/mnt/ss-kv"
SS_XFS_LABEL="sskv"
DISK_DEV="$(findmnt -no SOURCE --target "$SHARED_FS" 2>/dev/null | xargs -r basename)"
if [[ -z "$DISK_DEV" ]]; then
    _n=0; while [[ -e "/dev/md${_n}" ]]; do _n=$((_n + 1)); done; DISK_DEV="md${_n}"
fi

# Reconfigure the shared NVMe group for a phase via tools/configure-bench.sh.
#   sharedstorage -> kernel nvme + RAID0/XFS at $SHARED_FS
#   certus        -> vfio-pci + 1G hugepages (SPDK)
# Runtime-only; configure-bench.sh never reboots. Output tee'd for audit. Assumes
# sudo was already cached (see preflight). Returns configure-bench.sh's exit code.
reconfigure() {  # mode
    local mode="$1"
    local f="${LOGDIR}/reconfigure-${mode}.log"
    # The 1G hugepage BOOT reservation is global host state and must satisfy the
    # most-demanding selected variant. Certus-SPDK needs CERTUS_HUGEPAGES reserved
    # at boot; SharedStorage needs none and normally sets hugepages=0. But in a
    # combined run the phases go certus -> free -> sharedstorage, so letting the
    # sharedstorage phase write 0 clobbers the reservation the certus phase just
    # set — and the required "reboot once and re-run" can then never get its pool
    # (it re-skips and re-writes 0, forever). Pin the sharedstorage phase's boot
    # count to CERTUS_HUGEPAGES whenever certus-spdk is also selected; its runtime
    # pages are still released (free_1g_hugepages) so SharedStorage's RAM is fair.
    local hp_env=()
    if [[ "$mode" == "sharedstorage" ]] && want certus-spdk; then
        hp_env=(SS_HUGEPAGES="${CERTUS_HUGEPAGES:-16}")
    fi
    log "reconfigure host -> ${mode} on [${NVME_BDFS}] (see reconfigure-${mode}.log)"
    sudo env \
        NVME_BDFS="$NVME_BDFS" \
        MD_DEVICE="/dev/${DISK_DEV}" \
        MOUNT_POINT="$SHARED_FS" \
        XFS_LABEL="$SS_XFS_LABEL" \
        ${MEM_METHOD:+MEM_METHOD="$MEM_METHOD"} \
        ${CERTUS_HUGEPAGES:+CERTUS_HUGEPAGES="$CERTUS_HUGEPAGES"} \
        "${hp_env[@]}" \
        ${RESOURCE_NUMA:+RESOURCE_NUMA="$RESOURCE_NUMA"} \
        "$CONFIG_SH" "$mode" > "$f" 2>&1
}

# Release the boot-reserved 1G hugepages back to normal RAM (runtime; no reboot).
# Only Certus-SPDK needs the pool, so once it has run we free the ~16 GiB so the
# host-RAM backends (CPUOffload) and page cache aren't starved under mem=32G. Safe
# after stop_server; pages still held by a live process simply won't drop to 0.
free_1g_hugepages() {
    log "releasing 1G hugepages -> RAM (post Certus-SPDK; see free-hugepages.log)"
    sudo bash -c '
        for f in /sys/devices/system/node/node*/hugepages/hugepages-1048576kB/nr_hugepages; do
            [ -w "$f" ] && echo 0 > "$f"
        done
        awk "/HugePages_Total/ && \$2!=0 {print \"WARN: \" \$2 \" hugepages still reserved (held by a process?)\"}" /proc/meminfo
    ' > "${LOGDIR}/free-hugepages.log" 2>&1 || warn "free_1g_hugepages: see free-hugepages.log"
}

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

# Emit "null" for an empty/"null" numeric field, else the raw value (JSON literal).
_num_or_null() { local v="$1"; { [[ -z "$v" || "$v" == "null" ]] && echo null; } || echo "$v"; }

# Print one variant's JSON object (args in record() order). Shared by the per-variant
# result files and the aggregate results.json so both stay byte-for-byte consistent.
variant_json() {  # variant status wall rounds gens tps native reason log
    printf '{"variant": "%s", "status": "%s", "wall_s": %s, "rounds": %s, "generations": %s, "tokens_per_sec": %s, "native_metric": %s, "reason": "%s", "log": "%s"}' \
        "$1" "$2" "$(_num_or_null "$3")" "$(_num_or_null "$4")" "$(_num_or_null "$5")" \
        "$(_num_or_null "$6")" "$(_num_or_null "$7")" "${8//\"/\\\"}" "$9"
}

record() {  # variant status wall rounds gens tps native reason log
    R_VARIANT+=("$1"); R_STATUS+=("$2"); R_WALL+=("$3"); R_ROUNDS+=("$4")
    R_GENS+=("$5"); R_TPS+=("$6"); R_NATIVE+=("$7"); R_REASON+=("$8"); R_LOG+=("$9")
    # Flush this variant's result to its own file the instant it finishes, so a later
    # crash (or a --only subset run) can never lose an already-completed backend. The
    # aggregate results.json is still written at the end from the in-memory arrays.
    if [[ -n "${LOGDIR:-}" && -d "${LOGDIR:-}" ]]; then
        local slug; slug="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
        variant_json "$@" > "${LOGDIR}/result-${slug}.json"
    fi
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

# Reconfiguring the shared NVMe group (RAID0/XFS <-> vfio/hugepages) needs root.
# Cache the sudo credential ONCE, up front — before any long-running work — but
# only when a storage backend is actually selected.
if want sharedstorage || want certus-spdk; then
    log "shared NVMe group for storage backends: [${NVME_BDFS}]"
    # Fail fast on a mistyped or absent PCI address: every device in the group must
    # exist in sysfs. Otherwise a bad BDF (e.g. a dropped domain digit, 000:61:00.0
    # instead of 0000:61:00.0) aborts configure-bench.sh deep inside its rebind loop
    # with a cryptic driver_override ENOENT, skipping the storage backend after work
    # has already been spent on the others.
    missing=()
    for bdf in "${DEVICE_PCI[@]}"; do
        [[ -e "/sys/bus/pci/devices/${bdf}" ]] || missing+=("$bdf")
    done
    if (( ${#missing[@]} )); then
        echo "error: NVMe device(s) not present on this host: ${missing[*]}" >&2
        echo "       checked /sys/bus/pci/devices/<bdf>; each --device-pci must be a full" >&2
        echo "       PCI address DDDD:BB:DD.F with a four-digit domain (e.g. 0000:61:00.0)." >&2
        echo "       NVMe controllers present on this host:" >&2
        for d in /sys/bus/pci/devices/*; do
            [[ "$(cat "$d/class" 2>/dev/null)" == 0x0108* ]] && echo "         $(basename "$d")" >&2
        done
        exit 1
    fi
    # Confirm we can sudo. `sudo -n true` succeeds without a tty when sudo is
    # passwordless/NOPASSWD or already cached, so the whole run works headless
    # (background, no controlling terminal). Only when that fails do we fall back
    # to interactive `sudo -v`, which prompts once up front on hosts that require
    # a password (and have a tty to type it into).
    if ! sudo -n true 2>/dev/null && ! sudo -v; then
        echo "error: sudo is required to reconfigure the NVMe group via ${CONFIG_SH}" >&2
        exit 1
    fi
fi

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

# Build a self-contained image (default store) from one of our Dockerfiles,
# honoring any --vllm-version build-arg. Returns build rc.
build_simple() {  # image dockerfile logtag
    log "building $1 from $2 ${VLLM_VERSION:+(vLLM ${VLLM_VERSION})}"
    command podman build "${BUILD_ARGS[@]}" -f "${SCRIPT_DIR}/$2" -t "$1" "$REPO_ROOT" \
        > "${LOGDIR}/build-$3.log" 2>&1
}

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
        -e "MAX_ROUNDS=${MAX_ROUNDS}" \
        -e "OUTPUT_TOKENS=${OUTPUT_TOKENS}" \
        -e "MAX_MODEL_LEN=${MAX_MODEL_LEN}" \
        -e "MAX_NUM_SEQS=${MAX_NUM_SEQS}" \
        -e "GPU_MEM_UTIL=${GPU_MEM_UTIL}" \
        -e "HF_HUB_OFFLINE=0" \
        -v "${HF_CACHE}:/root/.cache/huggingface:z" \
        "${extra[@]}" \
        "$image" 2>&1 | tee "$f"
    local rc="${PIPESTATUS[0]}"
    finish_variant "$variant" "$rc" "$f"
}

# ══ NoOffload ═════════════════════════════════════════════════════════════════
if want nooffload; then
    if ! img_exists "$IMG_NOOFFLOAD"; then
        if [[ "$DO_BUILD" -eq 1 ]] && build_simple "$IMG_NOOFFLOAD" Dockerfile.nooffload nooffload; then
            run_container_bench "NoOffload" "$IMG_NOOFFLOAD"
        else
            reason="image ${IMG_NOOFFLOAD} missing (pass --build)"
            [[ "$DO_BUILD" -eq 1 ]] && reason="image ${IMG_NOOFFLOAD} build failed (see build-nooffload.log)"
            record "NoOffload" "SKIPPED" "" "" "" "" "" "$reason" ""
            warn "NoOffload SKIPPED: $reason"
        fi
    else
        run_container_bench "NoOffload" "$IMG_NOOFFLOAD"
    fi
fi

# ══ Certus-SPDK ═══════════════════════════════════════════════════════════════
# Runs BEFORE CPUOffload and SharedStorage so it consumes the boot-reserved 1G
# hugepage pool while it is still intact (no runtime realloc, no reboot). The pool
# is released right after (free_1g_hugepages) so the host-RAM backends get it back.
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
                "${BUILD_ARGS[@]}" \
                -f "${REPO_ROOT}/certus-grpc-connector/Dockerfile" \
                -t "${IMG_GRPC#localhost/}" "$REPO_ROOT" \
                > "${LOGDIR}/build-grpc.log" 2>&1 \
                || cs_skip="gRPC image build failed (see build-grpc.log)"
        else
            cs_skip="image ${IMG_GRPC} missing (pass --build)"
        fi
    fi

    # Flip the shared group to vfio-pci + 1G hugepages for SPDK (runtime; no reboot).
    # Certus-SPDK runs BEFORE SharedStorage, so the boot-reserved 1G pool is still
    # intact here and allocate_hugepages_node is normally a no-op (pool already at
    # target). Then verify the pool actually reached target — a reboot is warranted
    # ONLY if it fell short (e.g. the pages were freed/fragmented earlier this boot).
    # configure-bench.sh has already set the boot param, so the fix is a single
    # reboot + re-run; here we degrade to a clean SKIP.
    if [[ -z "$cs_skip" ]]; then
        if ! reconfigure certus; then
            cs_skip="host reconfigure -> certus failed (see reconfigure-certus.log)"
        else
            hp_path="/sys/devices/system/node/node${HUGEPAGES_1G_NODE}/hugepages/hugepages-1048576kB/nr_hugepages"
            hp_now="$(cat "$hp_path" 2>/dev/null || echo 0)"
            if [[ "${hp_now:-0}" -lt "$HUGEPAGES_1G_TARGET" ]]; then
                cs_skip="1G hugepages under-allocated (got ${hp_now}/${HUGEPAGES_1G_TARGET}); boot param set by configure-bench.sh — reboot once and re-run"
            fi
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
            MAX_ROUNDS="$MAX_ROUNDS" \
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

# ══ Free 1G hugepages ═════════════════════════════════════════════════════════
# Certus-SPDK is the only backend that needs the boot-reserved 1G pool. Now that it
# has run (and its server is stopped), release those ~16 GiB back to normal RAM so
# the host-RAM backends below aren't starved under mem=32G. Runtime, no reboot.
if want cpuoffload || want sharedstorage; then
    free_1g_hugepages
fi

# ══ CPUOffload ════════════════════════════════════════════════════════════════
if want cpuoffload; then
    if ! img_exists "$IMG_CPU"; then
        if [[ "$DO_BUILD" -eq 1 ]] && build_simple "$IMG_CPU" Dockerfile.cpu-offload cpu-offload; then
            run_container_bench "CPUOffload" "$IMG_CPU" -e "CPU_BYTES=${CPU_BYTES}"
        else
            reason="image ${IMG_CPU} missing (pass --build)"
            [[ "$DO_BUILD" -eq 1 ]] && reason="image ${IMG_CPU} build failed (see build-cpu-offload.log)"
            record "CPUOffload" "SKIPPED" "" "" "" "" "" "$reason" ""
            warn "CPUOffload SKIPPED: $reason"
        fi
    else
        run_container_bench "CPUOffload" "$IMG_CPU" -e "CPU_BYTES=${CPU_BYTES}"
    fi
fi

# ══ SharedStorage ═════════════════════════════════════════════════════════════
# Runs AFTER Certus-SPDK (1G pool already released by free_1g_hugepages above): the
# reconfigure rebinds the shared group from vfio-pci back to kernel nvme + RAID0/XFS.
if want sharedstorage; then
    ss_skip=""
    # Bring the shared group up as kernel nvme + RAID0/XFS at $SHARED_FS.
    if ! reconfigure sharedstorage; then
        ss_skip="host reconfigure -> sharedstorage failed (see reconfigure-sharedstorage.log)"
    elif [[ ! -d "$SHARED_FS" ]]; then
        ss_skip="'$SHARED_FS' not present after reconfigure (see reconfigure-sharedstorage.log)"
    fi
    if [[ -z "$ss_skip" ]] && ! img_exists "$IMG_SHARED"; then
        if [[ "$DO_BUILD" -eq 1 ]]; then
            if [[ -f "${FS_BACKEND_DIR}/Dockerfile.wheel" ]]; then
                log "building ${IMG_SHARED} (FS_BACKEND_DIR=${FS_BACKEND_DIR})"
                # When a vLLM version is pinned, match the compiled wheel's torch
                # ABI to that base image by probing its torch/CUDA.
                declare -a ss_env=(FS_BACKEND_DIR="$FS_BACKEND_DIR" RUNTIME_IMG="$IMG_SHARED")
                if [[ -n "$VLLM_VERSION" ]]; then
                    ss_env+=(VLLM_VERSION="$VLLM_VERSION")
                    log "sharedstorage: probing torch in vllm/vllm-openai:v${VLLM_VERSION}"
                    # NB: the vllm-openai base image has an ENTRYPOINT that wraps
                    # args (`vllm ...`), so probe with --entrypoint python3 or the
                    # command never runs and the probe comes back empty.
                    probe="$(command podman run --rm --entrypoint python3 "docker.io/vllm/vllm-openai:v${VLLM_VERSION}" -c \
                        'import torch;print(torch.__version__.split("+")[0]);print((torch.version.cuda or "").replace(".",""))' 2>/dev/null)"
                    tv="$(echo "$probe" | sed -n 1p)"; cd_digits="$(echo "$probe" | sed -n 2p)"
                    if [[ -n "$tv" && -n "$cd_digits" ]]; then
                        log "sharedstorage: base torch=${tv} cuda=cu${cd_digits}"
                        ss_env+=(TORCH_VERSION="$tv" TORCH_CUDA_INDEX="cu${cd_digits}")
                    else
                        warn "sharedstorage: torch probe failed — using build-sharedstorage.sh defaults (may mismatch ABI; set TORCH_* / CUDA_BASE_TAG)"
                    fi
                fi
                ( cd "$SCRIPT_DIR" && env "${ss_env[@]}" bash build-sharedstorage.sh ) \
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
            -v "${SHARED_FS}:/mnt/fs-backend-bench:z" \
            -e "DRAM=${DRAM}" \
            -e "DISK_DEV=${dev}" \
            -e "SKIP_PREFLIGHT=1"
    fi
fi

# ── Emit results.json ─────────────────────────────────────────────────────────
json="${LOGDIR}/results.json"
{
    echo "{"
    echo "  \"vllm_version\": $([[ -n "$VLLM_VERSION" ]] && echo "\"${VLLM_VERSION}\"" || echo null),"
    echo "  \"model\": \"${MODEL}\","
    echo "  \"num_convs\": ${NUM_CONVS},"
    echo "  \"max_rounds\": ${MAX_ROUNDS},"
    echo "  \"output_tokens\": ${OUTPUT_TOKENS},"
    echo "  \"logdir\": \"${LOGDIR}\","
    echo "  \"variants\": ["
    n=${#R_VARIANT[@]}
    for i in "${!R_VARIANT[@]}"; do
        printf '    %s' "$(variant_json \
            "${R_VARIANT[$i]}" "${R_STATUS[$i]}" "${R_WALL[$i]}" "${R_ROUNDS[$i]}" \
            "${R_GENS[$i]}" "${R_TPS[$i]}" "${R_NATIVE[$i]}" "${R_REASON[$i]}" "${R_LOG[$i]}")"
        [[ $((i+1)) -lt $n ]] && echo "," || echo ""
    done
    echo "  ]"
    echo "}"
} > "$json"

# ── Human table ───────────────────────────────────────────────────────────────
echo ""
echo "=============================== KV-Offload Profile ==============================="
echo "model=${MODEL}  num_convs=${NUM_CONVS}  output_tokens=${OUTPUT_TOKENS}${VLLM_VERSION:+  vllm=${VLLM_VERSION}}"
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
echo "results.json    -> ${json}"
echo "per-variant     -> ${LOGDIR}/result-<variant>.json"
