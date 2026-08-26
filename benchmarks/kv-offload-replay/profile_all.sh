#!/usr/bin/env bash
# profile_all.sh — run the KV-offload benchmark variants against the same
# 12-turn ShareGPT replay workload and emit a side-by-side throughput table.
#
# Variants (run in this order):
#   NoOffload      GPU-only baseline                 (image certus-offload-bench, OFFLOAD_MODE=none)
#   Certus-SPDK    shmq client + certus-server-yaml  (image certus-shmq-bench + host server)
#   CPUOffload     vLLM OffloadingConnector -> host RAM (image certus-offload-bench, default mode)
#   SharedStorage  llmd_fs_backend on RAID0/XFS      (image certus-sharedstorage-bench)
#                  vLLM <= 0.23 path (native tiering not yet available)
#   Tiered-CPU-FS  vLLM TieringOffloadingManager: CPU primary + FS secondary
#                  vLLM >= 0.23 path (same certus-offload-bench + SECONDARY_TIER=fs; FS tier on RAID0/XFS)
#
# Certus-SPDK runs first (of the storage backends) on purpose: it consumes the
# boot-reserved 1G hugepage pool while it is still intact (no runtime realloc, no
# reboot). Once it has run, the pool is released back to normal RAM (see
# free_1g_hugepages) so the host-RAM backends (CPUOffload) and the RAID0/XFS
# page cache (SharedStorage, Tiered-CPU-FS) get that ~16 GiB back under mem=32G.
#
# Each variant is preflighted independently: ready ones run, the rest are marked
# SKIPPED with a reason. An existing bench image is reused as-is; a missing one
# is SKIPPED unless --rebuild is passed, which forces a fresh build (even when the
# image already exists — use it to bake in updated drivers). The shared NVMe
# group (--device-pci) is reconfigured in-run between the
# FS backends (kernel nvme + RAID0/XFS) and Certus-SPDK (vfio-pci + 1G hugepages)
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
#                  --model-fs /mnt/fs-backend-bench --rebuild

set -uo pipefail   # NOT -e: per-variant failures are handled, not fatal.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# ── Defaults ──────────────────────────────────────────────────────────────────
MODEL="NousResearch/Meta-Llama-3-8B"
MODEL_FS="/mnt/certus1"
declare -a DEVICE_PCI=()
NUM_CONVS=""           # empty = default by turn config (450 only for 12/12, whole corpus otherwise); --num-convs overrides
MAX_ROUNDS=0           # 0 = replay all turns; N caps every backend at N rounds/turns
OUTPUT_TOKENS=150
MAX_MODEL_LEN=8192
MAX_NUM_SEQS=64
GPU_MEM_UTIL=0.90
GPU="all"
SHM_PATH="${SHM_PATH:-/dev/shm/certus-shmq}"   # Certus-SPDK shmq mailbox (host <-> client)
CHANNELS="${CHANNELS:-32}"                      # server worker threads / max in-flight requests
# The DRAM tier is a single contiguous spdk_zmalloc from the reserved 1G hugepage
# pool (CERTUS_HUGEPAGES, default 16 — configure-bench.sh sizes the pool to leave
# node-0 RAM for vLLM). It cannot exceed the pool: DPDK's heap plus SPDK's
# per-controller DMA buffers reserve a few of the 1G pages (measured with 4
# controllers: 15G fails, 14G works). Default to pool minus DPDK's overhead, using
# the same margin configure-bench.sh declares (DPDK_HUGEPAGE_OVERHEAD_GIB=3) so the
# two scripts agree: pool 16 -> 13G tier.
MEM_TIER_SIZE="${MEM_TIER_SIZE:-$(( ${CERTUS_HUGEPAGES:-16} - 3 ))G}"
MEM_TIER_EXPLICIT=0     # set when --memory-tier-size is passed (wins over --total-mem)
# --total-mem derives MEM_TIER_SIZE = total − vLLM floor − DPDK/SPDK overhead.
# Values mirror configure-bench.sh so the two scripts agree.
TOTAL_MEM_GIB=""        # set by --total-mem <GiB>; empty = derivation off
VLLM_MIN_RAM_GIB="${VLLM_MIN_RAM_GIB:-16}"           # RAM floor reserved for vLLM
DPDK_HUGEPAGE_OVERHEAD_GIB="${DPDK_HUGEPAGE_OVERHEAD_GIB:-3}"  # DPDK heap + SPDK DMA
DPDK_MEMSEG_LIST_GIB="${DPDK_MEMSEG_LIST_GIB:-64}"   # DPDK single-alloc ceiling (pool cap)
EVICT_THRESH="0.6"
CPU_BYTES=$((16 * (1 << 30)))
DRAM=$((32 * (1 << 30)))
SLAB_SIZE_BYTES=2097152
TENSOR_PARALLEL_SIZE=1
# enforce_eager: run vLLM in eager mode (no CUDA-graph capture, no torch.compile)
# on EVERY backend. Default 0 (graphs on) so all variants match vLLM's own
# default and stay apples-to-apples; --enforce-eager flips it to 1 for the whole
# run. Plumbed to each driver as the ENFORCE_EAGER env var.
ENFORCE_EAGER=0
# Workload execution model, forwarded to every driver as the WORKLOAD_MODE env
# var. "batched" (default) is the synchronous per-round generate loop; "async"
# runs one vLLM coroutine per conversation (V1 AsyncLLM). --async flips it for
# the whole run; --workload-mode <mode> sets it explicitly.
WORKLOAD_MODE=batched
# Metrics are captured on every run: all five drivers default stats ON, so the
# vllm: prometheus counters (+ the KV-offload connector stats) and, on the
# Certus-SPDK/shmq path, the per-round SSD device I/O are always recorded. No
# flag or env is needed to turn them on, and the orchestrator no longer forwards
# anything to force them off.
# Named dataset workload, forwarded to every driver as WORKLOAD_NAME (see
# run_multiturn_common.resolve_workload). Empty = each driver's baked default
# (the 450x12 dataset). "sharegpt" selects the ShareGPT multi-turn workload by
# human-turn count via SHAREGPT_MIN_TURNS/SHAREGPT_MAX_TURNS. Exactly two configs
# are accepted: 12/12 (default) = the 450-conv, 12-turn set every image bakes as
# its DATASET_PATH (so at 12/12 the container variants are a no-op); 2/2 = the
# FULL 94,145-conv corpus (data/sharegpt/*.json), which is mounted in and capped
# by --num-convs. (2 is the loader's own >=2-turn floor; 1 is accepted as a
# legacy alias for 2.) Any other pair errors; use an explicit DATASET_PATH
# instead. Passing --min-turns/--max-turns implies --workload sharegpt (see below).
WORKLOAD_NAME=""
SHAREGPT_MIN_TURNS=""  # min human turns; 2 = full corpus (1 = alias), 12 = 450x12 subset. empty = default (12)
SHAREGPT_MAX_TURNS=""  # max human turns; empty = mirrors --min-turns (so --min-turns alone works)
# long-doc-qa (--workload long-doc-qa) shape knobs. Empty = the workload's baked
# defaults (5000 tok / 8 turns / 1000 docs); set via the environment to tune. All
# forwarded to every backend; only read when WORKLOAD_NAME=long-doc-qa. Declared
# here (with :- fallbacks) so `set -u` doesn't trip on an unset knob.
LONGDOC_DOC_TOKENS="${LONGDOC_DOC_TOKENS:-}"
LONGDOC_QUESTIONS="${LONGDOC_QUESTIONS:-}"
LONGDOC_NUM_DOCS="${LONGDOC_NUM_DOCS:-}"
LONGDOC_SEED="${LONGDOC_SEED:-}"
SERVER_WAIT=180        # seconds to wait for the Certus-SPDK server mailbox
DO_REBUILD=0           # --rebuild: force a fresh build of each bench image even if it exists
VLLM_VERSION="0.26.0"  # pin the vLLM base-image version for ALL backends (override with --vllm-version)
ONLY=""
SKIP=""
LOGDIR=""

