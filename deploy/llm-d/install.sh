#!/usr/bin/env bash
#
# Bring llm-d up in the llm-d-quickstart namespace, using one site's overlays.
#
# This script is hardware-independent. Configuration comes in two layers:
#
#   profiles/<PROFILE>/   committed. Tuning for a GPU type -- model, memory
#                         sizing, TP, KV transport -- with no knowledge of any
#                         particular machine:
#     agg/      aggregated serving: overlay on llm-d's optimized-baseline base
#     pd/       prefill/decode split: overlay on llm-d's pd-disaggregation base
#     certus/   certus KV offload: overlay on ../agg
#   A profile needs only the overlays it uses.
#
#   SITE_ENV              NOT committed. The one place that names real
#                         infrastructure: node names, image registry, which
#                         profile this cluster uses. A shell fragment of
#                         variable defaults; see site.env.example. Kept outside
#                         the repo by default so it cannot be committed by
#                         accident.
#
# The llm-d checkout is a disposable build area pinned to llm-d.ref. The overlays
# are copied into it because they consume llm-d's bases (and each other) by
# relative path, which kustomize will not resolve outside its own root.
#
# Usage:
#   deploy/llm-d/install.sh                 # aggregated serving
#   WITH_CERTUS=1 deploy/llm-d/install.sh   # + certus KV-cache offload
#   PD=1 deploy/llm-d/install.sh            # prefill/decode disaggregation
#
# Env:
#   SITE_ENV     private site file (default:
#                ${XDG_CONFIG_HOME:-~/.config}/certus/llm-d-site.env; optional)
#   PROFILE      profile under profiles/ (default: a30)
#   LLMD_DIR     llm-d checkout to reuse. Default: a fresh clone in a temporary
#                directory under /tmp, deleted on exit.
#   KEEP_LLMD    1 to keep the temporary checkout and print its path, e.g. to
#                inspect what was applied (default: 0)
#   LLMD_REF     override the pin in llm-d.ref
#   NAMESPACE    target namespace (default: llm-d-quickstart)
#   WITH_CERTUS  1 to apply profiles/<PROFILE>/certus (default: 0)
#   PD           1 to apply profiles/<PROFILE>/pd instead of agg (default: 0).
#                Not combinable with WITH_CERTUS.
#   AGG_REPLICAS
#                aggregated replicas, normally one per GPU. Belongs in SITE_ENV.
#   PD_PREFILL_REPLICAS, PD_DECODE_REPLICAS
#                prefill / decode replicas (PD=1). Belongs in SITE_ENV.
#   PD_PREFILL_NODES, PD_DECODE_NODES
#                space-separated node names to label llm-d.ai/pd-prefill=true /
#                llm-d.ai/pd-decode=true. Belongs in SITE_ENV. Optional if the
#                labels already exist; labels on nodes not listed are left
#                alone.
#   CERTUS_REGISTRY, CERTUS_REPO
#                where publish-image.sh pushed the connector image
#                (WITH_CERTUS). Belongs in SITE_ENV.
#
# Deliberately absent: the llm-d-hf-token secret. The a30 profile serves the
# ungated Qwen/Qwen3-8B and its overlays delete the HF_TOKEN env entry. vLLM
# warns about "unauthenticated requests to the HF Hub"; that costs download
# rate limit, not function. A site serving a gated model must create the secret
# and keep the entry.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The site file supplies defaults only (`: "${VAR:=...}"`), so a variable set by
# the caller still wins. Absent is fine: everything it sets can come from the
# environment instead, and the checks below say what is missing.
SITE_ENV="${SITE_ENV:-${XDG_CONFIG_HOME:-${HOME}/.config}/certus/llm-d-site.env}"
if [ -f "${SITE_ENV}" ]; then
    # shellcheck disable=SC1090
    source "${SITE_ENV}"
    echo "[install] site settings from ${SITE_ENV}"
fi

PROFILE="${PROFILE:-a30}"
PROFILE_DIR="${SCRIPT_DIR}/profiles/${PROFILE}"
[ -d "${PROFILE_DIR}" ] || {
    echo "ERROR: no profile '${PROFILE}' under ${SCRIPT_DIR}/profiles/" >&2
    echo "       Available: $(ls "${SCRIPT_DIR}/profiles" | tr '\n' ' ')" >&2
    exit 1
}

