#!/bin/bash
# run-gqa-iobound-stream-sweep.sh — concurrency sweep over run-gqa-iobound-matrix.sh.
# For each stream count in STREAMS, runs the full 2x2 matrix {certus-shmq, cputier} x
# {no intervals, intervals} against Qwen2.5-7B-Instruct (GQA) with the SAME DRAM tier
# size on both backends (CPU_TIER_GB, default 128G), writes one report.html per stream
# count, then a cross-sweep summary (summary.md / summary.csv) in the sweep dir.
# FAMILY=mha sweeps run-mha-iobound-matrix.sh (Qwen1.5-7B-Chat, MHA) instead; see
# run-mha-iobound-stream-sweep.sh.
#
# WHY A PRIVATE SPDK BUILD: the Certus DRAM tier is ONE spdk_zmalloc, and stock DPDK caps
# a single memseg list at 32 GiB (RTE_MAX_MEM_MB_PER_LIST=32768) — a >=32G tier fails
# with "spdk_zmalloc failed (insufficient hugepages?)". This script builds (once, cached
# under SPDK_TIER_DIR) a private copy of deps/spdk with RTE_MAX_MEM_MB_PER_LIST/PER_TYPE
# raised to 266240 (same patch as deps/build_spdk.sh) plus a certus-server-yaml linked
# against it in a detached git worktree. The shared deps/spdk-build is never modified.
#
# HOST PREREQUISITES (checked up front; the script never uses sudo):
#   * 1 GiB hugepages on NUMA node CPUNODE >= CPU_TIER_GB + 4, e.g. for 128G:
#       echo 136 | sudo tee /sys/devices/system/node/node0/hugepages/hugepages-1048576kB/nr_hugepages
#   * /dev/hugepages writable by you          (sudo chmod 1777 /dev/hugepages)
#   * no stale /var/tmp/spdk_pci_lock_* owned by another user for DEVICE_PCIS
#   * NVMe DEVICE_PCIS bound to vfio-pci, GPU 0 idle, port 8000 free
#   * guidellm on PATH                        (uv tool install --python 3.12 guidellm)
#   * images in PODMAN_STORE: localhost/certus-shmq-connector:vllm0.30.0
#       (certus-shmq-connector/build_connector_container.sh 0.30.0) and
#       localhost/certus-offload:vllm0.30.0 (benchmarks/kv-offload-replay/build_cputier_container.sh 0.30.0)
#
#   ./run-gqa-iobound-stream-sweep.sh                       # 200..1000 streams, 128G, ~80 min per stream count
#   STREAMS="400 800" ./run-gqa-iobound-stream-sweep.sh     # subset
#   CPU_TIER_GB=64 ./run-gqa-iobound-stream-sweep.sh        # different (equal) tier size on both backends
#   MAX_SECONDS=120 STREAMS=200 ./run-gqa-iobound-stream-sweep.sh   # quick smoke
#   PREFLIGHT_ONLY=1 ./run-gqa-iobound-stream-sweep.sh      # check prerequisites (+ build the server) and exit
#   SERVER_BIN=/path/certus-server-yaml ./run-gqa-iobound-stream-sweep.sh  # skip the private build
#
# Other knobs pass straight through to the matrix/client scripts: ARMS, MAX_SECONDS,
# TURNS, PREFIX_COUNT, DELAY_SCALE. At the default PREFIX_COUNT=64 the ~28 GiB working
# set is DRAM-resident; raise PREFIX_COUNT (e.g. 512) to push reloads to SSD.
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

