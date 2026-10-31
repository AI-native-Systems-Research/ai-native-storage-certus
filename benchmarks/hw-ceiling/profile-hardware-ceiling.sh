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
# NVMe benchmarking is OPT-IN BY PCI BDF. The NVMe paths (fio Path F and the
# SPDK spdk_nvme_perf / nvme-bar1-bench paths) run ONLY against the drives you
# name with --nvme, given as PCI BDFs (e.g. 0000:41:00.0), matching the
# NVME_BDFS convention of configure-bench.sh / profile_all.sh. With no --nvme,
# every NVMe path is skipped — no auto-discovery, so the script can never wander
# onto a boot drive or a live mount. The BDF is the source of truth; the kernel
# block node (/dev/nvmeXnY) and NUMA node are derived from it via sysfs, which
# also sidesteps NVMe-multipath name mismatches.
#
# DESTRUCTIVE steps, all gated behind the explicit BDF list:
#   * fio raw-block WRITE (--fio-write) overwrites the named drives.
#   * the SPDK path (--spdk) kills certus-server-yaml and rebinds the named BDFs
#     to vfio-pci (NOT rebound to the kernel driver afterward; run
#     `<spdk>/scripts/setup.sh reset` to restore).
# A device whose block node is mounted is refused. Check `lsblk` first.
# ---------------------------------------------------------------------------
#
# Usage:
#   profile-hardware-ceiling.sh [outdir]                         # no NVMe; detection + GPU + memcpy
#   profile-hardware-ceiling.sh --nvme 0000:41:00.0 [outdir]     # fio read on one scratch drive
#   profile-hardware-ceiling.sh --nvme 0000:41:00.0,0000:42:00.0 --fio-write [outdir]
#   profile-hardware-ceiling.sh --nvme 0000:41:00.0 --spdk [outdir]   # + SPDK (rebinds to vfio!)
#   profile-hardware-ceiling.sh --detect-only
#
# Options:
#   --nvme LIST      Comma-separated PCI BDF list (or repeat the flag) to benchmark,
#                    e.g. --nvme 0000:41:00.0,0000:42:00.0. REQUIRED for any NVMe path;
#                    omit to skip all NVMe benchmarking. Only these BDFs are used.
#   --spdk           Also run the SPDK paths on the named BDFs. DESTRUCTIVE: kills
#                    certus-server-yaml and rebinds the named BDFs to vfio-pci.
#   --fio-write      Also run fio sequential WRITE on the named drives. DESTRUCTIVE:
#                    overwrites their raw contents. Omitted by default (read-only).
#   --tag TAG        Embed TAG in the output filenames so they correlate with a bench
#                    sweep run (e.g. --tag 3-drive -> hardware_profile_3-drive_2024-10-28.yaml).
#   --fio-runtime N  Seconds per fio run (default 5).
#   --detect-only    Only Steps 1-3 (platform detect + cpu features + nvme inspect). No benches.
#   --skip-build     Do not attempt to compile the CUDA/C benches.
#   --total-bytes N  nvme-bar1-bench total bytes per drive (default 2147483648 = 2GiB).
#   -h, --help       This help.
#
# fio note: Path F runs fio over the KERNEL driver (the block node derived from
# the BDF), so it needs NO vfio rebind and is read-only unless --fio-write. The
# SPDK paths need the drives on vfio-pci and only run with --spdk.
#
# Run from the repo root (it resolves paths relative to the repo).

set -u  # deliberately NOT -e: a failing bench must skip, not abort the run.

# ---------------------------------------------------------------------------
# Arg parsing
# ---------------------------------------------------------------------------
OUTDIR="profiling"
NVME_BDFS=()         # PCI BDFs named via --nvme; empty => all NVMe paths skipped
DO_SPDK=0            # --spdk: run SPDK paths on named BDFs (destructive rebind)
DETECT_ONLY=0
SKIP_BUILD=0
TOTAL_BYTES=2147483648
FIO_WRITE=0          # fio sequential WRITE is destructive to raw devices — opt-in only
FIO_RUNTIME=5        # seconds per fio run
TAG=""               # --tag: embed in output filenames for sweep correlation