# Image tags. Env-overridable (a caller can point this at externally-built
# images). With --vllm-version set, an untagged name here gets a :vllm<ver> tag
# appended below so multiple versions coexist.
# NoOffload, CPUOffload and Tiered-CPU-FS now share ONE image built from
# Dockerfile.offload (run_multiturn_offloading.py drives all three; the backend is
# picked per-run by OFFLOAD_MODE / SECONDARY_TIER). IMG_NOOFFLOAD / IMG_CPU are
# kept as override knobs but default to the same unified image.
IMG_OFFLOAD="${IMG_OFFLOAD:-certus-offload-bench}"
IMG_NOOFFLOAD="${IMG_NOOFFLOAD:-$IMG_OFFLOAD}"
IMG_CPU="${IMG_CPU:-$IMG_OFFLOAD}"
IMG_SHARED="${IMG_SHARED:-certus-sharedstorage-bench}"
IMG_SHMQ="${IMG_SHMQ:-localhost/certus-shmq-bench}"

# Host copy of the replay dataset, used only for the preflight existence warn
# (container runs bake their own copy). It lives in data/ but older layouts kept
# it beside this script — accept either.
DATASET_HOST="${SCRIPT_DIR}/../../data/sharegpt_12turn_450.json"
[[ -f "$DATASET_HOST" ]] || DATASET_HOST="${SCRIPT_DIR}/sharegpt_12turn_450.json"
SERVER_BIN="${REPO_ROOT}/target/release/certus-server-yaml"
# llmd_fs_backend repo (for --rebuild of the SharedStorage image). Empty = auto:
# resolved after --model-fs is parsed, preferring <model-fs>/llm-d-kv-cache/...
# (where it lives on this host) with a $HOME fallback. Override via env.
FS_BACKEND_DIR="${FS_BACKEND_DIR:-}"