NAMESPACE="${NAMESPACE:-llm-d-quickstart}"
WITH_CERTUS="${WITH_CERTUS:-0}"
PD="${PD:-0}"
KEEP_LLMD="${KEEP_LLMD:-0}"
GUIDE_NAME="${GUIDE_NAME:-quickstart}"

if [ -z "${LLMD_REF:-}" ]; then
    LLMD_REF="$(grep -vE '^[[:space:]]*(#|$)' "${SCRIPT_DIR}/llm-d.ref" \
                | head -1 | tr -d '[:space:]')"
fi
[ -n "${LLMD_REF}" ] || { echo "ERROR: no llm-d revision in ${SCRIPT_DIR}/llm-d.ref" >&2; exit 1; }

if [ "${PD}" != "0" ] && [ "${WITH_CERTUS}" != "0" ]; then
    # Both want --kv-transfer-config. Combining them means wrapping NIXL and the
    # certus offload connector in vLLM's MultiConnector, which is not done yet.
    echo "ERROR: PD=1 and WITH_CERTUS=1 cannot be combined yet." >&2
    exit 1
fi

# Check the profile has what this run needs before touching the cluster.
need_overlay() {
    [ -f "${PROFILE_DIR}/$1/kustomization.yaml" ] || {
        echo "ERROR: profile '${PROFILE}' has no $1/ overlay (${PROFILE_DIR}/$1)." >&2
        exit 1
    }
}
if [ "${PD}" != "0" ]; then need_overlay pd; else need_overlay agg; fi
[ "${WITH_CERTUS}" = "0" ] || need_overlay certus

# Replica counts describe the cluster, not the GPU type, so they come from the
# site file rather than the committed profiles. Required rather than defaulted:
# a silent default of 1 would leave GPUs idle with nothing to notice.
need_count() {
    case "${!1:-}" in
        ''|*[!0-9]*|0)
            echo "ERROR: $1 must be a positive integer (got '${!1:-}')." >&2
            echo "       Set it in ${SITE_ENV} (see site.env.example) or the environment." >&2
            exit 1 ;;
    esac
}
if [ "${PD}" != "0" ]; then
    need_count PD_PREFILL_REPLICAS
    need_count PD_DECODE_REPLICAS
else
    need_count AGG_REPLICAS
fi

# --- llm-d checkout, pinned ---
# Default is a throwaway clone: the checkout holds nothing but llm-d at
# LLMD_REF plus the overlays copied in below, so there is no reason to keep it
# between runs, and a fresh one cannot carry a stale hand-edit. A blobless clone
# of one ref is ~40MB and a few seconds.
if [ -z "${LLMD_DIR:-}" ]; then
    LLMD_DIR="$(mktemp -d /tmp/llm-d.XXXXXX)"
    if [ "${KEEP_LLMD}" = "0" ]; then
        trap 'rm -rf "${LLMD_DIR}"' EXIT
    else
        trap 'echo "[install] kept llm-d checkout: ${LLMD_DIR}"' EXIT
    fi
fi
[ -d "${LLMD_DIR}/.git" ] || git clone --quiet --filter=blob:none --no-checkout \
    https://github.com/llm-d/llm-d.git "${LLMD_DIR}"
git -C "${LLMD_DIR}" fetch --quiet origin
# Detached on purpose: this checkout is build output, not somewhere to commit.
# Fetch all refs rather than the bare sha -- servers need not allow sha fetches.
if ! git -C "${LLMD_DIR}" checkout --quiet --detach "${LLMD_REF}"; then
    echo "ERROR: cannot check out llm-d ${LLMD_REF} in ${LLMD_DIR}." >&2
    echo "       Local modifications to tracked files will block this; stash them." >&2
    exit 1
fi
echo "[install] profile ${PROFILE}; llm-d pinned at $(git -C "${LLMD_DIR}" rev-parse --short HEAD)"