FAMILY="${FAMILY:-gqa}"   # gqa | mha — selects run-${FAMILY}-iobound-matrix.sh
STREAMS="${STREAMS:-200 400 600 800 1000}"
CPU_TIER_GB="${CPU_TIER_GB:-128}"
CPUNODE="${CPUNODE:-0}"
DEVICE_PCIS="${DEVICE_PCIS:-0000:61:00.0 0000:62:00.0 0000:63:00.0 0000:64:00.0}"
SWEEP_DIR="${SWEEP_DIR:-/mnt/certus1/vllm-serve-matrix/${FAMILY}-stream-sweep-$(date +%Y%m%d_%H%M%S)}"
SPDK_TIER_DIR="${SPDK_TIER_DIR:-/mnt/certus1/${USER}-spdk-tier128}"
SPDK_SRC="${SPDK_SRC:-$(readlink -f "${REPO_ROOT}/deps/spdk")}"
DPDK_TIER_CAP_MB="${DPDK_TIER_CAP_MB:-266240}"
GDRCOPY_LIB_PATH="${GDRCOPY_LIB_PATH:-${REPO_ROOT}/kernel/modules/gdrcopy/src}"
SERVER_BIN="${SERVER_BIN:-}"
SHMQ_IMAGE="localhost/certus-shmq-connector:vllm0.30.0"
CPUTIER_IMAGE="localhost/certus-offload:vllm0.30.0"
# Default to the caller's podman store: the /mnt/certus1 store the serve scripts assume
# is not usable by every user (crun error), so both images live in one store here.
PODMAN_STORE="${PODMAN_STORE:-$(podman info --format '{{.Store.GraphRoot}}' 2>/dev/null)}"
PODMAN_RUNROOT="${PODMAN_RUNROOT:-$(podman info --format '{{.Store.RunRoot}}' 2>/dev/null)}"

log(){ echo "[$(date +%H:%M:%S)] $*"; }
die(){ echo "error: $*" >&2; exit 1; }

# ── Private SPDK + certus-server-yaml (built once, reused) ─────────────────────────
build_server(){
  local spdk="${SPDK_TIER_DIR}/spdk" build="${SPDK_TIER_DIR}/spdk-build" wt="${SPDK_TIER_DIR}/certus"
  mkdir -p "$SPDK_TIER_DIR"
  if [[ ! -f "${build}/lib/librte_eal.a" ]]; then
    [[ -d "$SPDK_SRC" ]] || die "SPDK source not found at ${SPDK_SRC} (set SPDK_SRC)"
    log "building private SPDK (DPDK per-list cap ${DPDK_TIER_CAP_MB} MiB) -> ${build}  (~15 min)"
    rm -rf "$spdk"; cp -a "$SPDK_SRC" "$spdk" || die "copying ${SPDK_SRC} failed"
    for key in RTE_MAX_MEM_MB_PER_LIST RTE_MAX_MEM_MB_PER_TYPE; do
      sed -i "s/^#define ${key} [0-9]*/#define ${key} ${DPDK_TIER_CAP_MB}/" "${spdk}/dpdk/config/rte_config.h"
    done
    ( cd "$spdk" && ./configure --prefix="$build" --without-crypto && make -j"$(nproc)" && make install ) \
      >"${SPDK_TIER_DIR}/spdk-build.log" 2>&1 || die "SPDK build failed; see ${SPDK_TIER_DIR}/spdk-build.log"
  fi
  if [[ ! -d "$wt" ]]; then
    log "creating server worktree ${wt} at $(git -C "$REPO_ROOT" rev-parse --short HEAD)"
    git -C "$REPO_ROOT" worktree add --detach "$wt" HEAD >/dev/null || die "git worktree add failed"
    rm -f "${wt}/deps/spdk" "${wt}/deps/spdk-build"
    ln -s "$spdk" "${wt}/deps/spdk"; ln -s "$build" "${wt}/deps/spdk-build"
  fi
  if [[ ! -x "${wt}/target/release/certus-server-yaml" ]]; then
    [[ -e "${GDRCOPY_LIB_PATH}/libgdrapi.so" ]] || die "libgdrapi.so not found in ${GDRCOPY_LIB_PATH} (set GDRCOPY_LIB_PATH)"
    log "building certus-server-yaml against the private SPDK"
    # gpu-services' build.rs does not rerun on GDRCOPY_LIB_PATH changes; clean it so it does.
    ( cd "$wt" && cargo clean --release -p gpu-services >/dev/null 2>&1
      GDRCOPY_LIB_PATH="$GDRCOPY_LIB_PATH" cargo build --release -p certus-server-yaml ) \
      >"${SPDK_TIER_DIR}/server-build.log" 2>&1 || die "server build failed; see ${SPDK_TIER_DIR}/server-build.log"
  fi
  SERVER_BIN="${wt}/target/release/certus-server-yaml"
}

