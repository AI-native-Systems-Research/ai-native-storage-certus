#!/usr/bin/env bash
# profile-hardware-ceiling.sh
#
# Standalone, offline port of the `profile-hardware-ceiling` Claude skill.
# Measures raw hardware bandwidth ceilings for every Certus data path and
# writes the same two artifacts the skill produces:
#
#     <outdir>/hardware-ceiling-<YYYY-MM-DD>.md
#     <outdir>/hardware_profile_<YYYY-MM-DD>.yaml
#
# No Claude and no internet required. Auto-detects hardware, runs whatever
# tools are present, and SKIPS (never fakes) any path whose tool is missing.
#
# ---------------------------------------------------------------------------
# DESTRUCTIVE: with --nvme (default on if spdk_nvme_perf is found) this script
#   * kills any running certus-server-yaml (it holds VFIO groups), and
#   * rebinds ALL NVMe drives to vfio-pci via SPDK setup.sh.
# Drives are NOT automatically rebound to the kernel driver afterward (matches
# the skill's Step 15). Run `deps/spdk/scripts/setup.sh reset` to restore.
# Pass --no-nvme to skip every step that touches drive bindings.
# ---------------------------------------------------------------------------
#
# Usage:
#   sudo tools/hw-ceiling/profile-hardware-ceiling.sh [outdir]
#   sudo tools/hw-ceiling/profile-hardware-ceiling.sh --no-nvme profiling/ru10
#   tools/hw-ceiling/profile-hardware-ceiling.sh --detect-only
#
# Options:
#   --no-nvme        Skip the SPDK/vfio Paths 1,2,6,9 and the vfio rebind entirely.
#   --detect-only    Only Steps 1-3 (platform detect + cpu features + nvme inspect). No benches.
#   --skip-build     Do not attempt to compile the tools/hw-ceiling CUDA/C benches.
#   --total-bytes N  nvme-bar1-bench total bytes per drive (default 2147483648 = 2GiB).
#   --no-fio         Skip the fio kernel-driver NVMe path (Path F) even if fio is present.
#   --fio-write      ALSO run fio sequential WRITE. DESTRUCTIVE: overwrites raw-device
#                    data on every /dev/nvme*n1 tested. Omitted by default (read-only).
#   --fio-runtime N  Seconds per fio run (default 5).
#   -h, --help       This help.
#
# fio note: Path F runs fio over the KERNEL driver (/dev/nvme*n1), so it needs NO
# vfio rebind and is non-destructive by default (reads only). It complements the
# SPDK paths — a kernel-side NVMe number you can get inside a container without
# tearing down drive bindings. The skill's inspect-nvme.py (Step 3) also needs fio.
#
# Run from the repo root (it resolves paths relative to the repo).

set -u  # deliberately NOT -e: a failing bench must skip, not abort the run.

# ---------------------------------------------------------------------------
# Arg parsing
# ---------------------------------------------------------------------------
OUTDIR="profiling"
DO_NVME=auto
DETECT_ONLY=0
SKIP_BUILD=0
TOTAL_BYTES=2147483648
DO_FIO=auto          # fio kernel-driver NVMe path: auto = run if fio + /dev/nvme present
FIO_WRITE=0          # fio sequential WRITE is destructive to raw devices — opt-in only
FIO_RUNTIME=5        # seconds per fio run

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-nvme)      DO_NVME=0; shift ;;
    --detect-only)  DETECT_ONLY=1; shift ;;
    --skip-build)   SKIP_BUILD=1; shift ;;
    --total-bytes)  TOTAL_BYTES="$2"; shift 2 ;;
    --no-fio)       DO_FIO=0; shift ;;
    --fio-write)    FIO_WRITE=1; shift ;;   # DESTROYS raw-device data — see note below
    --fio-runtime)  FIO_RUNTIME="$2"; shift 2 ;;
    -h|--help)      sed -n '2,52p' "$0"; exit 0 ;;
    -*)             echo "unknown option: $1" >&2; exit 2 ;;
    *)              OUTDIR="$1"; shift ;;
  esac
done

# Privilege prefix: empty when already root (e.g. inside the container) or when
# sudo is absent; otherwise `sudo`. Avoids "sudo: command not found" in minimal
# container images that run as root and ship no sudo.
if [[ "$(id -u)" -eq 0 ]]; then
  SUDO=""
elif command -v sudo >/dev/null 2>&1; then
  SUDO="sudo"
else
  SUDO=""
  echo "[hw-ceiling] warning: not root and no sudo; privileged steps may fail" >&2
fi