cd "${LLMD_DIR}"
export REPO_ROOT="$(realpath "$(git rev-parse --show-toplevel)")"
# shellcheck disable=SC1091
source "${REPO_ROOT}/guides/env.sh"
export NAMESPACE GUIDE_NAME

VLLM_DIR="${REPO_ROOT}/guides/optimized-baseline/modelserver/gpu/vllm"
PD_VLLM_DIR="${REPO_ROOT}/guides/pd-disaggregation/modelserver/gpu/vllm"
AGG_DEPLOYS=(deploy/optimized-baseline-nvidia-gpu-vllm-decode)
PD_DEPLOYS=(deploy/pd-disaggregation-nvidia-gpu-vllm-prefill
            deploy/pd-disaggregation-nvidia-gpu-vllm-decode)

if [ "${PD}" != "0" ]; then
    GUIDE_VALUES="${REPO_ROOT}/guides/pd-disaggregation/router/pd-disaggregation.values.yaml"
    DEPLOYS=("${PD_DEPLOYS[@]}")
    OTHER_DEPLOYS=("${AGG_DEPLOYS[@]}")
else
    GUIDE_VALUES="${REPO_ROOT}/guides/optimized-baseline/router/optimized-baseline.values.yaml"
    DEPLOYS=("${AGG_DEPLOYS[@]}")
    OTHER_DEPLOYS=("${PD_DEPLOYS[@]}")
fi

# --- P/D placement. The overlay selects on node labels, not node names, so it
# stays site-neutral; the site's choice of nodes is made here. Check before
# touching the cluster: with no labelled node the pods would sit Pending. ---
if [ "${PD}" != "0" ]; then
    label_role() {            # label_role <role> <nodes...>
        local role="$1"; shift
        local n
        for n in "$@"; do
            kubectl label node "${n}" "llm-d.ai/pd-${role}=true" --overwrite
        done
        if [ -z "$(kubectl get nodes -l "llm-d.ai/pd-${role}=true" -o name)" ]; then
            echo "ERROR: no node labelled llm-d.ai/pd-${role}=true." >&2
            echo "       Set PD_$(echo "${role}" | tr a-z A-Z)_NODES (in ${SITE_ENV}) to the" >&2
            echo "       nodes that should run ${role}." >&2
            exit 1
        fi
    }
    # shellcheck disable=SC2086  # word-splitting the node lists is intended
    label_role prefill ${PD_PREFILL_NODES:-}
    # shellcheck disable=SC2086
    label_role decode ${PD_DECODE_NODES:-}
fi

# --- Gateway API Inference Extension CRDs ---
kubectl apply -f \
  "https://github.com/kubernetes-sigs/gateway-api-inference-extension/${GAIE_URL}/v1-manifests.yaml"

kubectl create namespace "${NAMESPACE}" --dry-run=client -o yaml | kubectl apply -f -

# --- Router / EPP. `upgrade --install` so a re-run updates in place instead of
# failing with "cannot re-use a name that is still in use". ---
helm upgrade --install "${GUIDE_NAME}" \
  "${ROUTER_STANDALONE_CHART}" \
  -f "${REPO_ROOT}/guides/recipes/router/base.values.yaml" \
  -f "${GUIDE_VALUES}" \
  -n "${NAMESPACE}" \
  --version "${ROUTER_CHART_VERSION}"

# --- Materialize the profile's overlays into the pinned checkout. ---
# Each lands beside the llm-d base it consumes, under the same name it has in
# profiles/<PROFILE>/, because certus refers to ../agg. The directory must not be one
# llm-d itself ships -- replacing it would silently swap upstream's overlay for
# ours -- so refuse rather than overwrite. Never hand-edit the copies: change
# the sources in profiles/ instead.
install_overlay() {           # install_overlay <dest-parent> <name>
    local dest="$1/$2"
    if [ -n "$(git -C "${REPO_ROOT}" ls-files -- "${dest}" | head -1)" ]; then
        echo "ERROR: ${dest} is part of llm-d itself at ${LLMD_REF};" >&2
        echo "       rename profiles/${PROFILE}/$2 so it does not collide." >&2
        exit 1
    fi
    rm -rf "${dest}"
    cp -r "${PROFILE_DIR}/$2" "${dest}"
}