usage() {
    cat <<'EOF'
profile_all.sh — run the KV-offload benchmark variants and print one table.

Flags (all optional; defaults shown):
  --device-pci <DDDD:BB:DD.F>   NVMe PCIe addr of the SHARED drive group (repeatable).
                                Used for BOTH the RAID0/XFS FS backends (SharedStorage,
                                Tiered-CPU-FS) and Certus-SPDK (vfio/SPDK): the host is reconfigured onto this
                                group between the two phases via tools/configure-bench.sh, so
                                the storage backends compare on identical devices.
                                [default 0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0]
  --model-fs <dir>              Filesystem for HF cache + shmq podman store. [/mnt/certus1]
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
  --memory-tier-size <sz>      Certus-SPDK server DRAM pool (e.g. 32G). Wins over
                               --total-mem if both are given. [CERTUS_HUGEPAGES-3 G]
  --total-mem <GiB>            Derive the Certus-SPDK DRAM tier from total system
                               memory: tier = GiB − vLLM floor (${VLLM_MIN_RAM_GIB}G) − DPDK/SPDK
                               overhead (${DPDK_HUGEPAGE_OVERHEAD_GIB}G), clamped to the reserved 1G pool
                               (CERTUS_HUGEPAGES − ${DPDK_HUGEPAGE_OVERHEAD_GIB}G). Ignored if --memory-tier-size set.
  --evict-threshold <f>        Certus-SPDK DRAM->SSD demotion threshold. [0.6]
  --enforce-eager              Run vLLM in eager mode on ALL backends (no CUDA-graph
                               capture / torch.compile). Default off (graphs on),
                               matching vLLM's default; set this to keep the
                               variants comparable when profiling per-op transfers.
  --async                      Run every backend in async mode (one vLLM coroutine
                               per conversation, V1 AsyncLLM) instead of the default
                               synchronous batched-round loop. Shorthand for
                               --workload-mode async.
  --workload-mode <mode>       Execution model for all backends: batched (default)
                               or async. Forwarded as the WORKLOAD_MODE env var.
  --workload <name>            Named dataset workload for all backends, forwarded as
                               WORKLOAD_NAME. Empty (default) = the baked 450x12
                               dataset. "sharegpt" = the ShareGPT multi-turn workload
                               selected by human-turn count (--min-turns/--max-turns).
                               12/12 (default) = that baked 450x12 set; --min-turns 2
                               = the FULL 94,145-conv corpus (data/sharegpt/*.json,
                               mounted in, capped by --num-convs). Any other pair errors;
                               use an explicit DATASET_PATH instead.
                               "long-doc-qa" = synthetic long-document QA (a large
                               per-doc-unique prefix + follow-ups; KV-cache stress).
                               Shape via env LONGDOC_DOC_TOKENS / LONGDOC_QUESTIONS /
                               LONGDOC_NUM_DOCS / LONGDOC_SEED (defaults 4000/8/1000);
                               NUM_CONVS defaults to LONGDOC_NUM_DOCS. Big docs need a
                               matching --max-model-len.
  --min-turns <n>              Min human turns for the sharegpt workload; 2 selects the
                               full corpus (the loader's >=2-turn floor; 1 = legacy
                               alias), 12 the 450x12 subset. Implies --workload
                               sharegpt. Forwarded as SHAREGPT_MIN_TURNS. [default 12]
  --max-turns <n>              Max human turns; must match --min-turns (2 or 12). Implies
                               --workload sharegpt; empty mirrors --min-turns.
                               Forwarded as SHAREGPT_MAX_TURNS. [default = --min-turns]
  --cpu-bytes <n>              CPU tier size in bytes — CPUOffload tier, and the
                               Tiered-CPU-FS PRIMARY tier (overflow spills to the FS tier). [16Gi]
  --dram <n>                   SharedStorage DRAM budget (DRAM env). [32Gi]
  --rebuild                    Force a fresh build of each bench image before its
                               run, EVEN IF the image already exists (this is how
                               you bake in updated drivers — a stale image is
                               otherwise reused as-is). Without it, a missing image
                               is SKIPPED. All images build via their Dockerfiles;
                               Tiered-CPU-FS reuses the CPUOffload image;
                               SharedStorage builds certus-sharedstorage-bench
                               (needs FS_BACKEND_DIR).
  --vllm-version <x.y.z>       Pin the vLLM base-image version for ALL backends
                               (--build-arg VLLM_VERSION). Images are tagged
                               :vllm<x.y.z> so versions coexist. Implies the images
                               must be built at that version — pass --rebuild too (or
                               pre-build them). Tiered-CPU-FS needs the native
                               TieringOffloadingSpec (vLLM >= 0.23); use SharedStorage
                               on older. [default 0.26.0]
  --only a,b                   Run only these variants.
  --skip a,b                   Skip these variants.
                               Names: nooffload, cpuoffload, certus-spdk, sharedstorage, tiered-cpu-fs.
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
        --memory-tier-size) MEM_TIER_SIZE="$2"; MEM_TIER_EXPLICIT=1; shift 2;;
        --total-mem)        TOTAL_MEM_GIB="$2"; shift 2;;
        --evict-threshold)  EVICT_THRESH="$2"; shift 2;;
        --cpu-bytes)        CPU_BYTES="$2"; shift 2;;
        --dram)             DRAM="$2"; shift 2;;
        --rebuild)          DO_REBUILD=1; shift;;
        --vllm-version)     VLLM_VERSION="$2"; shift 2;;
        --enforce-eager)    ENFORCE_EAGER=1; shift;;
        --async)            WORKLOAD_MODE=async; shift;;
        --workload-mode)    WORKLOAD_MODE="$2"; shift 2;;
        --workload)         WORKLOAD_NAME="$2"; shift 2;;
        --min-turns)        SHAREGPT_MIN_TURNS="$2"; shift 2;;
        --max-turns)        SHAREGPT_MAX_TURNS="$2"; shift 2;;
        --only)             ONLY="$2"; shift 2;;
        --skip)             SKIP="$2"; shift 2;;
        --logdir)           LOGDIR="$2"; shift 2;;
        -h|--help)          usage; exit 0;;
        *) echo "error: unknown argument '$1'" >&2; usage >&2; exit 2;;
    esac
done

# --min-turns/--max-turns only mean anything for the sharegpt workload, so
# supplying either without --workload implies it. Without this, turn flags alone
# leave WORKLOAD_NAME empty, workload_container_args() adds nothing, and every
# container falls back to its baked 450x12 DATASET_PATH — the "I asked for the
# full corpus and still got 450 convs" trap.
if [[ -z "$WORKLOAD_NAME" && ( -n "$SHAREGPT_MIN_TURNS" || -n "$SHAREGPT_MAX_TURNS" ) ]]; then
    WORKLOAD_NAME="sharegpt"
fi

# Only two sharegpt turn configs are prepared: 12/12 (450x12 subset) and 2/2
# (full corpus; the loader's own >=2-turn floor, so 1 is accepted as a legacy
# alias for 2). Reject anything else here, so the corpus bind-mount below (keyed
# on min-turns 1|2) can't pair with a bogus max and force an unvalidated
# DATASET_PATH. max-turns mirrors min-turns when unset, so --min-turns 2|12 alone
# is valid.
if [[ "$WORKLOAD_NAME" == "sharegpt" ]]; then
    _mn="${SHAREGPT_MIN_TURNS:-12}"; _mx="${SHAREGPT_MAX_TURNS:-$_mn}"
    if ! { [[ "$_mn" == "12" && "$_mx" == "12" ]] || \
           { [[ "$_mn" == "1" || "$_mn" == "2" ]] && [[ "$_mx" == "$_mn" ]]; }; }; then
        echo "error: --workload sharegpt accepts only 12/12 (the 450-conv subset)" >&2
        echo "       or 2/2 (the full 94,145-conv corpus; 1 also accepted); got min=${_mn} max=${_mx}." >&2
        echo "       Use an explicit DATASET_PATH for other turn counts." >&2
        exit 2
    fi
fi

# Default the conversation count from the turn config (mirrors
# run_multiturn_common._sharegpt_num_convs): 450 ONLY for the exactly-12/12
# subset, the whole corpus for every other turn config. Without this the
# hardcoded 450 default was forwarded as NUM_CONVS and — being the final override
# in resolve_workload — masked the corpus default, so --min-turns 2 still ran 450
# convs. An explicit --num-convs (NUM_CONVS non-empty) always wins.
if [[ -z "$NUM_CONVS" ]]; then
    if [[ "$WORKLOAD_NAME" == "long-doc-qa" ]]; then
        # Draw the whole generated corpus; the workload's own default is
        # LONGDOC_NUM_DOCS (also 1000), and load_convs caps at NUM_CONVS. Kept as
        # a literal here (not read from the workload) so the sharegpt-shaped
        # defaulting below stays untouched for that workload.
        NUM_CONVS="${LONGDOC_NUM_DOCS:-1000}"
    elif [[ "${SHAREGPT_MIN_TURNS:-12}" == "12" && "${SHAREGPT_MAX_TURNS:-${SHAREGPT_MIN_TURNS:-12}}" == "12" ]]; then
        NUM_CONVS=450     # exactly-12/12 subset
    else
        NUM_CONVS=94145   # everything else -> whole corpus (= _SHAREGPT_CORPUS_CONVS; load_convs caps here)
    fi
fi

# Reject unknown --only/--skip tokens up front. want() does substring-on-comma
# matching, so a typo (e.g. --only cpu, --only certus) silently selects nothing and
# that variant just never runs — fail loudly instead.
VALID_VARIANTS="nooffload cpuoffload certus-spdk sharedstorage tiered-cpu-fs"
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
    [[ "$IMG_SHMQ"      != *:* ]] && IMG_SHMQ+="$_tag"
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
# Reuse an EXISTING md array only if one is mounted at the exact SharedStorage
# mountpoint. Note: NO --target here. `findmnt --target <dir>` resolves the mount
# that *contains* <dir>, walking UP to the parent when <dir> itself isn't a mount —
# so when $SHARED_FS is unmounted (e.g. the drives are on vfio for the certus phase)
# it returned the ROOT device /dev/mapper/rhel-root, which then became mdadm's array
# node (MD_DEVICE=/dev/rhel-root) — creating a RAID over the live root device node.
# Plain `findmnt <mountpoint>` matches only an exact mountpoint (empty otherwise).
DISK_DEV="$(findmnt -no SOURCE "$SHARED_FS" 2>/dev/null | xargs -r basename)"
# Accept only a real md array; otherwise pick the lowest free /dev/mdN
# (md0 is the persistent model-fs array -> md1, and so on).
if [[ "$DISK_DEV" != md* ]]; then
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

# ── Derive the Certus DRAM tier from total system memory (--total-mem) ─────────
# tier = total − vLLM RAM floor − DPDK/SPDK overhead, clamped to the reserved 1G
# hugepage pool (the tier is a single spdk_zmalloc from that pool, so it can't
# exceed CERTUS_HUGEPAGES − DPDK_HUGEPAGE_OVERHEAD_GIB regardless of total RAM).
# An explicit --memory-tier-size always wins.
if [[ -n "$TOTAL_MEM_GIB" ]]; then
    if ! [[ "$TOTAL_MEM_GIB" =~ ^[0-9]+$ ]]; then
        echo "error: --total-mem expects an integer GiB (got '${TOTAL_MEM_GIB}')" >&2; exit 2
    fi
    # Reject values larger than the RAM physically present. MemTotal runs a bit
    # below nominal (firmware/kernel reserve), so round it up to the next 4 GiB
    # to recover the installed size before comparing — 29.6G MemTotal -> 32G.
    _memtotal_kib=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null)
    if [[ -n "$_memtotal_kib" ]]; then
        _phys_gib=$(( _memtotal_kib / 1048576 ))
        _phys_installed=$(( ( (_phys_gib + 3) / 4 ) * 4 ))   # round up to 4 GiB
        if [[ $TOTAL_MEM_GIB -gt $_phys_installed ]]; then
            echo "error: --total-mem ${TOTAL_MEM_GIB}G exceeds physical RAM" \
                 "(MemTotal ${_phys_gib}G, ~${_phys_installed}G installed)" >&2
            exit 2
        fi
    fi
    if [[ "$MEM_TIER_EXPLICIT" -eq 1 ]]; then
        warn "--total-mem ignored: --memory-tier-size ${MEM_TIER_SIZE} was given explicitly"
    else
        _overhead=$(( VLLM_MIN_RAM_GIB + DPDK_HUGEPAGE_OVERHEAD_GIB ))
        _derived=$(( TOTAL_MEM_GIB - _overhead ))
        if [[ $_derived -le 0 ]]; then
            echo "error: --total-mem ${TOTAL_MEM_GIB}G leaves no room for a DRAM tier after" \
                 "vLLM ${VLLM_MIN_RAM_GIB}G + DPDK/SPDK ${DPDK_HUGEPAGE_OVERHEAD_GIB}G = ${_overhead}G" >&2
            exit 2
        fi
        # Size the hugepage pool the same way configure-bench.sh does — reserve
        # everything above the vLLM floor, capped just under the DPDK single-alloc
        # ceiling — unless CERTUS_HUGEPAGES pins it explicitly. Deriving from the
        # same total means the pool exactly fits the tier (no clamp); an explicit,
        # smaller pool still clamps the tier down to what actually fits.
        if [[ -n "${CERTUS_HUGEPAGES:-}" ]]; then
            _pool=$CERTUS_HUGEPAGES
        else
            _pool=$(( TOTAL_MEM_GIB - VLLM_MIN_RAM_GIB ))
            (( _pool > DPDK_MEMSEG_LIST_GIB - 1 )) && _pool=$(( DPDK_MEMSEG_LIST_GIB - 1 ))
            (( _pool < 0 )) && _pool=0
            # Propagate the derived pool so it actually gets reserved. Without this
            # --total-mem only resized the tier: HUGEPAGES_1G_TARGET, the pre-start
            # preflight, and the reconfigure hand-off (all keyed on CERTUS_HUGEPAGES)
            # stayed at the 16-page default, so a 45G tier was aimed at a 16-page
            # pool → spdk_zmalloc fails every time.
            CERTUS_HUGEPAGES=$_pool
        fi
        # HUGEPAGES_1G_TARGET was captured from CERTUS_HUGEPAGES above (before this
        # block ran); re-sync it so the pre-start preflight checks the derived pool.
        HUGEPAGES_1G_TARGET="$CERTUS_HUGEPAGES"
        _ceiling=$(( _pool - DPDK_HUGEPAGE_OVERHEAD_GIB ))
        if [[ $_derived -gt $_ceiling ]]; then
            warn "--total-mem implies a ${_derived}G tier, but the reserved 1G pool caps it at" \
                 "${_ceiling}G (CERTUS_HUGEPAGES=${_pool} − ${DPDK_HUGEPAGE_OVERHEAD_GIB}G DPDK). Clamping;" \
                 "raise CERTUS_HUGEPAGES and reboot for a larger tier."
            _derived=$_ceiling
        fi
        MEM_TIER_SIZE="${_derived}G"
        log "Certus DRAM tier ${MEM_TIER_SIZE} derived from --total-mem ${TOTAL_MEM_GIB}G" \
            "(− vLLM ${VLLM_MIN_RAM_GIB}G − DPDK/SPDK ${DPDK_HUGEPAGE_OVERHEAD_GIB}G)"
    fi
fi

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
    gpu_mark end "$variant"
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
if want sharedstorage || want tiered-cpu-fs || want certus-spdk; then
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

# Reap stale bench containers (the earlier GPU-pin foot-gun). Match on IMAGE, not
# just NAME: a bench container started without --name gets a random name (e.g.
# "inspiring_swartz") that a name-only pattern misses, yet it still pins the GPU.
# rm -f stops the container, releasing its GPU memory. Runs BEFORE the GPU-free
# check below so that check only flags usage we did not just clear.
reap() {
    local names ids
    names="$(command podman ps -a --format '{{.ID}} {{.Names}} {{.Image}}' 2>/dev/null | grep -E 'certus-(nooffload|cpu-offload|sharedstorage|shmq)-bench' | awk '{print $1}')"
    if [[ -n "$names" ]]; then
        warn "reaping stale bench containers: $(echo "$names" | tr '\n' ' ')"
        echo "$names" | xargs -r command podman rm -f >/dev/null 2>&1
    fi
    # Same for the shmq store.
    ids="$(command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" ps -a --format '{{.ID}} {{.Names}} {{.Image}}' 2>/dev/null | grep -E 'certus-shmq-bench|shmq-bench' | awk '{print $1}')"
    [[ -n "$ids" ]] && echo "$ids" | xargs -r command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" rm -f >/dev/null 2>&1
}
reap

# GPU-free check — after reaping our own stale containers, so it only flags usage
# from a foreign process (which we must not kill). Informational: warns, does not
# abort — the benchmark may still fit, or the user may want to intervene.
if command -v nvidia-smi >/dev/null 2>&1; then
    used="$(nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits 2>/dev/null | sort -rn | head -1)"
    if [[ -n "$used" && "$used" -gt 1024 ]]; then
        warn "a GPU still has ${used} MiB in use after reaping bench containers — a foreign process may starve the benchmark"
    fi
else
    warn "nvidia-smi not found — cannot verify GPU availability"
fi

# ── Pin GPU clocks for stable cross-backend timing ──
# The A30/A100 auto-boost clock drifts with temperature and power draw across
# back-to-back runs, so generation throughput (and thus wall time) wanders even
# on byte-identical work — measured ~12% wall spread on repeat Certus-SPDK runs,
# larger than the backend differences we're trying to measure. Lock every GPU to
# its OWN max SM clock (queried per host, never hardcoded) and enable persistence
# mode so all four backends see the same clock. Applies host-wide (the bench uses
# GPU=all). Needs root; this is a stability knob, so warn-and-continue if we can't
# — and always reset to auto-boost on exit (see the EXIT trap) so the GPU isn't
# left pinned after the run.
GPU_CLOCK_LOCKED=0
lock_gpu_clocks() {
    command -v nvidia-smi >/dev/null 2>&1 || return 0
    local maxsm
    maxsm="$(nvidia-smi --query-gpu=clocks.max.sm --format=csv,noheader,nounits 2>/dev/null | sort -rn | head -1)"
    if [[ ! "$maxsm" =~ ^[0-9]+$ ]]; then
        warn "could not read GPU max SM clock — leaving clocks on auto-boost (runs may drift ~10%)"
        return 0
    fi
    # Acquire sudo non-fatally (the storage-backend preflight may not have run for
    # a GPU-only variant). Cached/NOPASSWD works headless; else prompt once here.
    if ! sudo -n true 2>/dev/null && ! sudo -v; then
        warn "no sudo — cannot pin GPU clocks; generation throughput may drift across backends"
        return 0
    fi
    if sudo -n nvidia-smi -pm 1 >/dev/null 2>&1 \
       && sudo -n nvidia-smi -lgc "${maxsm},${maxsm}" >/dev/null 2>&1; then
        GPU_CLOCK_LOCKED=1
        log "pinned GPU SM clock to ${maxsm} MHz (persistence on) — stable cross-backend timing"
    else
        warn "failed to pin GPU clocks (nvidia-smi -pm/-lgc) — continuing on auto-boost"
    fi
}
unlock_gpu_clocks() {
    [[ "${GPU_CLOCK_LOCKED:-0}" == 1 ]] || return 0
    log "resetting GPU clocks to default (auto-boost)"
    sudo -n nvidia-smi -rgc >/dev/null 2>&1 || warn "could not reset GPU clocks (sudo nvidia-smi -rgc)"
    GPU_CLOCK_LOCKED=0
}
lock_gpu_clocks

# ── GPU utilization sampler (time series over the whole run) ──
# A background poller snapshots per-GPU utilization/clock/mem/power every
# GPU_SAMPLE_SEC to gpu-timeline.csv; gpu_mark records the start/end epoch of each
# variant's window to gpu-markers.csv. gpu_report (end of run) slices the timeline
# by those windows into a per-variant table + an over-time sparkline. 2 s is fine
# grain for a ~20 min/variant run (~600 samples) without flooding the file.
GPU_SAMPLE_SEC="${GPU_SAMPLE_SEC:-2}"
GPU_SAMPLER_PID=""
start_gpu_sampler() {
    command -v nvidia-smi >/dev/null 2>&1 || return 0
    [[ -n "${LOGDIR:-}" && -d "${LOGDIR:-}" ]] || return 0
    local tl="${LOGDIR}/gpu-timeline.csv"
    echo "epoch_s,gpu_idx,util_gpu_pct,util_mem_pct,mem_used_mib,sm_clock_mhz,temp_c,power_w" > "$tl"
    (
        while true; do
            ts="$(date +%s)"
            nvidia-smi --query-gpu=index,utilization.gpu,utilization.memory,memory.used,clocks.sm,temperature.gpu,power.draw \
                --format=csv,noheader,nounits 2>/dev/null \
                | sed "s/^/${ts}, /; s/, /,/g" >> "$tl" || true
            sleep "$GPU_SAMPLE_SEC"
        done
    ) &
    GPU_SAMPLER_PID=$!
    log "sampling GPU telemetry every ${GPU_SAMPLE_SEC}s -> $(basename "$tl")"
}
stop_gpu_sampler() {
    [[ -n "${GPU_SAMPLER_PID:-}" ]] || return 0
    kill "$GPU_SAMPLER_PID" 2>/dev/null
    wait "$GPU_SAMPLER_PID" 2>/dev/null
    GPU_SAMPLER_PID=""
}
# Record a variant-window boundary (start|end) with a wall-clock epoch.
gpu_mark() {  # phase variant
    [[ -n "${LOGDIR:-}" && -d "${LOGDIR:-}" ]] || return 0
    local mk="${LOGDIR}/gpu-markers.csv"
    [[ -f "$mk" ]] || echo "epoch_s,phase,variant" > "$mk"
    echo "$(date +%s),$1,$2" >> "$mk"
}
# Slice the timeline by variant window and print a table + sparkline.
gpu_report() {
    local tl="${LOGDIR}/gpu-timeline.csv" mk="${LOGDIR}/gpu-markers.csv"
    [[ -f "$tl" ]] || return 0
    command -v python3 >/dev/null 2>&1 || { warn "python3 not found — skipping GPU report"; return 0; }
    python3 "${SCRIPT_DIR}/gpu_report.py" "$tl" "$mk" 2>/dev/null | tee "${LOGDIR}/gpu-summary.txt"
    echo "gpu timeline    -> ${tl}"
    echo "gpu summary     -> ${LOGDIR}/gpu-summary.txt"
}
start_gpu_sampler

# Reset clocks / stop the sampler on any exit from here on. Superseded below by a
# combined handler once the Certus-SPDK server exists, so an early exit here still
# unpins the GPU and reaps the sampler.
trap 'stop_gpu_sampler; unlock_gpu_clocks' EXIT

img_exists()      { command podman image exists "$1" >/dev/null 2>&1; }
img_exists_shmq() { command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" image exists "$1" >/dev/null 2>&1; }

# Build a self-contained image (default store) from one of our Dockerfiles,
# honoring any --vllm-version build-arg. Returns build rc.
build_simple() {  # image dockerfile logtag
    log "building $1 from $2 ${VLLM_VERSION:+(vLLM ${VLLM_VERSION})}"
    command podman build "${BUILD_ARGS[@]}" -f "${SCRIPT_DIR}/$2" -t "$1" "$REPO_ROOT" \
        > "${LOGDIR}/build-$3.log" 2>&1
}

# NoOffload, CPUOffload and Tiered all build the SAME Dockerfile.offload image, so
# memoize the build per resolved tag: the first variant to need it builds it, the
# rest reuse it (avoids 2-3 redundant `podman build` passes under --rebuild).
OFFLOAD_IMG_BUILT=""
build_offload() {  # image
    if [[ "$OFFLOAD_IMG_BUILT" == "$1" ]]; then
        log "reusing offload image $1 (built earlier this run)"
        return 0
    fi
    if build_simple "$1" Dockerfile.offload offload; then
        OFFLOAD_IMG_BUILT="$1"
        return 0
    fi
    return 1
}

# Named-workload env + mounts shared by every backend launcher. When --workload
# is set it forwards WORKLOAD_NAME plus any SHAREGPT_MIN_TURNS/MAX_TURNS.
#
# The 12/12 sharegpt set is baked into every image (as DATASET_PATH), so at
# 12/12 this just forwards env and is a harmless no-op. min-turns 2 selects the
# FULL corpus, which is NOT baked: the images differ in where their
# __file__-relative data dir resolves (offload/sharedstorage flatten the
# layout), so instead of trusting that path we bind-mount data/sharegpt
# read-only and point DATASET_PATH at the mount — DATASET_PATH always wins in
# resolve_workload, so this overrides the baked 450x12 default uniformly.
# Empty WORKLOAD_NAME => no extra args, so the baked default dataset is used.
workload_container_args() {  # -> prints podman args, one per line
    [[ -z "$WORKLOAD_NAME" ]] && return 0
    printf '%s\n' "-e" "WORKLOAD_NAME=${WORKLOAD_NAME}"
    [[ -n "$SHAREGPT_MIN_TURNS" ]] && printf '%s\n' "-e" "SHAREGPT_MIN_TURNS=${SHAREGPT_MIN_TURNS}"
    [[ -n "$SHAREGPT_MAX_TURNS" ]] && printf '%s\n' "-e" "SHAREGPT_MAX_TURNS=${SHAREGPT_MAX_TURNS}"
    if [[ "$WORKLOAD_NAME" == "sharegpt" && ( "$SHAREGPT_MIN_TURNS" == "2" || "$SHAREGPT_MIN_TURNS" == "1" ) ]]; then
        if [[ -d "${REPO_ROOT}/data/sharegpt" ]]; then
            printf '%s\n' \
                "-v" "${REPO_ROOT}/data/sharegpt:/workspace/data/sharegpt:ro,z" \
                "-e" "DATASET_PATH=/workspace/data/sharegpt"
        else
            warn "min-turns ${SHAREGPT_MIN_TURNS} needs the full corpus at ${REPO_ROOT}/data/sharegpt (data/sharegpt/*.json) — not found; run will fall back to the baked 450x12 set"
        fi
    elif [[ "$WORKLOAD_NAME" != "sharegpt" ]]; then
        # A self-generating workload (e.g. long-doc-qa builds its own dataset in
        # the container). Every image bakes DATASET_PATH=<450x12 sharegpt>, and an
        # explicit DATASET_PATH WINS over WORKLOAD_NAME in resolve_workload — so
        # unless we blank it, the named workload is silently ignored and the baked
        # 450-conv ShareGPT set runs instead. An empty value reads as unset there.
        printf '%s\n' "-e" "DATASET_PATH="
    fi
}

# Common container run for the three self-contained images (default podman store).
run_container_bench() {  # variant image extra-args...
    local variant="$1" image="$2"; shift 2
    local extra=("$@") f="${LOGDIR}/${variant}.log"
    local wl=(); mapfile -t wl < <(workload_container_args)
    log "starting ${variant} (image ${image})"
    gpu_mark start "$variant"
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
        -e "ENFORCE_EAGER=${ENFORCE_EAGER}" \
        -e "WORKLOAD_MODE=${WORKLOAD_MODE}" \
        -e "LONGDOC_DOC_TOKENS=${LONGDOC_DOC_TOKENS}" \
        -e "LONGDOC_QUESTIONS=${LONGDOC_QUESTIONS}" \
        -e "LONGDOC_NUM_DOCS=${LONGDOC_NUM_DOCS}" \
        -e "LONGDOC_SEED=${LONGDOC_SEED}" \
        -e "HF_HUB_OFFLINE=0" \
        -v "${HF_CACHE}:/root/.cache/huggingface:z" \
        "${wl[@]}" \
        "${extra[@]}" \
        "$image" 2>&1 | tee "$f"
    local rc="${PIPESTATUS[0]}"
    finish_variant "$variant" "$rc" "$f"
}

# ══ NoOffload ═════════════════════════════════════════════════════════════════
if want nooffload; then
    if [[ "$DO_REBUILD" -eq 1 ]]; then
        if build_offload "$IMG_NOOFFLOAD"; then
            run_container_bench "NoOffload" "$IMG_NOOFFLOAD" -e "OFFLOAD_MODE=none"
        else
            reason="image ${IMG_NOOFFLOAD} build failed (see build-offload.log)"
            record "NoOffload" "SKIPPED" "" "" "" "" "" "$reason" ""
            warn "NoOffload SKIPPED: $reason"
        fi
    elif img_exists "$IMG_NOOFFLOAD"; then
        run_container_bench "NoOffload" "$IMG_NOOFFLOAD" -e "OFFLOAD_MODE=none"
    else
        reason="image ${IMG_NOOFFLOAD} missing (pass --rebuild to build it)"
        record "NoOffload" "SKIPPED" "" "" "" "" "" "$reason" ""
        warn "NoOffload SKIPPED: $reason"
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
trap 'stop_server; stop_gpu_sampler; unlock_gpu_clocks' EXIT

if want certus-spdk; then
    cs_skip=""
    if [[ ${#DEVICE_PCI[@]} -eq 0 ]]; then
        cs_skip="no --device-pci given (Certus-SPDK server needs an NVMe device)"
    elif [[ ! -x "$SERVER_BIN" ]]; then
        cs_skip="server binary not built at ${SERVER_BIN} (cargo build --release -p certus-server-yaml --features rw-telemetry)"
    fi
    if [[ -z "$cs_skip" ]] && { [[ "$DO_REBUILD" -eq 1 ]] || ! img_exists_shmq "$IMG_SHMQ"; }; then
        if [[ "$DO_REBUILD" -eq 1 ]]; then
            log "building ${IMG_SHMQ} into ${PODMAN_STORE}"
            command podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" build \
                "${BUILD_ARGS[@]}" \
                -f "${REPO_ROOT}/certus-shmq-connector/Dockerfile" \
                -t "${IMG_SHMQ#localhost/}" "$REPO_ROOT" \
                > "${LOGDIR}/build-shmq.log" 2>&1 \
                || cs_skip="shmq image build failed (see build-shmq.log)"
        else
            cs_skip="image ${IMG_SHMQ} missing (pass --rebuild to build it)"
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
        # Pin the whole server to the NVMe/hugepage NUMA node (HUGEPAGES_1G_NODE,
        # normally node 0). The 1G pages and the NVMe group both live there; without
        # this the SPDK reactor can land on the other socket and try to allocate the
        # memory tier from that node's (empty) 1G pool — "set_mempolicy failed" +
        # spdk_zmalloc failure. GPU access is NUMA-agnostic, so binding host CPU and
        # memory to node 0 (everything but the GPU) is exactly what we want.
        numa_prefix=()
        if command -v numactl >/dev/null 2>&1; then
            numa_prefix=(numactl "--cpunodebind=${HUGEPAGES_1G_NODE}" "--membind=${HUGEPAGES_1G_NODE}")
        fi
        # Start from a clean mailbox so a stale file from a crashed prior run
        # can't fool the client preflight (the server recreates it anyway).
        rm -f "$SHM_PATH"
        log "starting Certus-SPDK server: ${dev_flags[*]} --memory-tier-size ${MEM_TIER_SIZE} shm=${SHM_PATH} channels=${CHANNELS} (numa node ${HUGEPAGES_1G_NODE})"
        "${numa_prefix[@]}" "$SERVER_BIN" "${dev_flags[@]}" \
            --memory-tier-size "$MEM_TIER_SIZE" \
            --memory-tier-eviction-threshold "$EVICT_THRESH" \
            --shm-path "$SHM_PATH" \
            --channels "$CHANNELS" \
            --format \
            > "${LOGDIR}/server.log" 2>&1 &
        SERVER_PID=$!

        # Readiness: no TCP port. The server builds the whole stack, then
        # publishes the mailbox at SHM_PATH last — so its presence means the
        # server is serving and the client container can attach via --ipc=host.
        up=0
        for _ in $(seq 1 "$SERVER_WAIT"); do
            if ! kill -0 "$SERVER_PID" 2>/dev/null; then break; fi
            if [[ -e "$SHM_PATH" ]]; then up=1; break; fi
            sleep 1
        done

        if [[ "$up" -ne 1 ]]; then
            record "Certus-SPDK" "SKIPPED" "" "" "" "" "" "server mailbox ${SHM_PATH} did not appear within ${SERVER_WAIT}s (see server.log)" "${LOGDIR}/server.log"
            warn "Certus-SPDK SKIPPED: server did not come up"
            stop_server
        else
            log "server serving, mailbox ${SHM_PATH} — launching shmq client"
            f="${LOGDIR}/certus-spdk.log"
            gpu_mark start "Certus-SPDK"
            # No CERTUS_SERVER: the shared /dev/shm mailbox at SHM_PATH is the
            # endpoint (run-bench.sh shares it into the container via --ipc=host).
            IMAGE="$IMG_SHMQ" \
            GPU="$GPU" \
            SHM_PATH="$SHM_PATH" \
            NUM_CONVS="$NUM_CONVS" \
            MAX_ROUNDS="$MAX_ROUNDS" \
            MODEL="$MODEL" \
            SLAB_SIZE_BYTES="$SLAB_SIZE_BYTES" \
            TENSOR_PARALLEL_SIZE="$TENSOR_PARALLEL_SIZE" \
            ENFORCE_EAGER="$ENFORCE_EAGER" \
            WORKLOAD_MODE="$WORKLOAD_MODE" \
            WORKLOAD_NAME="$WORKLOAD_NAME" \
            SHAREGPT_MIN_TURNS="$SHAREGPT_MIN_TURNS" \
            SHAREGPT_MAX_TURNS="$SHAREGPT_MAX_TURNS" \
            LONGDOC_DOC_TOKENS="$LONGDOC_DOC_TOKENS" \
            LONGDOC_QUESTIONS="$LONGDOC_QUESTIONS" \
            LONGDOC_NUM_DOCS="$LONGDOC_NUM_DOCS" \
            LONGDOC_SEED="$LONGDOC_SEED" \
            HF_CACHE="$HF_CACHE" \
            PODMAN_STORE="$PODMAN_STORE" \
            PODMAN_RUNROOT="$PODMAN_RUNROOT" \
                bash "${REPO_ROOT}/certus-shmq-connector/run-bench.sh" 2>&1 | tee "$f"
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
if want cpuoffload || want sharedstorage || want tiered-cpu-fs; then
    free_1g_hugepages
fi

# ══ CPUOffload ════════════════════════════════════════════════════════════════
if want cpuoffload; then
    if [[ "$DO_REBUILD" -eq 1 ]]; then
        if build_offload "$IMG_CPU"; then
            run_container_bench "CPUOffload" "$IMG_CPU" -e "CPU_BYTES=${CPU_BYTES}"
        else
            reason="image ${IMG_CPU} build failed (see build-offload.log)"
            record "CPUOffload" "SKIPPED" "" "" "" "" "" "$reason" ""
            warn "CPUOffload SKIPPED: $reason"
        fi
    elif img_exists "$IMG_CPU"; then
        run_container_bench "CPUOffload" "$IMG_CPU" -e "CPU_BYTES=${CPU_BYTES}"
    else
        reason="image ${IMG_CPU} missing (pass --rebuild to build it)"
        record "CPUOffload" "SKIPPED" "" "" "" "" "" "$reason" ""
        warn "CPUOffload SKIPPED: $reason"
    fi
fi

# ══ SharedStorage ═════════════════════════════════════════════════════════════
# vLLM <= 0.23 path: llmd_fs_backend on kernel nvme + RAID0/XFS. (For 0.23+ prefer
# the Tiered variant below, which uses vLLM's native TieringOffloadingManager.)
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
    if [[ -z "$ss_skip" ]] && { [[ "$DO_REBUILD" -eq 1 ]] || ! img_exists "$IMG_SHARED"; }; then
        if [[ "$DO_REBUILD" -eq 1 ]]; then
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
            ss_skip="image ${IMG_SHARED} missing (pass --rebuild with FS_BACKEND_DIR set)"
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

# ══ Tiered-CPU-FS (CPU primary + FS secondary) ═══════════════════════════════
# vLLM 0.23+ path: the native TieringOffloadingManager — CPU tier as primary, a
# filesystem tier as secondary. Reuses the CPUOffload image (same
# OffloadingConnector); the only difference is SECONDARY_TIER=fs, which selects
# TieringOffloadingSpec + the fs secondary written to FS_ROOT_DIR on the RAID0/XFS
# group. Runs in the same kernel-nvme/RAID0 phase as SharedStorage (after
# Certus-SPDK + free_1g_hugepages).
if want tiered-cpu-fs; then
    t_skip=""
    # Native TieringOffloadingSpec requires vLLM >= 0.23 (SharedStorage covers older).
    if [[ -n "$VLLM_VERSION" ]]; then
        _mm="${VLLM_VERSION%.*}"
        _min="$(printf '%s\n0.23\n' "$_mm" | sort -V | head -1)"
        [[ "$_min" == "$_mm" && "$_mm" != "0.23" ]] && \
            t_skip="Tiered-CPU-FS needs vLLM >= 0.23 (got ${VLLM_VERSION}); use SharedStorage for older"
    fi
    # Bring the shared group up as kernel nvme + RAID0/XFS at $SHARED_FS (the fs tier).
    if [[ -z "$t_skip" ]] && ! reconfigure sharedstorage; then
        t_skip="host reconfigure -> RAID0/XFS failed (see reconfigure-sharedstorage.log)"
    elif [[ -z "$t_skip" && ! -d "$SHARED_FS" ]]; then
        t_skip="'$SHARED_FS' not present after reconfigure (see reconfigure-sharedstorage.log)"
    fi
    # Reuses the CPUOffload image. Under --rebuild, force a fresh build the same
    # way nooffload/cpuoffload do: build_offload is memoized per tag, so if
    # CPUOffload already built it earlier this invocation this is a no-op reuse,
    # but --only tiered-cpu-fs (no CPUOffload) still gets a fresh build even when
    # a stale image already exists. Without --rebuild, run the existing image or
    # skip if missing.
    if [[ -z "$t_skip" ]]; then
        if [[ "$DO_REBUILD" -eq 1 ]]; then
            build_offload "$IMG_CPU" || \
                t_skip="image ${IMG_CPU} build failed (see build-offload.log)"
        elif ! img_exists "$IMG_CPU"; then
            t_skip="image ${IMG_CPU} missing (pass --rebuild to build it)"
        fi
    fi
    if [[ -n "$t_skip" ]]; then
        record "Tiered-CPU-FS" "SKIPPED" "" "" "" "" "" "$t_skip" ""
        warn "Tiered-CPU-FS SKIPPED: $t_skip"
    else
        # fs secondary-tier root on the RAID0/XFS group, mounted into the container.
        mkdir -p "${SHARED_FS}/kv-tier"
        # The TieringOffloadingSpec CPU primary tier is a SINGLE mmap in /dev/shm
        # (/dev/shm/vllm_offload_*.mmap), sized to cpu_bytes_to_use and faulted in
        # with MADV_POPULATE_WRITE. The container's default /dev/shm is 64 MiB, so
        # populating a 16 GiB region dies with "OSError: [Errno 14] Bad address".
        # (Plain CPUOffload uses a CUDA pinned buffer, not /dev/shm, so it is fine.)
        # Give /dev/shm the tier size + 2 GiB headroom.
        tier_shm=$((CPU_BYTES + 2 * (1 << 30)))
        # DISK_DEV lets the runner snapshot /sys/block/<md>/stat per round so the
        # fs secondary tier's real SSD read/write is recorded (like SharedStorage).
        run_container_bench "Tiered-CPU-FS" "$IMG_CPU" \
            --shm-size="${tier_shm}" \
            -v "${SHARED_FS}:/mnt/fs-tier:z" \
            -e "CPU_BYTES=${CPU_BYTES}" \
            -e "SECONDARY_TIER=fs" \
            -e "FS_ROOT_DIR=/mnt/fs-tier/kv-tier" \
            -e "DISK_DEV=${DISK_DEV}"
    fi
fi

# ── GPU utilization report (stop the sampler first so the CSV is complete) ─────
stop_gpu_sampler
gpu_report

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