DATE="$(date +%Y-%m-%d)"
# hostname binary may be absent in minimal images; fall back to /etc/hostname
# (host's, when mounted) or $HOSTNAME, and strip any domain suffix.
host_short() {
  local h
  h="$(hostname -s 2>/dev/null)" && [[ -n "$h" ]] && { echo "$h"; return; }
  h="$(cat /etc/hostname 2>/dev/null)" || h="${HOSTNAME:-unknown}"
  echo "${h%%.*}"
}
HOST="$(host_short)"
mkdir -p "$OUTDIR"
MD="$OUTDIR/hardware-ceiling-${DATE}.md"
YAML="$OUTDIR/hardware_profile_${DATE}.yaml"
RAW="$OUTDIR/raw-${DATE}"        # captured stdout from each tool, for audit
mkdir -p "$RAW"

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$REPO_ROOT" || { echo "cannot cd to repo root" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
log()  { printf '\033[1;36m[hw-ceiling]\033[0m %s\n' "$*" >&2; }
warn() { printf '\033[1;33m[skip]\033[0m %s\n' "$*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

# Append to the markdown report.
md()   { printf '%s\n' "$*" >> "$MD"; }
# Append to the yaml profile.
yml()  { printf '%s\n' "$*" >> "$YAML"; }

# MiB/s -> decimal GB/s, per the skill's canonical formula. $1 = MiB/s
mib_to_gbps() { awk -v m="$1" 'BEGIN{printf "%.2f", m*1.048576/1000}'; }

# Find first existing path from a list; echo it or empty.
first_of() { for p in "$@"; do [[ -e "$p" ]] && { echo "$p"; return; }; done; }

# Tool discovery (portable — do not hardcode).
NVCC="$(first_of /usr/local/cuda*/bin/nvcc 2>/dev/null)"; [[ -z "$NVCC" ]] && have nvcc && NVCC="$(command -v nvcc)"
SPDK_PERF="$(first_of deps/spdk-build/bin/spdk_nvme_perf 2>/dev/null)"
SPDK_SETUP="$(first_of deps/spdk/scripts/setup.sh deps/spdk-build/../scripts/setup.sh 2>/dev/null)"
BAR1_BENCH="$(first_of target/release/nvme-bar1-bench 2>/dev/null)"
GDSIO="$(command -v gdsio 2>/dev/null || first_of /usr/local/cuda*/gds/tools/gdsio 2>/dev/null)"
GDRCOPY_BW="$(command -v gdrcopy_copybw 2>/dev/null || first_of /usr/local/gdrcopy*/bin/gdrcopy_copybw 2>/dev/null)"
FIO="$(command -v fio 2>/dev/null)"

# Decide whether NVMe (destructive) paths run.
if [[ "$DO_NVME" == auto ]]; then
  if [[ -n "$SPDK_PERF" || -n "$BAR1_BENCH" ]]; then DO_NVME=1; else DO_NVME=0; fi
fi

# ---------------------------------------------------------------------------
# Start reports
# ---------------------------------------------------------------------------
: > "$MD"
: > "$YAML"
md "# Hardware Ceiling Profile — ${HOST}"
md ""
md "_Generated ${DATE} by \`profile-hardware-ceiling.sh\` (offline standalone port)._"
md ""
yml "# Auto-generated by profile-hardware-ceiling.sh (offline port)"
yml "# Date: ${DATE}"
yml "# Node: ${HOST}"
yml ""

# ===========================================================================
# STEP 1 — Platform detection (read-only)
# ===========================================================================
log "Step 1: platform detection"
md "## 1. Platform"
md ""
md '```'
{ echo "### hostname"; host_short
  echo; echo "### uname -r"; uname -r
  echo; echo "### /etc/os-release"; cat /etc/os-release 2>/dev/null
  echo; echo "### lscpu"; lscpu 2>/dev/null
  echo; echo "### numactl --hardware"; (have numactl && numactl --hardware) 2>/dev/null || echo "numactl: not installed"
  echo; echo "### nvidia-smi -L"; (have nvidia-smi && nvidia-smi -L) 2>/dev/null || echo "nvidia-smi: not installed"
  echo; echo "### NVMe block devices"; ls -1 /dev/nvme*n1 2>/dev/null || echo "no /dev/nvme*n1"
  echo; echo "### lspci -tv"; lspci -tv 2>/dev/null || echo "lspci: not available"
} | tee "$RAW/platform.txt" >> "$MD"
md '```'
md ""

# nvidia-smi -q is large; keep it in raw only, extract BAR1 later.
(have nvidia-smi && nvidia-smi -q > "$RAW/nvidia-smi-q.txt" 2>/dev/null) || true

# Per-GPU static info -> CSV for the YAML emitter (index,name,bus_id,gen,width).
# BAR1 is pulled separately in the Python emitter from nvidia-smi-q.txt.
: > "$RAW/gpu-static.csv"
if have nvidia-smi; then
  nvidia-smi --query-gpu=index,name,pci.bus_id,pcie.link.gen.max,pcie.link.width.max \
             --format=csv,noheader,nounits > "$RAW/gpu-static.csv" 2>/dev/null || true
fi

# PCIe theoretical bandwidth table (GT/s x lanes x 128/130 encoding), skill Step 1.
md "### PCIe theoretical per-link bandwidth"
md ""
md "| Gen | GT/s | x4 (GB/s) | x16 (GB/s) |"
md "|-----|-----:|----------:|-----------:|"
awk 'BEGIN{
  split("3 8 4 16 5 32",a," ");
  for(i=1;i<=3;i++){g=a[(i-1)*2+1];t=a[(i-1)*2+2];
    x4=t*4*128/130/8; x16=t*16*128/130/8;
    printf "| Gen%s | %d | %.2f | %.2f |\n",g,t,x4,x16}}' >> "$MD"
md ""

CPU_MODEL="$(lscpu 2>/dev/null | awk -F: '/Model name/{gsub(/^ +/,"",$2);print $2;exit}')"
NUMA_NODES="$(lscpu 2>/dev/null | awk -F: '/NUMA node\(s\)/{gsub(/ /,"",$2);print $2;exit}')"
KERNEL="$(uname -r)"
OSNAME="$(. /etc/os-release 2>/dev/null; echo "${PRETTY_NAME:-unknown}")"

yml "platform:"
yml "  hostname: ${HOST}"
yml "  cpu: \"${CPU_MODEL:-unknown}\""
yml "  numa_nodes: ${NUMA_NODES:-1}"
yml "  kernel: \"${KERNEL}\""
yml "  os: \"${OSNAME}\""
yml ""