if [ "${PD}" != "0" ]; then
    install_overlay "${PD_VLLM_DIR}" pd
    OVERLAY="${PD_VLLM_DIR}/pd"
    echo "[install] prefill/decode disaggregation ENABLED"
else
    install_overlay "${VLLM_DIR}" agg
    OVERLAY="${VLLM_DIR}/agg"
fi

if [ "${WITH_CERTUS}" != "0" ]; then
    # Consumes ../agg, so it has to sit beside it.
    install_overlay "${VLLM_DIR}" certus
    OVERLAY="${VLLM_DIR}/certus"

    # The committed overlay pins a REGISTRY/REPO placeholder rather than a real
    # registry, so resolve it here against the variables publish-image.sh pushed
    # with. Done on the materialized copy, so the sources stay site-neutral.
    certus_km="${OVERLAY}/kustomization.yaml"
    if grep -q 'REGISTRY/REPO/' "${certus_km}"; then
        if [ -z "${CERTUS_REGISTRY:-}" ] || [ -z "${CERTUS_REPO:-}" ]; then
            echo "ERROR: profiles/${PROFILE}/certus/kustomization.yaml pins the REGISTRY/REPO" >&2
            echo "       placeholder, so there is no image to pull. Set CERTUS_REGISTRY and" >&2
            echo "       CERTUS_REPO (the values publish-image.sh pushed with) in" >&2
            echo "       ${SITE_ENV} or the environment." >&2
            exit 1
        fi
        sed -i "s|REGISTRY/REPO/|${CERTUS_REGISTRY}/${CERTUS_REPO}/|" "${certus_km}"
        echo "[install] image registry -> ${CERTUS_REGISTRY}/${CERTUS_REPO}"
    fi
    echo "[install] certus KV-cache offload ENABLED"
elif [ "${PD}" = "0" ]; then
    echo "[install] aggregated serving (WITH_CERTUS=1 for certus offload, PD=1 for P/D)"
fi

# Stamp the site's replica counts onto the top overlay, so they take effect
# whichever overlay is applied. Names are post-namePrefix.
{
    echo
    echo "# Added by install.sh from the site settings."
    echo "replicas:"
    if [ "${PD}" != "0" ]; then
        echo "  - {name: pd-disaggregation-nvidia-gpu-vllm-prefill, count: ${PD_PREFILL_REPLICAS}}"
        echo "  - {name: pd-disaggregation-nvidia-gpu-vllm-decode, count: ${PD_DECODE_REPLICAS}}"
        echo "[install] replicas: ${PD_PREFILL_REPLICAS} prefill, ${PD_DECODE_REPLICAS} decode" >&2
    else
        echo "  - {name: optimized-baseline-nvidia-gpu-vllm-decode, count: ${AGG_REPLICAS}}"
        echo "[install] replicas: ${AGG_REPLICAS}" >&2
    fi
} >> "${OVERLAY}/kustomization.yaml"

# Fail here rather than letting a malformed overlay reach the cluster. This is
# also where an upstream base restructuring shows up after a llm-d.ref bump.
kubectl kustomize "${OVERLAY}" >/dev/null

# The aggregated and P/D layouts each size themselves to every GPU, so whichever
# is not being deployed has to give its GPUs back first or the new pods sit
# Pending. This is an outage for the duration of the switch.
for d in "${OTHER_DEPLOYS[@]}"; do
    if kubectl get "${d}" -n "${NAMESPACE}" >/dev/null 2>&1; then
        echo "[install] removing ${d} to free its GPUs"
        kubectl delete "${d}" -n "${NAMESPACE}" --wait=true
    fi
done

kubectl apply -k "${OVERLAY}" -n "${NAMESPACE}"

# First start pulls the weights into the profile's shared model-cache hostPath
# and compiles CUDA graphs; the startup probe allows 60 min (120 x 30s). Match
# it, or rollout status reports a false failure.
for d in "${DEPLOYS[@]}"; do
    kubectl rollout status "${d}" -n "${NAMESPACE}" --timeout=60m
done