# ── Pre-flight ──────────────────────────────────────────────────────────────────────
preflight(){
  local bad=0 hp="/sys/devices/system/node/node${CPUNODE}/hugepages/hugepages-1048576kB"
  local need=$((CPU_TIER_GB + 4)) free
  free="$(cat "${hp}/free_hugepages" 2>/dev/null || echo 0)"
  (( free >= need )) || { echo "  FAIL: node${CPUNODE} has ${free} free 1G hugepages, need >= ${need}:" \
    "echo $((need + 4)) | sudo tee ${hp}/nr_hugepages"; bad=1; }
  [[ -w /dev/hugepages ]] || { echo "  FAIL: /dev/hugepages not writable: sudo chmod 1777 /dev/hugepages"; bad=1; }
  for pci in $DEVICE_PCIS; do
    local lk="/var/tmp/spdk_pci_lock_${pci}"
    if [[ -e "$lk" && "$(stat -c %U "$lk")" != "$USER" ]]; then
      echo "  FAIL: ${lk} owned by $(stat -c %U "$lk"): sudo rm -f ${lk}  (if that user is not running SPDK)"; bad=1
    fi
    [[ "$(basename "$(readlink -f "/sys/bus/pci/devices/${pci}/driver" 2>/dev/null)")" == vfio-pci ]] \
      || { echo "  FAIL: ${pci} is not bound to vfio-pci"; bad=1; }
  done
  command -v guidellm >/dev/null || { echo "  FAIL: guidellm not on PATH: uv tool install --python 3.12 guidellm"; bad=1; }
  for img in "$SHMQ_IMAGE" "$CPUTIER_IMAGE"; do
    podman --root "$PODMAN_STORE" --runroot "$PODMAN_RUNROOT" image exists "$img" 2>/dev/null \
      || { echo "  FAIL: image ${img} not in podman store ${PODMAN_STORE}"; bad=1; }
  done
  pgrep -f 'certus-server-yaml.*--shm-path' >/dev/null && { echo "  FAIL: a certus-server is already running"; bad=1; }
  curl -fsS --max-time 3 http://127.0.0.1:8000/v1/models >/dev/null 2>&1 && { echo "  FAIL: port 8000 already serving"; bad=1; }
  local used; used="$(nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits -i 0 2>/dev/null || echo -1)"
  (( used >= 0 && used < 1024 )) || { echo "  FAIL: GPU 0 busy or not visible (${used} MiB used)"; bad=1; }
  (( bad == 0 )) || die "pre-flight failed (see above)"
  log "pre-flight OK (node${CPUNODE} hugepages free=${free}, store ${PODMAN_STORE})"
}

# Probe: the server must actually allocate the tier before we spend 80 min per sweep point.
probe_server(){
  local shm=/dev/shm/certus-shmq-probe plog="${SWEEP_DIR}/server-probe.log" pid
  log "probing certus-server with a ${CPU_TIER_GB}G tier"
  rm -f "$shm"
  SERVER_BIN="$SERVER_BIN" MEMORY_TIER_SIZE="${CPU_TIER_GB}G" SHM_PATH="$shm" DEVICE_PCIS="$DEVICE_PCIS" \
    CPUNODE="$CPUNODE" "${SCRIPT_DIR}/start-certus-optimized.sh" >"$plog" 2>&1 &
  pid=$!
  for _ in $(seq 1 120); do [[ -e "$shm" ]] && break; kill -0 "$pid" 2>/dev/null || break; sleep 1; done
  local ok=0; [[ -e "$shm" ]] && ok=1
  pkill -TERM -f "certus-server-yaml.*--shm-path ${shm}" 2>/dev/null
  for _ in $(seq 1 60); do pgrep -f "certus-server-yaml.*--shm-path ${shm}" >/dev/null || break; sleep 1; done
  pkill -KILL -f "certus-server-yaml.*--shm-path ${shm}" 2>/dev/null; rm -f "$shm"
  # Hugepages are released a few seconds after the process exits; the first certus arm
  # would otherwise race them and fail its tier allocation.
  local hp="/sys/devices/system/node/node${CPUNODE}/hugepages/hugepages-1048576kB/free_hugepages"
  for _ in $(seq 1 60); do (( $(cat "$hp") >= CPU_TIER_GB + 4 )) && break; sleep 1; done
  (( ok )) || die "certus-server failed to start with a ${CPU_TIER_GB}G tier: $(grep -m1 -E 'Error|ERROR' "$plog")"
  log "probe OK: $(grep -m1 -o 'memory-tier: initialized with [0-9]* MiB pool' "$plog")"
}