# ===========================================================================
# STEP 2 — CPU feature check
# ===========================================================================
log "Step 2: CPU feature check"
md "## 2. CPU Features (check-amd-proc-features.py)"
md ""
if [[ -f tools/check-amd-proc-features.py ]]; then
  # This tool uses 3.12-only f-string syntax (backslash in expression). RHEL 9.6
  # ships Python 3.9, which cannot even parse it. Prefer python3.12 if present,
  # else fall back and tell the user plainly rather than dump a SyntaxError.
  CPUFEAT_PY=""
  for cand in python3.12 python3.13 python3; do
    if have "$cand"; then
      v="$("$cand" -c 'import sys;print(sys.version_info[:2]>=(3,12))' 2>/dev/null)"
      [[ "$v" == "True" ]] && { CPUFEAT_PY="$cand"; break; }
    fi
  done
  if [[ -n "$CPUFEAT_PY" ]]; then
    if "$CPUFEAT_PY" tools/check-amd-proc-features.py > "$RAW/cpu-features.txt" 2>&1; then
      md '```'; cat "$RAW/cpu-features.txt" >> "$MD"; md '```'
    else
      md "_Ran under $CPUFEAT_PY but failed (is \`py-cpuinfo\` installed for that interpreter? \`$CPUFEAT_PY -m pip install --user py-cpuinfo\`):_"
      md '```'; cat "$RAW/cpu-features.txt" >> "$MD"; md '```'
      warn "check-amd-proc-features.py failed (see report)"
    fi
  else
    md "_Skipped: \`tools/check-amd-proc-features.py\` needs Python >= 3.12 (uses 3.12-only f-string syntax),"
    md "but no python3.12 was found. RHEL 9.6 ships 3.9. Install with \`sudo dnf install python3.12\`"
    md "(via the Rocky repo/proxy), then re-run. CPU SIMD flags are still visible in \`lscpu\` above._"
    warn "no python>=3.12 for check-amd-proc-features.py; cpu_features skipped"
    : > "$RAW/cpu-features.txt"
  fi
else
  md "_tools/check-amd-proc-features.py not found — skipped._"
fi
md ""