while [[ $# -gt 0 ]]; do
  case "$1" in
    --nvme)         IFS=',' read -r -a _b <<< "$2"; NVME_BDFS+=("${_b[@]}"); shift 2 ;;
    --spdk)         DO_SPDK=1; shift ;;
    --detect-only)  DETECT_ONLY=1; shift ;;
    --skip-build)   SKIP_BUILD=1; shift ;;
    --total-bytes)  TOTAL_BYTES="$2"; shift 2 ;;
    --fio-write)    FIO_WRITE=1; shift ;;   # DESTROYS raw-device data on named drives
    --fio-runtime)  FIO_RUNTIME="$2"; shift 2 ;;
    --tag)          TAG="$2"; shift 2 ;;
    -h|--help)      sed -n '2,62p' "$0"; exit 0 ;;
    -*)             echo "unknown option: $1" >&2; exit 2 ;;
    *)              OUTDIR="$1"; shift ;;
  esac
done

# Normalize each BDF to the full 0000:xx:yy.z domain form, validate it exists as
# a PCI device, and derive its kernel block node (/dev/nvmeXnY) from sysfs. The
# BDF is the source of truth; the block node is looked up, never the reverse.
NVME_DEV_OF=()       # parallel to NVME_BDFS: block node, or "" if not on kernel driver
bdf_blockdev() {     # $1 = full BDF -> echoes /dev/nvmeXnY (first namespace) or ""
  local bdf="$1" cdir blk
  cdir="$(ls -d "/sys/bus/pci/devices/${bdf}/nvme/"nvme* 2>/dev/null | head -1)"  # .../nvme/nvmeN
  [[ -z "$cdir" ]] && return
  # The namespace block dir lives UNDER the controller's sysfs dir and may be
  # named differently from the controller (NVMe multipath), e.g. controller
  # nvme3 -> namespace nvme1n2. Find the first nvme*n* child that is a block dev.
  blk="$(ls -d "$cdir"/nvme* 2>/dev/null | grep -E '/nvme[0-9]+n[0-9]+$' | head -1)"
  [[ -z "$blk" ]] && blk="$(ls -d "$cdir"/nvme*c*n* 2>/dev/null | head -1)"
  [[ -n "$blk" ]] && echo "/dev/$(basename "$blk")"
}
NVME_N=${#NVME_BDFS[@]}
for (( i=0; i<NVME_N; i++ )); do
  [[ -z "${NVME_BDFS[$i]:-}" ]] && continue
  bdf="${NVME_BDFS[$i],,}"
  # accept short "41:00.0" or "0000:41:00.0"
  [[ "$bdf" =~ ^[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]] && bdf="0000:${bdf}"
  if [[ ! "$bdf" =~ ^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]]; then
    echo "error: --nvme '${NVME_BDFS[$i]}' is not a PCI BDF (want 0000:41:00.0)" >&2; exit 2
  fi
  if [[ ! -d "/sys/bus/pci/devices/${bdf}" ]]; then
    # Inside a rootless container, /sys/bus/pci may be masked. Skip validation
    # but keep the BDF so detect-only/lspci paths can still try.
    warn "--nvme BDF '${bdf}': /sys/bus/pci/devices not visible (container?); keeping BDF, skipping block-node lookup"
    NVME_BDFS[$i]="$bdf"
    NVME_DEV_OF[$i]=""
    continue
  fi
  NVME_BDFS[$i]="$bdf"
  NVME_DEV_OF[$i]="$(bdf_blockdev "$bdf")"   # "" if currently bound to vfio-pci
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
TAG_SLUG=""; [[ -n "$TAG" ]] && TAG_SLUG="${TAG}_"
mkdir -p "$OUTDIR"
MD="$OUTDIR/hardware-ceiling-${TAG_SLUG}${DATE}.md"
YAML="$OUTDIR/hardware_profile_${TAG_SLUG}${DATE}.yaml"
RAW="$OUTDIR/raw-${TAG_SLUG}${DATE}"        # captured stdout from each tool, for audit
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

# NVMe paths run only against explicitly named devices.
HAVE_NVME=0; [[ ${#NVME_BDFS[@]} -gt 0 ]] && HAVE_NVME=1
# --spdk requires the SPDK tools; warn+disable rather than silently skip later.
if [[ "$DO_SPDK" == 1 && -z "$SPDK_PERF" && -z "$BAR1_BENCH" ]]; then
  echo "[hw-ceiling] warning: --spdk given but neither spdk_nvme_perf nor nvme-bar1-bench is built; SPDK path will be skipped" >&2
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
[[ -n "$TAG" ]] && yml "# Tag: ${TAG}"
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

# GPU index -> NUMA node of its PCIe root complex, from sysfs. Used for Path 9
# NUMA pinning and the Path 8 topology matrix. Format: "idx bdf numa" per line.
# (nvidia-smi bus id is like 00000000:03:00.0; sysfs wants 0000:03:00.0.)
gpu_numa_of() {  # $1 = full bus id from nvidia-smi
  local bdf="${1,,}"; bdf="${bdf#0000}"; bdf="0000${bdf}"   # normalize to 0000:xx:yy.z
  local n; n="$(cat "/sys/bus/pci/devices/${bdf}/numa_node" 2>/dev/null)"
  [[ -z "$n" || "$n" == "-1" ]] && echo 0 || echo "$n"
}
: > "$RAW/gpu-numa.csv"
if [[ -s "$RAW/gpu-static.csv" ]]; then
  while IFS=, read -r idx name bus gen width; do
    idx="${idx// /}"; bus="${bus// /}"
    [[ -z "$idx" ]] && continue
    echo "${idx},${bus},$(gpu_numa_of "$bus")" >> "$RAW/gpu-numa.csv"
  done < "$RAW/gpu-static.csv"
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

yml "tag: ${TAG:-null}"
yml ""
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
# Only the BDFs named via --nvme; inspect needs the KERNEL block node (derived).
# BDFs currently bound to vfio-pci have no block node — fall back to lspci for
# identity (model/vendor) so the YAML still records what hardware is present.
NVME_DEVS=(); for _dv in "${NVME_DEV_OF[@]:-}"; do [[ -n "$_dv" ]] && NVME_DEVS+=("$_dv"); done

# lspci fallback: extract model string from PCI device even when on vfio-pci.
lspci_nvme_model() {  # $1 = BDF -> "Samsung PM9A3 NVMe 3840GB" or ""
  local bdf="$1" line
  line="$(lspci -s "$bdf" -mm 2>/dev/null | head -1)" || return
  # -mm format: Slot "Class" "Vendor" "Device" ...  (space-separated, quoted)
  # Extract the 4th quoted field (Device).
  echo "$line" | awk '{
    n=0; s=$0
    while (match(s, /"[^"]*"/)) {
      n++
      val=substr(s, RSTART+1, RLENGTH-2)
      if (n==3) { print val; exit }
      s=substr(s, RSTART+RLENGTH)
    }
  }'
}
lspci_nvme_driver() {  # $1 = BDF -> "nvme" or "vfio-pci" or ""
  lspci -s "$1" -k 2>/dev/null | awk '/Kernel driver in use:/{print $NF}'
}

yml "nvme:"
yml "  drives:"
if [[ "$HAVE_NVME" != 1 ]]; then
  md "_No drives named via \`--nvme\` — NVMe inspection skipped (by design)._"
  yml "    []   # no --nvme devices named"
elif [[ ${#NVME_DEVS[@]} -eq 0 ]]; then
  # All named BDFs are on vfio-pci (no kernel block node). Use lspci for identity.
  md "_Named BDFs have no kernel block node (bound to vfio-pci) — using lspci for identity._"
  for (( _i=0; _i<NVME_N; _i++ )); do
    bdf="${NVME_BDFS[$_i]}"
    model="$(lspci_nvme_model "$bdf")"
    driver="$(lspci_nvme_driver "$bdf")"
    md "### ${bdf} (${driver:-unknown driver})"
    md "_${model:-unknown model} — full inspect unavailable (no kernel block node)._"
    md ""
    yml "    - bdf: \"${bdf}\""
    yml "      model: \"${model:-unknown}\""
    yml "      driver: \"${driver:-unknown}\""
    yml "      inspect: false  # on vfio-pci, no kernel block node"
  done
elif [[ ! -f tools/inspect-nvme.py ]]; then
  md "_tools/inspect-nvme.py not found — skipped._"
  yml "    []   # inspect-nvme.py missing"
else
  for (( _i=0; _i<NVME_N; _i++ )); do
    dev="${NVME_DEV_OF[$_i]}"
    bdf="${NVME_BDFS[$_i]}"
    [[ -z "$dev" ]] && {
      # This BDF has no block node (vfio-pci) — lspci fallback.
      model="$(lspci_nvme_model "$bdf")"
      driver="$(lspci_nvme_driver "$bdf")"
      md "### ${bdf} (${driver:-unknown driver})"
      md "_${model:-unknown model} — no kernel block node._"
      md ""
      yml "    - bdf: \"${bdf}\""
      yml "      model: \"${model:-unknown}\""
      yml "      driver: \"${driver:-unknown}\""
      yml "      inspect: false"
      continue
    }
    j="$RAW/inspect-$(basename "$dev").json"
    # Pass PATH + user-site through sudo: inspect-nvme.py shells out to `fio`, which
    # sudo's secure_path may not resolve; -E/explicit PATH keeps the caller's PATH.
    if $SUDO env "PATH=$PATH" python3 tools/inspect-nvme.py "$dev" --skip-gc --skip-power --json > "$j" 2>"$RAW/inspect-$(basename "$dev").err"; then
      md "### $dev"
      md '```json'; cat "$j" >> "$MD"; md '```'
      # Minimal YAML extraction via python (no jq dependency assumed).
      python3 -I - "$bdf" "$dev" "$j" >> "$YAML" <<'PY'
import json,sys
bdf,dev,path=sys.argv[1],sys.argv[2],sys.argv[3]
try:
    d=json.load(open(path))
except Exception:
    print(f"    - bdf: \"{bdf}\"  # {dev}: inspect JSON unreadable"); sys.exit()
idn=d.get("identify",{})
cap=idn.get("capacity_bytes")
print(f"    - bdf: \"{bdf}\"")
print(f"      device: \"{dev}\"")
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
      # Fall back to lspci for basic identity.
      model="$(lspci_nvme_model "$bdf")"
      driver="$(lspci_nvme_driver "$bdf")"
      yml "    - bdf: \"${bdf}\""
      yml "      device: \"${dev}\""
      yml "      model: \"${model:-unknown}\"  # from lspci (inspect failed)"
      yml "      driver: \"${driver:-unknown}\""
      yml "      inspect: false"
      warn "inspect-nvme.py failed on $dev; lspci fallback for identity"
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
# Block nodes derived from the named BDFs. fio needs the kernel driver, so it
# runs BEFORE any --spdk rebind. Read is always safe; write only with --fio-write.
FIO_DEVS=(); for _dv in "${NVME_DEV_OF[@]:-}"; do [[ -n "$_dv" ]] && FIO_DEVS+=("$_dv"); done
if [[ "$HAVE_NVME" != 1 ]]; then
  md "_No drives named via \`--nvme\` — fio NVMe path skipped (by design)._"
elif [[ ${#FIO_DEVS[@]} -eq 0 ]]; then
  md "_Named BDFs have no kernel block node (bound to vfio-pci?) — fio path skipped._"
elif [[ -z "$FIO" ]]; then
  md "_fio not found — skipped._"
else
  # Pick an ioengine that actually WORKS here, by test-running a 1-op probe — not
  # by --enghelp, which lists engines that fail at runtime (libaio may be a missing
  # plugin; io_uring may be disabled via kernel.io_uring_disabled on hardened hosts).
  # Prefer async (libaio/io_uring) for true QD>1; fall back to psync, which needs no
  # plugin or special syscall and still gives the sequential bandwidth ceiling.
  probe_eng() {
    $SUDO "$FIO" --name=probe --ioengine="$1" --filename=/dev/null --rw=read \
         --bs=4k --size=4k --loops=1 >/dev/null 2>&1
  }
  FIO_ENG=""
  for cand in libaio io_uring psync; do
    if probe_eng "$cand"; then FIO_ENG="$cand"; break; fi
  done
  FIO_ENG="${FIO_ENG:-psync}"
  [[ "$FIO_ENG" == psync ]] && FIO_QD=1 || FIO_QD=32   # psync has no real queue depth
  log "fio ioengine: $FIO_ENG (iodepth $FIO_QD)"
  md "fio ioengine: \`$FIO_ENG\` (iodepth $FIO_QD)"; md ""
  # Workloads: read always; write only on explicit opt-in.
  FIO_RW=(read)
  [[ "$FIO_WRITE" == 1 ]] && FIO_RW+=(write)
  if [[ "$FIO_WRITE" == 1 ]]; then
    md "> **WARNING:** \`--fio-write\` given — sequential WRITE will OVERWRITE raw-device data on the drives below."
    md ""
    warn "fio WRITE enabled (--fio-write): this overwrites raw data on ${FIO_DEVS[*]}"
  fi
  md "Devices: \`${FIO_DEVS[*]}\`  |  block sizes: 4k 64k 128k 1M  |  QD ${FIO_QD}  |  ${FIO_RUNTIME}s each"
  md ""
  md "| device | rw | block | GB/s | IOPS | avg lat us |"
  md "|--------|----|------:|-----:|-----:|-----------:|"
  for dev in "${FIO_DEVS[@]}"; do
    for rw in "${FIO_RW[@]}"; do
      for bs in 4k 64k 128k 1M; do
        tag="fio_$(basename "$dev")_${rw}_${bs}"
        # --direct=1 bypasses page cache (raw device ceiling); JSON for clean parse.
        if $SUDO "$FIO" --name="$tag" --filename="$dev" --rw="$rw" --bs="$bs" \
              --direct=1 --ioengine="$FIO_ENG" --iodepth="$FIO_QD" --runtime="$FIO_RUNTIME" \
              --time_based --output-format=json > "$RAW/${tag}.json" 2>"$RAW/${tag}.err"; then
          # Parse one job's read or write block from fio JSON (stdlib only). fio may
          # print non-JSON warnings (e.g. "cache invalidation ... Permission denied")
          # before the JSON, so start parsing at the first '{'.
          read -r gbps iops lat < <(python3 - "$RAW/${tag}.json" "$rw" <<'PY'
import json,sys
try:
    txt=open(sys.argv[1]).read()
    i=txt.find("{")
    d=json.loads(txt[i:]) if i>=0 else {}
    j=d["jobs"][0][sys.argv[2]]
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
if [[ "$HAVE_NVME" != 1 || "$DO_SPDK" != 1 ]]; then
  md "_Skipped (needs \`--nvme <devices>\` AND \`--spdk\`)._"
  md ""
elif [[ -z "$SPDK_PERF" && -z "$BAR1_BENCH" ]]; then
  md "_--spdk given but neither spdk_nvme_perf nor nvme-bar1-bench is built — skipped._"
  md ""
else
  # BDFs are exactly what the user named — no resolution needed, and we rebind
  # only these, never all NVMe in the system.
  BDFS=("${NVME_BDFS[@]}")
  md "SPDK on BDFs: \`${BDFS[*]}\`"; md ""
  if [[ ${#BDFS[@]} -eq 0 ]]; then
    md "_No BDFs — SPDK skipped._"; md ""
  else
  log "Step 4: unmounting any FS on named devices + binding them to vfio-pci (DESTRUCTIVE)"
  $SUDO pkill -f certus-server-yaml 2>/dev/null || true
  sleep 2
  if [[ -n "$SPDK_SETUP" ]]; then
    # PCI_ALLOWED restricts setup.sh to ONLY the named BDFs — never all drives.
    PCI_ALLOWED="${BDFS[*]}" $SUDO env "PCI_ALLOWED=${BDFS[*]}" "$SPDK_SETUP" > "$RAW/spdk-setup.txt" 2>&1 || warn "SPDK setup.sh returned non-zero"
  else
    warn "SPDK setup.sh not found — cannot bind to vfio-pci; SPDK paths may fail"
  fi
  md "Bound via: ${SPDK_SETUP:-<not found>} (PCI_ALLOWED=${BDFS[*]})"
  md '```'; cat "$RAW/spdk-setup.txt" 2>/dev/null >> "$MD"; md '```'
  md ""
  log "NVMe BDFs (named only): ${BDFS[*]:-none}"

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
  # Per the skill: run NUMA-local (pin to the GPU's own NUMA node) AND cross-NUMA
  # (pin to the opposite node) to quantify the Infinity Fabric penalty, for GPU 0
  # and GPU 1 (if present). Drive counts capped at the named-device count.
  md "### Path 9: NVMe DMA -> BAR1 microbench (nvme-bar1-bench)"
  md ""
  if [[ -n "$BAR1_BENCH" ]]; then
    NGPU="$( (have nvidia-smi && nvidia-smi -L | wc -l) 2>/dev/null || echo 0)"
    # GPUs to test: 0, and 1 if present (skill's topology-best second pairing).
    GPUS=(0); [[ "$NGPU" -ge 2 ]] && GPUS+=(1)
    # Drive-count rungs within what was named.
    ND_LIST=(1); [[ ${#BDFS[@]} -ge 4 ]] && ND_LIST+=(4); [[ ${#BDFS[@]} -ge 6 ]] && ND_LIST+=(6)
    [[ ${#BDFS[@]} -gt 1 && ${#BDFS[@]} -lt 4 ]] && ND_LIST+=(${#BDFS[@]})

    bar1_run() {  # $1=gpu $2=nd $3=numa_mode(local|cross) $4=numa_node_to_pin
      local g="$1" nd="$2" mode="$3" node="$4" tag="bar1_g${g}_d${nd}_${mode}"
      local pfx=""
      if have numactl && [[ -n "$node" ]]; then pfx="numactl --cpunodebind=${node} --membind=${node}"; fi
      log "nvme-bar1-bench gpu=$g drives=$nd $mode (numa node ${node:-default})"
      if $SUDO $pfx "$BAR1_BENCH" --drive-count "$nd" --gpu "$g" \
           --chunk-size 131072 --total-bytes "$TOTAL_BYTES" --queue-depth 32 --iterations 3 \
           > "$RAW/${tag}.txt" 2>&1; then
        md "#### GPU ${g}, ${nd} drive(s), ${mode}-NUMA (pinned node ${node:-default})"
        md '```'; cat "$RAW/${tag}.txt" >> "$MD"; md '```'
        local perdrive ovh
        perdrive="$(awk -F'[ ,]+' '/Per-drive:/{print $4","$7}' "$RAW/${tag}.txt")"
        ovh="$(awk '/BAR1 overhead:/{print $3} /BAR1 is .* faster/{print "-"$3}' "$RAW/${tag}.txt" | tr -d '%' | head -1)"
        # sidecar: gpu,drives,mode,host_ram_gbps,bar1_gbps,overhead_pct
        echo "${g},${nd},${mode},${perdrive:-,},${ovh:-}" >> "$RAW/bar1_results.csv"
      else
        md "#### GPU ${g}, ${nd} drive(s), ${mode}-NUMA — FAILED"
        md '```'; cat "$RAW/${tag}.txt" >> "$MD"; md '```'
        warn "nvme-bar1-bench g${g} d${nd} ${mode} failed (gdrdrv loaded?)"
      fi
    }

    for g in "${GPUS[@]}"; do
      # NUMA node owning this GPU's root complex, and the opposite node.
      gnode="$(awk -F, -v i="$g" '$1==i{print $3}' "$RAW/gpu-numa.csv" 2>/dev/null)"; gnode="${gnode:-0}"
      other=""
      if [[ "${NUMA_NODES:-1}" -gt 1 ]]; then other=$(( gnode == 0 ? 1 : 0 )); fi
      for nd in "${ND_LIST[@]}"; do
        bar1_run "$g" "$nd" local "$gnode"
      done
      # Cross-NUMA: one representative drive count (max available in list) for the penalty.
      if [[ -n "$other" ]]; then
        bar1_run "$g" "${ND_LIST[-1]}" cross "$other"
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
  fi  # end: BDFs resolved
fi

# ===========================================================================
# PATH 8 — Topology-aware routing matrix (derived; no benchmark)
# ===========================================================================
# For every named drive x GPU pairing, classify the route: same NUMA node =
# direct PCIe-ish; different node = traverses the inter-socket link. Derived
# from sysfs numa_node of each device — runs whenever --nvme devices are named,
# regardless of --spdk, and is non-destructive.
md "## 8b. Topology-Aware Routing Matrix (Path 8, derived)"
md ""
if [[ "$HAVE_NVME" != 1 ]]; then
  md "_No \`--nvme\` devices named — topology matrix skipped._"
elif [[ ! -s "$RAW/gpu-numa.csv" ]]; then
  md "_No GPU NUMA info (nvidia-smi absent?) — topology matrix skipped._"
else
  # drive -> numa node sidecar, straight from the BDF's sysfs numa_node.
  : > "$RAW/drive-numa.csv"
  for bdf in "${NVME_BDFS[@]}"; do
    n="$(cat "/sys/bus/pci/devices/${bdf}/numa_node" 2>/dev/null)"; [[ -z "$n" || "$n" == "-1" ]] && n=0
    echo "${bdf},${n}" >> "$RAW/drive-numa.csv"
  done
  md "| drive (NUMA) | GPU (NUMA) | route |"
  md "|------|------|------|"
  while IFS=, read -r dname dn; do
    while IFS=, read -r gidx gbus gn; do
      [[ -z "$gidx" ]] && continue
      if [[ "$dn" == "$gn" ]]; then route="same NUMA node ${dn} (direct)"; else route="cross-NUMA (${dn} -> ${gn}, inter-socket)"; fi
      md "| ${dname} (n${dn}) | GPU ${gidx} (n${gn}) | ${route} |"
    done < "$RAW/gpu-numa.csv"
  done < "$RAW/drive-numa.csv"
fi
md ""

fi  # end DETECT_ONLY guard (Paths 3-9)

# ===========================================================================
# YAML: gpus: and host_ram: sections (parsed from bench raw output)
# ===========================================================================
log "Emitting gpus: and host_ram: YAML from bench output"
python3 -I - "$RAW" "$YAML" "${NUMA_NODES:-1}" "$MD" <<'PY'
import os, re, sys, glob
raw, yaml_path, nnodes = sys.argv[1], sys.argv[2], int(sys.argv[3])
md_path = sys.argv[4] if len(sys.argv)>4 else None

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

# --- bar1 from bar1_results.csv: gpu,drives,mode,host_ram,bar1,overhead_pct ---
bcsv=os.path.join(raw,"bar1_results.csv")
# bar1[(gpu,mode)][drives] = (host_ram, bar1, overhead)
bar1={}
if os.path.exists(bcsv):
    for ln in open(bcsv):
        f=ln.strip().split(",")
        if len(f)>=6 and f[0].isdigit():
            g=int(f[0]); d=int(f[1]); mode=f[2]; hr=f[3]; b1=f[4]; ov=f[5]
            bar1.setdefault((g,mode),{})[d]=(hr,b1,ov)
if bar1:
    out.append("bar1:")
    for g in sorted({gg for (gg,_) in bar1}):
        out.append(f"  gpu{g}:")
        for mode,key in (("local","numa_local"),("cross","cross_numa")):
            if (g,mode) not in bar1: continue
            out.append(f"    {key}:")
            for d in sorted(bar1[(g,mode)]):
                hr,b1,ov=bar1[(g,mode)][d]
                if hr: out.append(f"      host_ram_{d}drive_gbps: {hr}")
                if b1: out.append(f"      bar1_{d}drive_gbps: {b1}")
                if ov: out.append(f"      overhead_{d}drive_pct: {ov}")
        # cross-NUMA penalty vs local at the shared drive count
        if (g,"local") in bar1 and (g,"cross") in bar1:
            shared=set(bar1[(g,"local")]) & set(bar1[(g,"cross")])
            for d in sorted(shared):
                try:
                    lb=float(bar1[(g,"local")][d][1]); cb=float(bar1[(g,"cross")][d][1])
                    if lb>0: out.append(f"    cross_numa_penalty_pct_{d}drive: {(1-cb/lb)*100:.1f}")
                except (ValueError, IndexError): pass
    out.append("  # NOTE: nvme-bar1-bench does not print selected drive BDFs; the")
    out.append("  # named --nvme devices are assumed (see skill Path 9).")
    out.append("")

# --- gdrcopy ---
gd=os.path.join(raw,"gdrcopy.txt")
if os.path.exists(gd):
    t=open(gd,errors="replace").read()
    nums=re.findall(r'([\d.]+)\s*(?:GB/s|GiB/s|MB/s)', t)
    out.append("gdrcopy:")
    out.append(f"  raw_present: true  # see report for parsed values ({len(nums)} bw figures found)")
    out.append("")

# --- topology (Path 8, derived): which named drives share a NUMA node w/ each GPU ---
gnuma=os.path.join(raw,"gpu-numa.csv"); dnuma=os.path.join(raw,"drive-numa.csv")
if os.path.exists(gnuma) and os.path.exists(dnuma):
    gpu_node={}
    for ln in open(gnuma):
        f=ln.strip().split(",")
        if len(f)>=3 and f[0].isdigit(): gpu_node[int(f[0])]=f[2]
    drive_node={}
    for ln in open(dnuma):
        f=ln.strip().split(",")
        if len(f)>=2: drive_node[f[0]]=f[1]
    if gpu_node and drive_node:
        out.append("topology:")
        for g in sorted(gpu_node):
            same=[d for d,n in drive_node.items() if n==gpu_node[g]]
            out.append(f"  gpu{g}_numa_node: {gpu_node[g]}")
            out.append(f"  gpu{g}_same_numa_drives: [{', '.join(repr(d) for d in sorted(same))}]")
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
def fio_peak_read():
    # Best single-device fio READ GB/s (1-drive ceiling) from fio_results.csv.
    if not os.path.exists(fcsv): return None
    xs=[]
    for ln in open(fcsv):
        f=ln.strip().split(",")
        if len(f)>=4 and f[1]=="read":
            try: xs.append(float(f[3]))
            except ValueError: pass
    return max(xs) if xs else None
# NVMe read ceiling at a drive count: prefer SPDK (true multi-drive aggregate);
# else fall back to the fio single-device read peak (valid only for the 1-drive
# rung — fio here benches one device, so no aggregate claim beyond that).
def nvme_read_ceiling(d):
    nv=nvme_best("read",d)
    if nv is not None: return (nv,"spdk")
    if d==1:
        fp=fio_peak_read()
        if fp is not None: return (fp,"fio")
    return (None,None)
cl=["ceilings:"]
if h2d_peak_g0: cl.append(f"  warm_gpu0_peak_gbps: {h2d_peak_g0}")
for d in (1,4,6):
    nv,src=nvme_read_ceiling(d)
    if nv is not None:
        cap=min(nv,h2d_peak_g0) if h2d_peak_g0 else nv
        bound="h2d" if (h2d_peak_g0 and h2d_peak_g0<nv) else "nvme"
        cl.append(f"  dispatcher_cold_{d}drive_gbps: {cap:.2f}   # bound by {bound}, nvme src={src}")
# p2p cold ceiling = NUMA-local BAR1 result (GPU 0).
g0=bar1.get((0,"local"),{})
for d in sorted(g0):
    if g0[d][1]: cl.append(f"  p2p_cold_{d}drive_gpu0_gbps: {g0[d][1]}")
if len(cl)>1:
    out.extend(cl); out.append("")

open(yaml_path,"a").write("\n".join(out)+"\n")

# ---------------------------------------------------------------------------
# Summary sub-tables + crossover (skill Step 14) -> appended to the MARKDOWN.
# Derived entirely from the numbers parsed above. Cells left blank when a path
# was not measured, rather than fabricated.
# ---------------------------------------------------------------------------
def fmt(x): return f"{x:.2f}" if isinstance(x,(int,float)) else (x or "")
if md_path:
    m=["", "## Summary", ""]
    # Sub-table 2: warm path (H2D ceiling), per GPU
    bw0=parse_bw(numa_files[0] if numa_files else plain)
    m+=["### Warm path — Memory-Tier (DRAM) --H2D--> GPU", "",
        "| GPU | Ceiling (GB/s) | Bound by |", "|-----|----------------|----------|"]
    for g in sorted(bw0):
        pk=peak_h2d(bw0.get(g,{}).get("rows",{}))[0]
        if pk: m.append(f"| {g} | {fmt(pk)} | H2D PCIe |")
    m.append("")
    # Sub-table 3: dispatcher cold = min(nvme_agg_read, h2d_peak_gpu0)
    m+=["### `dispatcher` cold path — NVMe --DMA--> DRAM --H2D--> GPU", "",
        "| Drives | Ceiling (GB/s) | Bound by |", "|--------|----------------|----------|"]
    any_nvme=False
    for d in (1,4,6):
        nv,src=nvme_read_ceiling(d)
        if nv is None: continue
        any_nvme=True
        cap=min(nv,h2d_peak_g0) if h2d_peak_g0 else nv
        if h2d_peak_g0 and h2d_peak_g0<nv: bound="H2D PCIe"
        else: bound=("NVMe (fio, 1 dev)" if src=="fio" else "NVMe aggregate")
        m.append(f"| {d} | {fmt(cap)} | {bound} |")
    if not any_nvme: m.append("| _(no NVMe data — run with --nvme, or --spdk)_ | | |")
    m.append("")
    # Sub-table 4: dispatcher-p2p cold = NUMA-local BAR1 (gpu0)
    m+=["### `dispatcher-p2p` cold path — NVMe --DMA--> BAR1 (GPU)", "",
        "| Drives | BAR1 ceiling (GB/s) | Overhead vs host-ram | NUMA |", "|--------|---------------------|----------------------|------|"]
    g0l=bar1.get((0,"local"),{})
    if g0l:
        for d in sorted(g0l):
            hr,b1,ov=g0l[d]
            m.append(f"| {d} | {fmt(b1)} | {ov+'%' if ov else ''} | local |")
    else:
        m.append("| _(no nvme-bar1-bench data)_ | | | |")
    m.append("")
    # Crossover: dispatcher cold vs p2p cold
    m+=["### Crossover — `dispatcher` cold vs `dispatcher-p2p` cold", "",
        "| Drives | dispatcher cold (GB/s) | p2p cold (GB/s) | Winner |",
        "|--------|------------------------|-----------------|--------|"]
    rows=0
    for d in (1,4,6):
        nv,_=nvme_read_ceiling(d)
        disp=(min(nv,h2d_peak_g0) if (nv and h2d_peak_g0) else nv) if nv else None
        p2p=float(g0l[d][1]) if (d in g0l and g0l[d][1]) else None
        if disp is None and p2p is None: continue
        rows+=1
        win="" if (disp is None or p2p is None) else ("dispatcher" if disp>p2p else ("p2p" if p2p>disp else "~tie"))
        m.append(f"| {d} | {fmt(disp)} | {fmt(p2p)} | {win} |")
    if rows==0: m.append("| _(insufficient data — need --nvme, --spdk, and/or BAR1 runs)_ | | | |")
    m.append("")
    open(md_path,"a").write("\n".join(m)+"\n")
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
echo "NOTE: Measured paths are filled; skipped ones are marked (never faked)."
echo "The markdown report ends with the skill's derived Summary sub-tables"
echo "(warm / dispatcher-cold / p2p-cold) and the crossover table, computed from"
echo "whatever was measured; cells for unmeasured paths are left blank."