# Cross-sweep summary from each report's results table.
write_summary(){
  /usr/bin/python3 - "$SWEEP_DIR" $STREAMS <<'EOF'
import html, os, re, sys
sweep, streams = sys.argv[1], sys.argv[2:]
cols = ["offload block (tok)", "run (s)", "tokens/s", "req/s", "TTFT p50 (s)", "TTFT p99 (s)", "errored",
        "vLLM queue mean (s)", "ext hit %", "computed %", "KV load (TiB)", "load GB/s, wall clock (busiest 5 min)", "loads from SSD %", "load + tier read time (s)"]
rows = []
for n in streams:
    p = os.path.join(sweep, f"{n}streams", "report.html")
    if not os.path.exists(p):
        rows.append([n, "(no report)"] + [""] * len(cols)); continue
    # The report has a results table and a KV I/O table, one row per arm in each;
    # merge them by arm.
    by_arm = {}
    for t in re.findall(r"<table[\s\S]*?</table>", open(p).read()):
        trs = [[html.unescape(re.sub(r"<[^>]+>", "", c)).strip() for c in re.findall(r"<t[dh][^>]*>([\s\S]*?)</t[dh]>", tr)]
               for tr in re.findall(r"<tr[\s\S]*?</tr>", t)]
        hdr = trs[0]; ai = hdr.index("Arm")
        for r in trs[1:]:
            by_arm.setdefault(r[ai], {}).update(zip(hdr, r))
    for arm, vals in by_arm.items():
        rows.append([n, arm] + [vals.get(c, "") for c in cols])
head = ["streams", "arm"] + cols
md = ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)] + ["| " + " | ".join(r) + " |" for r in rows]
open(os.path.join(sweep, "summary.md"), "w").write("\n".join(md) + "\n")
with open(os.path.join(sweep, "summary.csv"), "w") as f:
    f.write(",".join(head) + "\n")
    for r in rows: f.write(",".join('"%s"' % c.replace(",", "") for c in r) + "\n")
print("\n".join(md))
EOF
}

# ── Main ────────────────────────────────────────────────────────────────────────────
MATRIX="${SCRIPT_DIR}/run-${FAMILY}-iobound-matrix.sh"
[[ -x "$MATRIX" ]] || die "FAMILY=${FAMILY}: ${MATRIX} not found (use gqa or mha)"
[[ -n "$PODMAN_STORE" && -n "$PODMAN_RUNROOT" ]] || die "could not determine podman store (set PODMAN_STORE/PODMAN_RUNROOT)"
mkdir -p "$SWEEP_DIR"
exec > >(tee -a "${SWEEP_DIR}/sweep.log") 2>&1
log "sweep (${FAMILY}) -> ${SWEEP_DIR}  streams: ${STREAMS}  tier: ${CPU_TIER_GB}G both backends"

preflight
[[ -n "$SERVER_BIN" ]] || build_server
[[ -x "$SERVER_BIN" ]] || die "SERVER_BIN not executable: ${SERVER_BIN}"
log "certus-server binary: ${SERVER_BIN}"
probe_server
[[ "${PREFLIGHT_ONLY:-0}" == 1 ]] && { log "PREFLIGHT_ONLY=1 — exiting"; exit 0; }

for n in $STREAMS; do
  dir="${SWEEP_DIR}/${n}streams"
  log "===== ${n} concurrent streams -> ${dir} ====="
  RESULTS_DIR="$dir" SERVER_BIN="$SERVER_BIN" CONCURRENT_STREAMS="$n" CPU_TIER_GB="$CPU_TIER_GB" \
    DEVICE_PCIS="$DEVICE_PCIS" CPUNODE="$CPUNODE" PODMAN_STORE="$PODMAN_STORE" PODMAN_RUNROOT="$PODMAN_RUNROOT" \
    "$MATRIX" >"${SWEEP_DIR}/matrix-${n}streams.log" 2>&1
  failed=$(ls -d "$dir"/*/FAILED 2>/dev/null | wc -l)
  if [[ -f "${dir}/report.html" ]]; then
    log "${n} streams done -> ${dir}/report.html (${failed} failed arms)"
  else
    log "${n} streams: NO report (see ${SWEEP_DIR}/matrix-${n}streams.log)"
  fi
done

log "sweep summary (${SWEEP_DIR}/summary.md, summary.csv):"
write_summary
log "SWEEP DONE -> ${SWEEP_DIR}"