# ===========================================================================
# STEP 3 — NVMe device inspection (kernel driver required)
# ===========================================================================
log "Step 3: NVMe device inspection"
md "## 3. NVMe Device Inspection (inspect-nvme.py, --skip-gc --skip-power)"
md ""
NVME_DEVS=( $(ls -1 /dev/nvme*n1 2>/dev/null) )
yml "nvme:"
yml "  drives:"
if [[ ${#NVME_DEVS[@]} -eq 0 ]]; then
  md "_No /dev/nvme*n1 devices present (or all bound to vfio-pci) — skipped._"
  yml "    []   # none detected on kernel driver"
elif [[ ! -f tools/inspect-nvme.py ]]; then
  md "_tools/inspect-nvme.py not found — skipped._"
  yml "    []   # inspect-nvme.py missing"
else
  for dev in "${NVME_DEVS[@]}"; do
    j="$RAW/inspect-$(basename "$dev").json"
    # Pass PATH + user-site through sudo: inspect-nvme.py shells out to `fio`, which
    # sudo's secure_path may not resolve; -E/explicit PATH keeps the caller's PATH.
    if $SUDO env "PATH=$PATH" python3 tools/inspect-nvme.py "$dev" --skip-gc --skip-power --json > "$j" 2>"$RAW/inspect-$(basename "$dev").err"; then
      md "### $dev"
      md '```json'; cat "$j" >> "$MD"; md '```'
      # Minimal YAML extraction via python (no jq dependency assumed).
      python3 -I - "$dev" "$j" >> "$YAML" <<'PY'
import json,sys
dev,path=sys.argv[1],sys.argv[2]
try:
    d=json.load(open(path))
except Exception:
    print(f"    - bdf: null  # {dev}: inspect JSON unreadable"); sys.exit()
idn=d.get("identify",{})
cap=idn.get("capacity_bytes")
print(f"    - device: \"{dev}\"")
print(f"      model: \"{idn.get('model','unknown')}\"")
print(f"      firmware_rev: \"{idn.get('firmware_rev','')}\"")
print(f"      serial: \"{idn.get('serial','')}\"")
if cap: print(f"      capacity_tb: {cap/1e12:.3f}")
print(f"      mdts_bytes: {idn.get('mdts_bytes','null')}")
print(f"      block_size: {idn.get('block_size','null')}")
vwc=idn.get('vwc')
if vwc is not None: print(f"      vwc_enabled: {bool(vwc & 0x1)!s}".lower().replace('vwc_enabled: ','vwc_enabled: '))
PY
    else
      md "### $dev — inspection failed"
      md '```'; cat "$RAW/inspect-$(basename "$dev").err" >> "$MD"; md '```'
      yml "    - device: \"${dev}\"  # inspect failed"
      warn "inspect-nvme.py failed on $dev"
    fi
  done
fi
yml ""

if [[ "$DETECT_ONLY" == 1 ]]; then
  log "--detect-only: skipping benchmarks, still emitting YAML from detection"
  md ""
  md "_--detect-only: benchmark paths (3-9) not executed; YAML holds detection + whatever was already measured._"
fi

# ===========================================================================
# STEP 3.5 — Build the CUDA/C benches (Paths 3,4,7)
# ===========================================================================
if [[ "$DETECT_ONLY" != 1 && "$SKIP_BUILD" == 0 ]]; then
  if [[ -n "$NVCC" ]]; then
    log "Building tools/hw-ceiling benches (nvcc=$NVCC)"
    make -C tools/hw-ceiling OUTDIR=/tmp NVCC="$NVCC" > "$RAW/build.txt" 2>&1 || warn "make failed (see raw/build.txt)"
  else
    warn "nvcc not found — CUDA benches (Paths 3,4) cannot be built; memcpy still can"
    make -C tools/hw-ceiling OUTDIR=/tmp /tmp/memcpy_bench >> "$RAW/build.txt" 2>&1 || warn "memcpy build failed"
  fi
fi

# Helper: run a prebuilt bench if present, capture + embed.
# $4 (optional) = distinct raw-file suffix so repeated calls (per NUMA node) don't clobber.
run_bench() {  # $1 = /tmp/binary  $2 = markdown heading  $3 = numactl-prefix (may be empty)  $4 = suffix
  local bin="$1" head="$2" pfx="$3" sfx="${4:-}" name; name="$(basename "$bin")${sfx}"
  md "## ${head}"; md ""
  if [[ -x "$bin" ]]; then
    log "Running $name ${pfx:+($pfx)}"
    if $pfx "$bin" > "$RAW/${name}.txt" 2>&1; then
      md '```'; cat "$RAW/${name}.txt" >> "$MD"; md '```'
    else
      md "_\`$name\` exited non-zero — partial output:_"
      md '```'; cat "$RAW/${name}.txt" >> "$MD"; md '```'
      warn "$name failed"
    fi
  else
    md "_\`$bin\` not built (nvcc missing or build failed) — skipped._"
  fi
  md ""
}

# ===========================================================================
# BENCHMARKS (Paths 3-9) — skipped entirely under --detect-only
# ===========================================================================
if [[ "$DETECT_ONLY" != 1 ]]; then

# ===========================================================================
# PATHS 3 & 4 & 7 — GPU H2D/D2H, GPU D2D, host RAM memcpy
# ===========================================================================
# NUMA-local vs cross-NUMA: the skill wraps cuda_bw_test in numactl per GPU
# NUMA node. Without a reliable GPU->NUMA map here we run plain + (if numactl
# and >1 node) both node bindings so you have the deltas.
run_bench /tmp/memcpy_bench "4. Host RAM memcpy baseline (Path 7)" ""

if [[ -x /tmp/cuda_bw_test ]]; then
  if have numactl && [[ "${NUMA_NODES:-1}" -gt 1 ]]; then
    for n in $(seq 0 $((NUMA_NODES-1))); do
      run_bench /tmp/cuda_bw_test "5. GPU H2D/D2H — cudaMemcpy (Path 3), NUMA node ${n}" "numactl --cpunodebind=${n} --membind=${n}" "_numa${n}"
    done
  else
    run_bench /tmp/cuda_bw_test "5. GPU H2D/D2H — cudaMemcpy (Path 3)" ""
  fi
else
  md "## 5. GPU H2D/D2H — cudaMemcpy (Path 3)"; md ""; md "_cuda_bw_test not built — skipped._"; md ""
fi

run_bench /tmp/cuda_d2d_test "6. GPU D2D — cudaMemcpyDeviceToDevice (Path 4)" ""

# ===========================================================================
# PATH 5 — GDRCopy CPU<->BAR1 (diagnostic)
# ===========================================================================
md "## 7. GDRCopy CPU<->BAR1 (Path 5)"; md ""
if [[ -n "$GDRCOPY_BW" ]]; then
  log "Running gdrcopy_copybw"
  "$GDRCOPY_BW" > "$RAW/gdrcopy.txt" 2>&1 || warn "gdrcopy_copybw failed"
  md '```'; cat "$RAW/gdrcopy.txt" >> "$MD"; md '```'
else
  md "_gdrcopy_copybw not found — skipped. Build from kernel/modules/gdrcopy/tests if needed._"
fi
md ""

# ===========================================================================
# PATH F — fio NVMe over the KERNEL driver (non-destructive, no vfio rebind)
# ===========================================================================
# Runs BEFORE the destructive SPDK rebind so drives are still /dev/nvme*n1.
# Read is always safe. Write only with --fio-write (it overwrites the raw device).
md "## 8. NVMe via fio (kernel driver) (Path F)"; md ""
[[ "$DO_FIO" == auto ]] && { [[ -n "$FIO" ]] && DO_FIO=1 || DO_FIO=0; }
FIO_DEVS=( $(ls -1 /dev/nvme*n1 2>/dev/null) )
if [[ "$DO_FIO" != 1 ]]; then
  md "_Skipped (--no-fio, or fio not found)._"
elif [[ ${#FIO_DEVS[@]} -eq 0 ]]; then
  md "_fio present but no /dev/nvme*n1 on the kernel driver (all bound to vfio-pci?) — skipped._"
else
  # Workloads: read always; write only on explicit opt-in.
  FIO_RW=(read)
  [[ "$FIO_WRITE" == 1 ]] && FIO_RW+=(write)
  if [[ "$FIO_WRITE" == 1 ]]; then
    md "> **WARNING:** \`--fio-write\` given — sequential WRITE will OVERWRITE raw-device data on the drives below."
    md ""
    warn "fio WRITE enabled (--fio-write): this overwrites raw data on ${FIO_DEVS[*]}"
  fi
  md "Devices: \`${FIO_DEVS[*]}\`  |  block sizes: 4k 64k 128k 1M  |  QD 32  |  ${FIO_RUNTIME}s each"
  md ""
  md "| device | rw | block | GB/s | IOPS | avg lat us |"
  md "|--------|----|------:|-----:|-----:|-----------:|"
  for dev in "${FIO_DEVS[@]}"; do
    for rw in "${FIO_RW[@]}"; do
      for bs in 4k 64k 128k 1M; do
        tag="fio_$(basename "$dev")_${rw}_${bs}"
        # --direct=1 bypasses page cache (raw device ceiling); JSON for clean parse.
        if $SUDO "$FIO" --name="$tag" --filename="$dev" --rw="$rw" --bs="$bs" \
              --direct=1 --ioengine=libaio --iodepth=32 --runtime="$FIO_RUNTIME" \
              --time_based --output-format=json > "$RAW/${tag}.json" 2>"$RAW/${tag}.err"; then
          # Parse one job's read or write block from fio JSON (stdlib only).
          read -r gbps iops lat < <(python3 - "$RAW/${tag}.json" "$rw" <<'PY'
import json,sys
try:
    d=json.load(open(sys.argv[1])); j=d["jobs"][0][sys.argv[2]]
    bw_kibps=j.get("bw",0)                       # fio reports bw in KiB/s
    gbps=bw_kibps*1024/1e9                        # -> decimal GB/s
    lat=j.get("lat_ns",{}).get("mean",0)/1000.0   # ns -> us
    print(f"{gbps:.2f} {int(j.get('iops',0))} {lat:.1f}")
except Exception:
    print("? ? ?")
PY
)
          md "| $(basename "$dev") | $rw | $bs | ${gbps} | ${iops} | ${lat} |"
          echo "$(basename "$dev"),${rw},${bs},${gbps},${iops},${lat}" >> "$RAW/fio_results.csv"
        else
          md "| $(basename "$dev") | $rw | $bs | FAIL | - | - |"
          warn "fio $tag failed (see raw/${tag}.err)"
        fi
      done
    done
  done
fi
md ""

# ===========================================================================
# NVMe paths (DESTRUCTIVE) — Steps 4,5,6,10,13 => Paths 1,2,6,9
# ===========================================================================
md "## 9. NVMe data paths (SPDK / BAR1) (Paths 1,2,6,9)"; md ""
if [[ "$DO_NVME" != 1 ]]; then
  md "_Skipped (--no-nvme, or neither spdk_nvme_perf nor nvme-bar1-bench is built)._"
  md ""
else
  log "Step 4: releasing VFIO + binding NVMe to vfio-pci (DESTRUCTIVE)"
  $SUDO pkill -f certus-server-yaml 2>/dev/null || true
  sleep 2
  if [[ -n "$SPDK_SETUP" ]]; then
    $SUDO "$SPDK_SETUP" > "$RAW/spdk-setup.txt" 2>&1 || warn "SPDK setup.sh returned non-zero"
  else
    warn "SPDK setup.sh not found — cannot bind to vfio-pci; NVMe paths may fail"
  fi
  md "Drives bound via: ${SPDK_SETUP:-<not found>}"
  md '```'; cat "$RAW/spdk-setup.txt" 2>/dev/null >> "$MD"; md '```'
  md ""

  # Discover NVMe PCI BDFs now bound to vfio-pci.
  BDFS=( $(lspci -D 2>/dev/null | awk '/Non-Volatile memory|NVMe/{print $1}') )
  log "NVMe BDFs: ${BDFS[*]:-none}"
  md "Detected NVMe BDFs: \`${BDFS[*]:-none}\`"; md ""

  # ---- Path 1 & 2: spdk_nvme_perf read/write matrices ----
  if [[ -n "$SPDK_PERF" && ${#BDFS[@]} -gt 0 ]]; then
    SIZES=(4096 16384 65536 131072 262144 524288 1048576 2097152 4194304 8388608)
    # Drive-count rungs capped at what's present.
    COUNTS=(1)
    [[ ${#BDFS[@]} -ge 2 ]] && COUNTS+=(2)
    [[ ${#BDFS[@]} -ge 4 ]] && COUNTS+=(4)
    [[ ${#BDFS[@]} -ge 6 ]] && COUNTS+=(6)
    [[ ${#BDFS[@]} -gt 6 ]] && COUNTS+=(${#BDFS[@]})

    for workload in read write; do
      md "### Path $([ $workload = read ] && echo 1 || echo 2): NVMe sequential ${workload} (spdk_nvme_perf)"
      md ""
      md "| drives | block | QD | GB/s | avg lat us |"
      md "|-------:|------:|---:|-----:|-----------:|"
      for nd in "${COUNTS[@]}"; do
        # build -r flags for first nd drives
        rflags=(); for ((i=0;i<nd;i++)); do rflags+=( -r "trtype:pcie traddr:${BDFS[$i]}" ); done
        for o in "${SIZES[@]}"; do
          for qd in 16 32; do
            tag="${workload}_d${nd}_o${o}_q${qd}"
            if $SUDO "$SPDK_PERF" "${rflags[@]}" -q "$qd" -o "$o" -w "$workload" -t 5 \
                 > "$RAW/perf_${tag}.txt" 2>&1; then
              line="$(awk '/^Total/{print; exit}' "$RAW/perf_${tag}.txt")"
              mibs="$(echo "$line" | awk '{print $4}')"
              lat="$(echo "$line" | awk '{print $5}')"
              gbps="$( [[ -n "$mibs" ]] && mib_to_gbps "$mibs" || echo "?" )"
              md "| $nd | $o | $qd | ${gbps} | ${lat:-?} |"
              # sidecar for the YAML emitter: workload,drives,size,qd,gbps,lat
              echo "${workload},${nd},${o},${qd},${gbps},${lat:-}" >> "$RAW/perf_results.csv"
            else
              md "| $nd | $o | $qd | FAIL | - |"
            fi
          done
        done
      done
      md ""
    done
  else
    md "### Paths 1-2: spdk_nvme_perf — skipped (binary missing or no drives bound)."
    md ""
  fi

  # ---- Path 9: nvme-bar1-bench (the headline P2P ceiling) ----
  md "### Path 9: NVMe DMA -> BAR1 microbench (nvme-bar1-bench)"
  md ""
  if [[ -n "$BAR1_BENCH" ]]; then
    NGPU="$( (have nvidia-smi && nvidia-smi -L | wc -l) 2>/dev/null || echo 0)"
    for nd in 1 4 6; do
      [[ $nd -gt ${#BDFS[@]} ]] && continue
      tag="bar1_d${nd}"
      log "nvme-bar1-bench --drive-count $nd"
      if $SUDO "$BAR1_BENCH" --drive-count "$nd" --gpu 0 \
           --chunk-size 131072 --total-bytes "$TOTAL_BYTES" --queue-depth 32 --iterations 3 \
           > "$RAW/${tag}.txt" 2>&1; then
        md "#### ${nd} drive(s), GPU 0"
        md '```'; cat "$RAW/${tag}.txt" >> "$MD"; md '```'
        # sidecar: parse "Per-drive: host-ram X GB/s, bar1 Y GB/s" + overhead line
        perdrive="$(awk -F'[ ,]+' '/Per-drive:/{print $4","$7}' "$RAW/${tag}.txt")"
        ovh="$(awk '/BAR1 overhead:/{print $3} /BAR1 is .* faster/{print "-"$3}' "$RAW/${tag}.txt" | tr -d '%' | head -1)"
        echo "${nd},${perdrive:-,},${ovh:-}" >> "$RAW/bar1_results.csv"
      else
        md "#### ${nd} drive(s) — FAILED"
        md '```'; cat "$RAW/${tag}.txt" >> "$MD"; md '```'
        warn "nvme-bar1-bench d${nd} failed (gdrdrv loaded?)"
      fi
    done
  else
    md "_nvme-bar1-bench not built — \`cargo build --release -p nvme-bar1-bench\`. Skipped._"
  fi
  md ""

  # ---- Path 6: GDS (optional; rebinds to kernel then back) ----
  md "### Path 6: GPUDirect Storage (gdsio)"
  md ""
  if [[ -n "$GDSIO" ]] && lsmod 2>/dev/null | grep -q nvidia_fs; then
    md "_gdsio present and nvidia_fs loaded. NOT run automatically (requires rebinding drives to"
    md "the kernel driver mid-run). Run manually per the skill's Step 10 if you want GDS numbers._"
  else
    md "_GDS not available (gdsio missing or nvidia_fs not loaded) — skipped._"
  fi
  md ""
fi

fi  # end DETECT_ONLY guard (Paths 3-9)

# ===========================================================================
# YAML: gpus: and host_ram: sections (parsed from bench raw output)
# ===========================================================================
log "Emitting gpus: and host_ram: YAML from bench output"
python3 -I - "$RAW" "$YAML" "${NUMA_NODES:-1}" <<'PY'
import os, re, sys, glob
raw, yaml_path, nnodes = sys.argv[1], sys.argv[2], int(sys.argv[3])

def smi_q_bar1(qpath):
    """Return list of BAR1 total MiB in GPU order from `nvidia-smi -q`."""
    out=[]
    if not os.path.exists(qpath): return out
    txt=open(qpath,errors="replace").read()
    # Split into per-GPU chunks beginning at "GPU 00000000:.."
    chunks=re.split(r'\nGPU 0000', txt)
    for ch in chunks[1:]:
        m=re.search(r'BAR1 Memory Usage.*?Total\s*:\s*([\d.]+)\s*MiB', ch, re.S)
        out.append(m.group(1) if m else None)
    return out

def parse_bw(path):
    """cuda_bw_test: per-GPU dict size->(h2d1,h2d2,h2d4,h2d8,d2h1)."""
    gpus={}
    if not os.path.exists(path): return gpus
    cur=None
    for ln in open(path,errors="replace"):
        m=re.match(r'=== GPU (\d+): (.+?) ===', ln)
        if m: cur=int(m.group(1)); gpus[cur]={"name":m.group(2),"rows":{}}; continue
        if cur is None: continue
        p=ln.split()
        if len(p)==6 and re.match(r'^[\d]+[KM]$', p[0]):
            try: gpus[cur]["rows"][p[0]]=tuple(float(x) for x in p[1:])
            except ValueError: pass
    return gpus

def parse_d2d(path):
    gpus={}
    if not os.path.exists(path): return gpus
    cur=None
    for ln in open(path,errors="replace"):
        m=re.match(r'=== GPU (\d+): (.+?) D2D ===', ln)
        if m: cur=int(m.group(1)); gpus[cur]={}; continue
        if cur is None: continue
        p=ln.split()
        if len(p)==4 and re.match(r'^[\d]+[KM]$', p[0]):
            try: gpus[cur][p[0]]=tuple(float(x) for x in p[1:])
            except ValueError: pass
    return gpus

def peak_h2d(rows):   # max over sizes of 1-stream and 4-stream
    if not rows: return (None,None)
    return (max(r[0] for r in rows.values()), max(r[2] for r in rows.values()))
def peak_d2h(rows):
    return max((r[4] for r in rows.values()), default=None)
def at4m(rows, idx):
    r=rows.get("4M"); return r[idx] if r else None

# Prefer per-NUMA files if present; else the single plain file.
numa_files=sorted(glob.glob(os.path.join(raw,"cuda_bw_test_numa*.txt")))
plain=os.path.join(raw,"cuda_bw_test.txt")
bw_local = parse_bw(numa_files[0]) if numa_files else parse_bw(plain)
bw_cross = parse_bw(numa_files[1]) if len(numa_files)>1 else {}
d2d      = parse_d2d(os.path.join(raw,"cuda_d2d_test.txt"))
bar1s    = smi_q_bar1(os.path.join(raw,"nvidia-smi-q.txt"))

# static CSV: index,name,bus_id,gen,width
static={}
scsv=os.path.join(raw,"gpu-static.csv")
if os.path.exists(scsv):
    for ln in open(scsv):
        f=[x.strip() for x in ln.split(",")]
        if len(f)>=5 and f[0].isdigit():
            static[int(f[0])]={"name":f[1],"bus":f[2],"gen":f[3],"width":f[4]}

out=["gpus:"]
gpu_ids=sorted(set(list(bw_local)+list(d2d)+list(static)))
if not gpu_ids:
    out.append("  {}   # no GPU bench output parsed")
for g in gpu_ids:
    rl=bw_local.get(g,{}).get("rows",{})
    rc=bw_cross.get(g,{}).get("rows",{})
    st=static.get(g,{})
    name=st.get("name") or bw_local.get(g,{}).get("name") or "unknown"
    out.append(f"  gpu{g}:")
    out.append(f"    model: \"{name}\"")
    if st.get("gen"):   out.append(f"    pcie_gen: {st['gen']}")
    if st.get("width"): out.append(f"    pcie_width: {st['width']}")
    if st.get("bus"):   out.append(f"    pci_bdf: \"{st['bus']}\"")
    if g < len(bar1s) and bar1s[g]:
        try: out.append(f"    bar1_gib: {float(bar1s[g])/1024:.0f}")
        except ValueError: pass
    if rl:
        h1,h4=peak_h2d(rl)
        out.append(f"    h2d_4m_1stream_numa_local_gbps: {at4m(rl,0)}")
        out.append(f"    h2d_peak_numa_local_gbps: {h1}")
        out.append(f"    h2d_4stream_peak_numa_local_gbps: {h4}")
        out.append(f"    d2h_peak_numa_local_gbps: {peak_d2h(rl)}")
    if rc:
        hc1,_=peak_h2d(rc)
        out.append(f"    h2d_4m_1stream_cross_numa_gbps: {at4m(rc,0)}")
        out.append(f"    h2d_peak_cross_numa_gbps: {hc1}")
        if rl:
            pl,_=peak_h2d(rl)
            if pl and hc1: out.append(f"    cross_numa_h2d_penalty_gbps: {pl-hc1:.2f}")
    dd=d2d.get(g,{})
    if dd:
        d1=dd.get("8M",dd.get("16M"))
        if d1:
            out.append(f"    d2d_1stream_gbps: {d1[0]}")
            out.append(f"    d2d_4stream_gbps: {d1[2]}")
out.append("")

# host_ram from memcpy_bench
mc=os.path.join(raw,"memcpy_bench.txt")
same=cross=None
if os.path.exists(mc):
    for ln in open(mc,errors="replace"):
        p=ln.split()
        if len(p)>=2 and re.match(r'^[\d]+M', p[0]):
            try: same=float(p[1])
            except: pass
            if len(p)>=3 and p[2]!="N/A":
                try: cross=float(p[2])
                except: pass
out.append("host_ram:")
out.append(f"  same_numa_gbps: {same if same is not None else 'null'}")
out.append(f"  cross_numa_gbps: {cross if cross is not None else 'null'}")
out.append("")

# --- fio kernel-driver NVMe (Path F): peak GB/s per device per rw ---
fcsv=os.path.join(raw,"fio_results.csv")
if os.path.exists(fcsv):
    # dev,rw,bs,gbps,iops,lat  -> peak gbps per (dev,rw)
    peak={}
    for ln in open(fcsv):
        f=ln.strip().split(",")
        if len(f)>=4:
            try: g=float(f[3])
            except ValueError: continue
            peak[(f[0],f[1])]=max(peak.get((f[0],f[1]),0.0),g)
    if peak:
        out.append("fio_nvme_kernel:")
        devs=sorted({d for (d,_) in peak})
        for d in devs:
            out.append(f"  {d}:")
            for rw in ("read","write"):
                if (d,rw) in peak:
                    out.append(f"    peak_{rw}_gbps: {peak[(d,rw)]:.2f}")
        out.append("")

# --- cpu_features from check-amd-proc-features.py text ---
cf=os.path.join(raw,"cpu-features.txt")
if os.path.exists(cf):
    t=open(cf,errors="replace").read()
    def grab(pat):
        m=re.search(pat,t); return m.group(1).strip() if m else None
    vendor=grab(r'Vendor\s*:\s*(\S+)')
    zen=grab(r'Generation\s*:\s*(.+)')
    tier=grab(r'Best path\s*:\s*(\S+)')
    tput=grab(r'Single-core est\.\s*:\s*([^\(]+)')
    rec=grab(r'Recommended\s*:\s*(.+)')
    out.append("cpu_features:")
    out.append(f"  vendor: \"{vendor or 'unknown'}\"")
    if zen: out.append(f"  zen_label: \"{zen}\"")
    out.append(f"  has_avx512: {str('avx512' in t.lower()).lower()}")
    out.append(f"  has_avx2: {str('avx2' in t.lower()).lower()}")
    if tier: out.append(f"  xxh3_tier: \"{tier}\"")
    if tput: out.append(f"  xxh3_throughput_estimate: \"{tput.strip()}\"")
    if rec: out.append(f"  certus_recommended_hash: \"{rec}\"")
    out.append("")

# --- nvme raw_read/write + io_size_sweep from perf_results.csv ---
pcsv=os.path.join(raw,"perf_results.csv")
if os.path.exists(pcsv):
    rows=[]
    for ln in open(pcsv):
        f=ln.strip().split(",")
        if len(f)>=5:
            try: rows.append((f[0],int(f[1]),int(f[2]),int(f[3]),float(f[4])))
            except ValueError: pass
    def best(workload, drives):
        # peak GB/s over sizes at QD=32 for this (workload,drives)
        xs=[g for (w,d,o,q,g) in rows if w==workload and d==drives and q==32]
        return max(xs) if xs else None
    def sweep(workload):
        # 1 drive, QD=32, size->gbps
        s={o:g for (w,d,o,q,g) in rows if w==workload and d==1 and q==32}
        return s
    SZ=[(4096,"4kib"),(16384,"16kib"),(65536,"64kib"),(131072,"128kib"),
        (262144,"256kib"),(524288,"512kib"),(1048576,"1mib"),(2097152,"2mib"),
        (4194304,"4mib"),(8388608,"8mib")]
    def knee(s):
        if not s: return None
        pk=max(s.values())
        for o,_ in SZ:
            if o in s and s[o]>=0.95*pk: return o
        return None
    out.append("nvme_measured:")
    out.append("  raw_read_gbps:")
    for d in (1,4,6):
        v=best("read",d);  out.append(f"    {d}_drive: {v if v else 'null'}")
    out.append("  raw_write_gbps:")
    for d in (1,4):
        v=best("write",d); out.append(f"    {d}_drive: {v if v else 'null'}")
    for wl,key in (("read","io_size_sweep_read_gbps"),("write","io_size_sweep_write_gbps")):
        s=sweep(wl)
        if s:
            out.append(f"  {key}:")
            for o,name in SZ:
                if o in s: out.append(f"    {name}: {s[o]}")
            k=knee(s)
            if k: out.append(f"    optimal_bytes: {k}")
    out.append("")

# --- bar1 from bar1_results.csv (drives,host_ram,bar1,overhead_pct) ---
bcsv=os.path.join(raw,"bar1_results.csv")
bar1_by_d={}
if os.path.exists(bcsv):
    out.append("bar1:")
    out.append("  gpu0:")
    out.append("    numa_local:")
    for ln in open(bcsv):
        f=ln.strip().split(",")
        if len(f)>=3 and f[0].isdigit():
            d=int(f[0]); hr=f[1]; b1=f[2]; ov=f[3] if len(f)>3 else ""
            bar1_by_d[d]=(hr,b1,ov)
            if hr: out.append(f"      host_ram_{d}drive_gbps: {hr}")
            if b1: out.append(f"      bar1_{d}drive_gbps: {b1}")
            if ov: out.append(f"      overhead_{d}drive_pct: {ov}")
    out.append("  # NOTE: nvme-bar1-bench does not print selected drive BDFs; topology")
    out.append("  # of the chosen drives is not verified from stdout (see skill Path 9).")
    out.append("")

# --- gdrcopy ---
gd=os.path.join(raw,"gdrcopy.txt")
if os.path.exists(gd):
    t=open(gd,errors="replace").read()
    nums=re.findall(r'([\d.]+)\s*(?:GB/s|GiB/s|MB/s)', t)
    out.append("gdrcopy:")
    out.append(f"  raw_present: true  # see report for parsed values ({len(nums)} bw figures found)")
    out.append("")

# --- ceilings (derived per skill Step 14 / SKILL.md:749) ---
# warm = NUMA-local H2D peak; dispatcher cold = min(nvme_agg, h2d_peak); p2p cold = bar1.
h2d_peak_g0 = peak_h2d(parse_bw(numa_files[0] if numa_files else plain).get(0,{}).get("rows",{}))[0] \
              if (numa_files or os.path.exists(plain)) else None
def nvme_best(workload,d):
    if not os.path.exists(pcsv): return None
    xs=[]
    for ln in open(pcsv):
        f=ln.strip().split(",")
        if len(f)>=5 and f[0]==workload:
            try:
                if int(f[1])==d and int(f[3])==32: xs.append(float(f[4]))
            except ValueError: pass
    return max(xs) if xs else None
cl=["ceilings:"]
if h2d_peak_g0: cl.append(f"  warm_gpu0_peak_gbps: {h2d_peak_g0}")
for d in (1,4,6):
    nv=nvme_best("read",d)
    if nv is not None:
        cap=min(nv,h2d_peak_g0) if h2d_peak_g0 else nv
        bound="h2d" if (h2d_peak_g0 and h2d_peak_g0<nv) else "nvme"
        cl.append(f"  dispatcher_cold_{d}drive_gbps: {cap:.2f}   # bound by {bound}")
for d in (1,4,6):
    if d in bar1_by_d and bar1_by_d[d][1]:
        cl.append(f"  p2p_cold_{d}drive_gpu0_gbps: {bar1_by_d[d][1]}")
if len(cl)>1:
    out.extend(cl); out.append("")

open(yaml_path,"a").write("\n".join(out)+"\n")
PY

# ===========================================================================
# Driver state report (Step 15)
# ===========================================================================
md "## Final driver state"
md ""
md '```'
{ echo "### drives in vfio-pci:"; ls /sys/bus/pci/drivers/vfio-pci/ 2>/dev/null | grep -E '^[0-9a-f]{4}:' || echo "none"
  echo; echo "### /dev/nvme*n1 (kernel driver):"; ls /dev/nvme*n1 2>/dev/null || echo "none"
} | tee "$RAW/driver-state.txt" >> "$MD"
md '```'
md ""
md "_To restore drives to the kernel NVMe driver: \`sudo ${SPDK_SETUP:-deps/spdk/scripts/setup.sh} reset\`_"

log "Done."
log "Markdown report : $MD"
log "YAML profile    : $YAML"
log "Raw tool output : $RAW/"
echo
echo "NOTE: This script fills the measured paths and leaves skipped ones marked."
echo "The skill's derived summary sub-tables (dispatcher warm/cold, p2p crossover)"
echo "are best computed from the numbers above — paste this report back to Claude"
echo "(on a connected machine) to generate those, or compute from the formulas in"
echo ".claude/skills/profile-hardware-ceiling/SKILL.md (Step 14)."
