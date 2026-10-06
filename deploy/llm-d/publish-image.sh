#!/usr/bin/env bash
#
# Build and publish the vLLM + certus-connector image that llm-d's modelserver
# pods run.
#
# This deliberately reuses certus-shmq-connector/Dockerfile rather than carrying
# a second one. That image is already proven by the benchmark flow and has the
# offline-safe pip incantation (--no-build-isolation --no-index --no-deps):
# without --no-deps, pip re-resolves torch's pins and fails on nvidia-nccl-cu13
# for CUDA-13 bases, i.e. vLLM >= 0.27. It is a benchmark-driver image, but
# llm-d's pod sets `command`/`args`, which override its ENTRYPOINT, so for
# serving the only cost is ~25MB of baked datasets and some inert env vars.
#
# WHERE TO RUN THIS: a host with ~31GB free in the engine's storage AND enough
# headroom to stay above kubelet's eviction floor. In practice that means a node
# that is not already serving: a GPU node holding the model cache and the running
# decode pods can sit at or below its floor, and pulling 30GB there evicts those
# pods mid-pull. The guard below computes the margin for whatever host it runs on
# and refuses if it does not fit; override with ALLOW_TIGHT_DISK=1 if you know
# better.
#
# Pulls on the GPU nodes are cheap: the base layers are already in their
# containerd stores, so only the connector layer transfers -- PROVIDED the base
# tag still resolves to the index they have. Verify with:
#   docker buildx imagetools inspect docker.io/vllm/vllm-openai:v${VLLM_VERSION}
#
# Usage:
#   deploy/llm-d/publish-image.sh [--no-push] [--tag <tag>]
#
# Required: CERTUS_REGISTRY, CERTUS_REPO  (same vars as deploy/k8s)
# Optional: CERTUS_IMAGE (default llm-d-vllm-certus), CERTUS_TAG,
#           VLLM_VERSION (default 0.30.0), ENGINE (docker|podman), ALLOW_TIGHT_DISK
set -euo pipefail

: "${CERTUS_REGISTRY:?set CERTUS_REGISTRY}"
: "${CERTUS_REPO:?set CERTUS_REPO}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

CERTUS_IMAGE="${CERTUS_IMAGE:-llm-d-vllm-certus}"
VLLM_VERSION="${VLLM_VERSION:-0.30.0}"
DOCKERFILE="${REPO_ROOT}/certus-shmq-connector/Dockerfile"
# Extracted size of the vLLM base; used only for the disk guard below.
NEED_GIB="${NEED_GIB:-31}"

[ -f "${DOCKERFILE}" ] || { echo "ERROR: ${DOCKERFILE} not found" >&2; exit 1; }

# --- engine selection: docker where the socket is reachable, else podman ---
# Group membership is per-login, so a session that predates a usermod will not
# have it; a fresh login (or ssh) will.
if [ -z "${ENGINE:-}" ]; then
    if docker info >/dev/null 2>&1; then ENGINE=docker
    elif podman info >/dev/null 2>&1; then ENGINE=podman
    else
        echo "ERROR: neither docker nor podman is usable here." >&2
        echo "       docker needs the 'docker' group (per-login: re-login after usermod)." >&2
        exit 1
    fi
fi

# --- disk guard: do not evict the cluster to build an image ---
# kubelet evicts on nodefs.available<10% / imagefs.available<15%. Building here
# must leave the engine's filesystem above its floor.
root_dir="$(${ENGINE} info --format '{{.DockerRootDir}}' 2>/dev/null \
            || ${ENGINE} info --format '{{.Store.GraphRoot}}' 2>/dev/null || echo /var/lib)"
guard_out="$(python3 - "$root_dir" "$NEED_GIB" <<'PY'
import os, shutil, sys
path = sys.argv[1]
need = float(sys.argv[2]) * 2**30
while not os.path.exists(path) and path != "/":
    path = os.path.dirname(path)
t, _, free = shutil.disk_usage(path)
floor = 0.10 * t          # nodefs floor; imagefs would be 15%
print(f"{path}|{free/2**30:.1f}|{floor/2**30:.1f}|{(free-need)/2**30:.1f}|"
      f"{'OK' if free - need > floor else 'TIGHT'}")
PY
)"
IFS='|' read -r g_path g_free g_floor g_after g_verdict <<<"$guard_out"
echo "[publish] engine=${ENGINE} store=${g_path} free=${g_free}G floor=${g_floor}G after=${g_after}G"
if [ "${g_verdict}" = "TIGHT" ] && [ "${ALLOW_TIGHT_DISK:-0}" = "0" ]; then
    echo "ERROR: building here would leave ${g_after}G, under the ${g_floor}G eviction floor." >&2
    echo "       kubelet would start evicting pods on this node. Build on a roomier host" >&2
    echo "       that is not running the decode pods, or set ALLOW_TIGHT_DISK=1." >&2
    exit 1
fi

if [ -z "${CERTUS_TAG:-}" ]; then
    _sha="$(cd "${REPO_ROOT}" && git rev-parse --short HEAD 2>/dev/null || echo nogit)"
    CERTUS_TAG="vllm${VLLM_VERSION}-${_sha}"
fi

NO_PUSH=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-push) NO_PUSH=true; shift ;;
        --tag)     CERTUS_TAG="$2"; shift 2 ;;
        *)         echo "Unknown arg: $1" >&2; exit 1 ;;
    esac
done

FULL_IMAGE="${CERTUS_REGISTRY}/${CERTUS_REPO}/${CERTUS_IMAGE}:${CERTUS_TAG}"

echo "[publish] building ${FULL_IMAGE}"
echo "[publish]   dockerfile: ${DOCKERFILE}"
echo "[publish]   context:    ${REPO_ROOT}"
"${ENGINE}" build \
    -f "${DOCKERFILE}" \
    --build-arg "VLLM_VERSION=${VLLM_VERSION}" \
    -t "${FULL_IMAGE}" \
    "${REPO_ROOT}"

if [ "${NO_PUSH}" = false ]; then
    echo "[publish] pushing ${FULL_IMAGE}"
    "${ENGINE}" push "${FULL_IMAGE}"
else
    echo "[publish] skipping push (--no-push)"
fi

echo
echo "Point the overlay at this build:"
echo "  (cd ${SCRIPT_DIR}/a30-certus && kustomize edit set image \\"
echo "     docker.io/vllm/vllm-openai=${FULL_IMAGE})"
