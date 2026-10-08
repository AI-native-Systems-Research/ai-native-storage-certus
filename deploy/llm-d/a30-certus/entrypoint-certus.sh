#!/usr/bin/env bash
# Derive this pod's certus mailbox from the NUMA domain of the GPU it was
# assigned, then exec vLLM with the offloading connector wired up.
#
# Why derive it at runtime: the NVIDIA device plugin hands out GPUs opaquely, so
# a single Deployment cannot statically say "the pod holding GPU0 uses the NUMA-0
# mailbox". The plugin does expose only the assigned GPU to the container, so
# nvidia-smi reports its real host PCI BDF (index is always 0 locally) and
# /sys/bus/pci/devices/<bdf>/numa_node gives the domain -- for example
# 0000:41:00.0 -> node 0 and 0000:a1:00.0 -> node 1.
set -euo pipefail

CERTUS_SHM_DIR="${CERTUS_SHM_DIR:-/certus-shm}"
# Matches the mailboxes the certus DaemonSets publish:
#   /dev/shm/certus-shmq-numa0   (NUMA 0)
#   /dev/shm/certus-shmq-numa1   (NUMA 1)
# set by CERTUS_SHM_PATH in deploy/k8s/certus-server-numa.yaml.tpl.
#
# Host-mode servers started by hand may publish under a different prefix
# (`certus-stress-n`, for one), so override this when pointing at those rather
# than at the DaemonSets -- the two naming
# schemes are the one thing that has to agree across the client and server
# deployments, and a mismatch fails fast below rather than silently.
CERTUS_SHM_PREFIX="${CERTUS_SHM_PREFIX:-certus-shmq-numa}"
CERTUS_SLAB_SIZE_BYTES="${CERTUS_SLAB_SIZE_BYTES:-131072}"

log() { printf '[certus-entrypoint] %s\n' "$*" >&2; }

bdf="$(nvidia-smi --query-gpu=pci.bus_id --format=csv,noheader 2>/dev/null | head -1 | tr -d '[:space:]')"
if [ -z "$bdf" ]; then
    log "FATAL: nvidia-smi reported no GPU; cannot determine the NUMA domain."
    exit 1
fi

# nvidia-smi prints an 8-digit PCI domain (00000000:A1:00.0); sysfs uses 4
# (0000:a1:00.0). Normalise via python3 rather than string-chopping, so a
# non-zero or hex domain cannot silently produce a wrong path.
numa="$(python3 - "$bdf" <<'PY'
import pathlib, sys
dom, bus, devfn = sys.argv[1].split(":")
sysfs = f"{int(dom, 16):04x}:{bus.lower()}:{devfn.lower()}"
print((pathlib.Path("/sys/bus/pci/devices") / sysfs / "numa_node").read_text().strip())
PY
)"

if [ "$numa" = "-1" ]; then
    # -1 means the kernel exposes no affinity for the device (BIOS/NUMA off).
    numa="${CERTUS_NUMA_FALLBACK:-0}"
    log "WARNING: no NUMA affinity reported for ${bdf}; falling back to node ${numa}."
fi

shm_path="${CERTUS_SHM_DIR}/${CERTUS_SHM_PREFIX}${numa}"
if [ ! -e "$shm_path" ]; then
    log "FATAL: mailbox ${shm_path} is not present."
    log "  GPU ${bdf} is on NUMA node ${numa}."
    log "  Expected certus-server for that domain to be running with"
    log "  --shm-path /dev/shm/${CERTUS_SHM_PREFIX}${numa} on this host."
    log "  Contents of ${CERTUS_SHM_DIR}:"
    ls -la "$CERTUS_SHM_DIR" >&2 2>/dev/null || true
    exit 1
fi

# Built with json.dumps so a path containing a shell metacharacter cannot break
# the argument. kv_role/kv_connector mirror the proven bench driver
# (run_multiturn_shmq_certus.py: KV_CONFIG).
kv_cfg="$(python3 - "$shm_path" "$CERTUS_SLAB_SIZE_BYTES" <<'PY'
import json, sys
print(json.dumps({
    "kv_connector": "OffloadingConnector",
    "kv_role": "kv_both",
    "kv_connector_extra_config": {
        "spec_name": "CertusShmqOffloadingSpec",
        "spec_module_path": "certus_shmq_connector.spec",
        "shm_path": sys.argv[1],
        "slab_size_bytes": int(sys.argv[2]),
    },
}, separators=(",", ":")))
PY
)"

log "GPU ${bdf} -> NUMA ${numa} -> ${shm_path}"

# --no-async-scheduling: 0.22+ auto-enables async scheduling, which breaks the
#   OffloadingConnector's per-request transfer serialization (a re-scheduled load
#   races an in-flight store -> assert -> EngineDeadError).
# --disable-hybrid-kv-cache-manager: required from 0.26; the connector assumes a
#   single uniform KV-cache group.
# Both mirror compat.py's needs_disable_* capability flags.
exec vllm serve "$@" \
    --no-async-scheduling \
    --disable-hybrid-kv-cache-manager \
    --kv-transfer-config "$kv_cfg"
