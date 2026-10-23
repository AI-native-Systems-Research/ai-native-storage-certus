#!/usr/bin/env bash
#
# Bring up one llm-d instance: a namespace, its router, and model servers
# confined to the instance's own nodes. Several instances can share a cluster
# on disjoint node sets; each has its own site file (SITE_ENV) and namespace.
#
# This script is hardware-independent. Configuration comes in two layers:
#
#   profiles/<PROFILE>/   committed. Tuning for a GPU type -- model, memory
#                         sizing, TP, KV transport -- with no knowledge of any
#                         particular machine:
#     agg/      aggregated serving: overlay on llm-d's optimized-baseline base
#     pd/       prefill/decode split: overlay on llm-d's pd-disaggregation base
#     certus/   certus KV offload: overlay on ../agg
#     pd-certus/ P/D with certus beside NIXL: overlay on ../pd
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
#   PD=1 WITH_CERTUS=1 deploy/llm-d/install.sh   # P/D, certus beside NIXL
#   DRY_RUN=1 PD=1 deploy/llm-d/install.sh  # check settings; change nothing
#   UNINSTALL=1 deploy/llm-d/install.sh     # remove the instance, free its nodes
#
# Env:
#   SITE_ENV     private site file (default:
#                ${XDG_CONFIG_HOME:-~/.config}/certus/llm-d-site.env; optional)
#   PROFILE      profile under profiles/ (default: a30)
#   LLMD_DIR     llm-d checkout to reuse. Default: a fresh clone in a temporary
#                directory under /tmp, deleted on exit.
#   DRY_RUN      1 to stop before the first cluster write: render the
#                manifests, check the named nodes and their GPUs (read-only,
#                if a cluster is reachable), and print the plan (default: 0)
#   KEEP_LLMD    1 to keep the temporary checkout and print its path, e.g. to
#                inspect what was applied (default: 0)
#   LLMD_REF     override the pin in llm-d.ref
#   NAMESPACE    target namespace, and the instance's identity (default:
#                llm-d-quickstart). One instance per namespace: install.sh labels
#                the instance's nodes llm-d.ai/instance=<NAMESPACE>, confines its
#                pods to them, and refuses a node another instance holds.
#   UNINSTALL    1 to remove this instance instead: its model servers, its
#                router release and its node labels. The namespace is kept, since
#                it may hold secrets created by hand. Honours DRY_RUN.
#   RELEASE_NODES
#                1 to remove this instance's claim from nodes it holds but no
#                longer lists, instead of refusing (default: 0)
#   WITH_CERTUS  1 to apply profiles/<PROFILE>/certus, or with PD=1
#                profiles/<PROFILE>/pd-certus (default: 0)
#   PD           1 to apply profiles/<PROFILE>/pd instead of agg (default: 0)
#   AGG_NODES    space-separated nodes for aggregated serving (default and
#                WITH_CERTUS). Belongs in SITE_ENV. Optional if the nodes
#                already carry this instance's label.
#   AGG_REPLICAS
#                aggregated replicas, normally one per GPU of AGG_NODES.
#                Belongs in SITE_ENV.
#   PD_PREFILL_REPLICAS, PD_DECODE_REPLICAS
#                prefill / decode replicas (PD=1). Belongs in SITE_ENV.
#   PD_PREFILL_NODES, PD_DECODE_NODES
#                space-separated node names to label llm-d.ai/pd-prefill=true /
#                llm-d.ai/pd-decode=true. Belongs in SITE_ENV. Optional if this
#                instance's nodes already carry the role labels.
#   ROUTER_NODEPORT
#                expose the router's OpenAI endpoint on every node at this
#                port (30000-32767), for clients outside the cluster. Unset:
#                none, and an earlier one is removed. The router's own Service
#                stays ClusterIP; this adds <GUIDE_NAME>-epp-nodeport beside it.
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
# The namespace is the instance: everything install.sh manages lives in it, and
# the layout switch deletes Deployments by name within it, so two instances
# sharing one would delete each other's. A namespace name is always a valid
# label value, so it doubles as the node-ownership label.
INSTANCE="${NAMESPACE}"
RELEASE_NODES="${RELEASE_NODES:-0}"
WITH_CERTUS="${WITH_CERTUS:-0}"
PD="${PD:-0}"
DRY_RUN="${DRY_RUN:-0}"
KEEP_LLMD="${KEEP_LLMD:-0}"
GUIDE_NAME="${GUIDE_NAME:-quickstart}"
ROUTER_NODEPORT="${ROUTER_NODEPORT:-}"
if [ -n "${ROUTER_NODEPORT}" ] && ! { [[ "${ROUTER_NODEPORT}" =~ ^[0-9]+$ ]] \
        && [ "${ROUTER_NODEPORT}" -ge 30000 ] && [ "${ROUTER_NODEPORT}" -le 32767 ]; }; then
    echo "ERROR: ROUTER_NODEPORT='${ROUTER_NODEPORT}' is not a port in 30000-32767." >&2
    exit 1
fi
NODEPORT_SVC="${GUIDE_NAME}-epp-nodeport"

if [ -z "${LLMD_REF:-}" ]; then
    LLMD_REF="$(grep -vE '^[[:space:]]*(#|$)' "${SCRIPT_DIR}/llm-d.ref" \
                | head -1 | tr -d '[:space:]')"
fi
[ -n "${LLMD_REF}" ] || { echo "ERROR: no llm-d revision in ${SCRIPT_DIR}/llm-d.ref" >&2; exit 1; }

if ! [[ "${NAMESPACE}" =~ ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$ ]]; then
    echo "ERROR: NAMESPACE='${NAMESPACE}' is not a valid namespace name." >&2
    exit 1
fi

AGG_DEPLOYS=(deploy/optimized-baseline-nvidia-gpu-vllm-decode)
PD_DEPLOYS=(deploy/pd-disaggregation-nvidia-gpu-vllm-prefill
            deploy/pd-disaggregation-nvidia-gpu-vllm-decode)

# --- Uninstall: needs neither a profile check nor the llm-d checkout. ---
if [ "${UNINSTALL:-0}" != "0" ]; then
    held="$(kubectl get nodes -l "llm-d.ai/instance=${INSTANCE}" -o name | sed 's|node/||' | tr '\n' ' ')"
    echo "[uninstall] instance ${INSTANCE} in namespace ${NAMESPACE}; holds: ${held:-no nodes}"
    if [ "${DRY_RUN}" != "0" ]; then
        for d in "${AGG_DEPLOYS[@]}" "${PD_DEPLOYS[@]}"; do
            kubectl get "${d}" -n "${NAMESPACE}" >/dev/null 2>&1 && echo "[dry-run] would delete ${d}"
        done
        echo "[dry-run] would uninstall helm release ${GUIDE_NAME} and release: ${held:-nothing}"
        echo "[dry-run] nothing was changed."
        exit 0
    fi
    for d in "${AGG_DEPLOYS[@]}" "${PD_DEPLOYS[@]}"; do
        kubectl delete "${d}" -n "${NAMESPACE}" --ignore-not-found --wait=true
    done
    kubectl delete service "${NODEPORT_SVC}" -n "${NAMESPACE}" --ignore-not-found >/dev/null
    helm uninstall "${GUIDE_NAME}" -n "${NAMESPACE}" --wait 2>/dev/null \
        || echo "[uninstall] no helm release ${GUIDE_NAME} in ${NAMESPACE}"
    for n in ${held}; do
        kubectl label node "${n}" llm-d.ai/instance- llm-d.ai/pd-prefill- llm-d.ai/pd-decode- >/dev/null
    done
    echo "[uninstall] done; namespace ${NAMESPACE} kept (delete it by hand if unused)."
    exit 0
fi

# Check the profile has what this run needs before touching the cluster.
need_overlay() {
    [ -f "${PROFILE_DIR}/$1/kustomization.yaml" ] || {
        echo "ERROR: profile '${PROFILE}' has no $1/ overlay (${PROFILE_DIR}/$1)." >&2
        exit 1
    }
}
if [ "${PD}" != "0" ]; then need_overlay pd; else need_overlay agg; fi
if [ "${WITH_CERTUS}" != "0" ]; then
    need_overlay certus   # pd-certus takes its entrypoint from certus/
    [ "${PD}" = "0" ] || need_overlay pd-certus
fi

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

if [ "${PD}" != "0" ]; then
    GUIDE_VALUES="${REPO_ROOT}/guides/pd-disaggregation/router/pd-disaggregation.values.yaml"
    DEPLOYS=("${PD_DEPLOYS[@]}")
    OTHER_DEPLOYS=("${AGG_DEPLOYS[@]}")
else
    GUIDE_VALUES="${REPO_ROOT}/guides/optimized-baseline/router/optimized-baseline.values.yaml"
    DEPLOYS=("${AGG_DEPLOYS[@]}")
    OTHER_DEPLOYS=("${PD_DEPLOYS[@]}")
fi

# --- Node ownership and capacity (read-only). ---
# Each instance owns a set of nodes, recorded as llm-d.ai/instance=<INSTANCE>.
# Its pods select on that label, so two instances cannot land on each other's
# GPUs, and a node another instance holds is refused rather than shared. Checks
# here catch a typo'd node name, a stolen node, or more replicas than GPUs --
# all of which would otherwise surface only as pods stuck Pending. Skipped when
# no cluster is reachable, which is fine for a DRY_RUN on a laptop.
CLUSTER_OK=0
if kubectl get nodes -o name >/dev/null 2>&1; then CLUSTER_OK=1; fi
INST_LABEL="llm-d.ai/instance"

if [ "${PD}" != "0" ]; then ROLES=(prefill decode); else ROLES=(agg); fi
role_var() { echo "PD_$(echo "$1" | tr a-z A-Z)"; }   # prefill -> PD_PREFILL
nodes_var() { if [ "$1" = agg ]; then echo AGG_NODES; else echo "$(role_var "$1")_NODES"; fi; }
count_var() { if [ "$1" = agg ]; then echo AGG_REPLICAS; else echo "$(role_var "$1")_REPLICAS"; fi; }
# P/D roles also carry a role label; aggregated pods need only the instance.
role_sel() { if [ "$1" = agg ]; then echo "${INST_LABEL}=${INSTANCE}"; else echo "${INST_LABEL}=${INSTANCE},llm-d.ai/pd-$1=true"; fi; }

node_list() { kubectl get nodes -l "$1" -o name | sed 's|node/||'; }
owner_of() { kubectl get node "$1" -o jsonpath="{.metadata.labels.llm-d\.ai/instance}"; }

# Resolve each role's nodes: the site's list, or -- if it is empty -- the nodes
# that already carry this instance's labels from an earlier run.
declare -A ROLE_NODES
for role in "${ROLES[@]}"; do
    v="$(nodes_var "${role}")"
    ROLE_NODES[${role}]="${!v:-}"
    if [ -z "${ROLE_NODES[${role}]}" ] && [ "${CLUSTER_OK}" = 1 ]; then
        ROLE_NODES[${role}]="$(node_list "$(role_sel "${role}")" | tr '\n' ' ')"
    fi
    if [ -z "${ROLE_NODES[${role}]// }" ]; then
        echo "ERROR: no node to run ${role} on. Set ${v} in ${SITE_ENV}" >&2
        echo "       (see site.env.example)." >&2
        exit 1
    fi
done
# shellcheck disable=SC2086  # word-splitting the node lists is intended
WANT_NODES="$(printf '%s\n' ${ROLE_NODES[*]} | sort -u | tr '\n' ' ')"

if [ "${CLUSTER_OK}" = 1 ]; then
    # Every node exists and belongs to no other instance.
    for n in ${WANT_NODES}; do
        if ! kubectl get node "${n}" >/dev/null 2>&1; then
            echo "ERROR: node '${n}' does not exist in this cluster." >&2
            echo "       Node names: $(kubectl get nodes -o name | sed 's|node/||' | tr '\n' ' ')" >&2
            exit 1
        fi
        owner="$(owner_of "${n}")"
        if [ -n "${owner}" ] && [ "${owner}" != "${INSTANCE}" ]; then
            echo "ERROR: node '${n}' belongs to llm-d instance '${owner}'." >&2
            echo "       Remove it from that instance's site file and re-run it with" >&2
            echo "       RELEASE_NODES=1, or pick another node." >&2
            exit 1
        fi
    done
    # Nodes this instance holds but no longer lists: refuse, unless told to
    # release them. Silently keeping them would leave pods where the site file
    # says they should not be; silently dropping them could strand a running
    # layout of this instance.
    STALE_NODES=""
    for n in $(node_list "${INST_LABEL}=${INSTANCE}"); do
        case " ${WANT_NODES} " in *" ${n} "*) ;; *) STALE_NODES+="${n} " ;; esac
    done
    if [ -n "${STALE_NODES}" ] && [ "${RELEASE_NODES}" = "0" ]; then
        echo "ERROR: instance '${INSTANCE}' holds node(s) its site file no longer lists:" >&2
        echo "       ${STALE_NODES}" >&2
        echo "       Re-run with RELEASE_NODES=1 to release them, or list them again." >&2
        exit 1
    fi

    # Capacity, per role. Allocatable, not free: GPUs held by the layout this run
    # replaces are counted, since that layout is deleted first. A node in two
    # roles counts toward both, so a shared node can still over-commit; the
    # README recommends disjoint role lists for that reason.
    for role in "${ROLES[@]}"; do
        cv="$(count_var "${role}")"; want="${!cv}"; have=0
        for n in ${ROLE_NODES[${role}]}; do
            g="$(kubectl get node "${n}" -o jsonpath='{.status.allocatable.nvidia\.com/gpu}')"
            echo "[install]   ${role}: ${n} has ${g:-0} allocatable GPU(s)"
            have=$((have + ${g:-0}))
        done
        if [ "${have}" -lt "${want}" ]; then
            echo "WARNING: ${cv}=${want} but ${role} nodes have only ${have} GPU(s);" >&2
            echo "         $((want - have)) pod(s) will stay Pending." >&2
        fi
    done

    # The InferencePool CRD is cluster-wide and shared by every instance;
    # re-applying it from a different GAIE release changes it under all of them.
    have_crd="$(kubectl get crd inferencepools.inference.networking.k8s.io \
        -o jsonpath='{.metadata.annotations.inference\.networking\.k8s\.io/bundle-version}' 2>/dev/null || true)"
    others="$(kubectl get nodes -o jsonpath='{range .items[*]}{.metadata.labels.llm-d\.ai/instance}{"\n"}{end}' \
              | grep -v -x -e '' -e "${INSTANCE}" | sort -u | tr '\n' ' ' || true)"
elif [ "${DRY_RUN}" != "0" ]; then
    echo "[dry-run] no cluster reachable; skipping node, ownership and GPU checks"
else
    echo "ERROR: kubectl cannot reach a cluster." >&2
    exit 1
fi

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
    if [ "${PD}" != "0" ]; then
        # Consumes ../pd, so it sits beside it; the entrypoint is certus's own,
        # copied in because kustomize will not load a file from outside.
        install_overlay "${PD_VLLM_DIR}" pd-certus
        OVERLAY="${PD_VLLM_DIR}/pd-certus"
        cp "${PROFILE_DIR}/certus/entrypoint-certus.sh" "${OVERLAY}/"
    else
        # Consumes ../agg, so it has to sit beside it.
        install_overlay "${VLLM_DIR}" certus
        OVERLAY="${VLLM_DIR}/certus"
    fi

    # The committed overlay pins a REGISTRY/REPO placeholder rather than a real
    # registry, so resolve it here against the variables publish-image.sh pushed
    # with. Done on the materialized copy, so the sources stay site-neutral.
    certus_km="${OVERLAY}/kustomization.yaml"
    if grep -q 'REGISTRY/REPO/' "${certus_km}"; then
        if [ -z "${CERTUS_REGISTRY:-}" ] || [ -z "${CERTUS_REPO:-}" ]; then
            echo "ERROR: profiles/${PROFILE}/$(basename "${OVERLAY}")/kustomization.yaml pins the REGISTRY/REPO" >&2
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

# Wrap the profile in a generated overlay carrying what belongs to the site, not
# the GPU type: replica counts, and the nodeSelector that confines every model
# server to this instance's nodes (merged with the profile's own selector, e.g.
# the P/D role labels, not replacing it). A wrapper rather than an append, so the
# profile's kustomization stays as committed. It sits beside the overlay it
# wraps, as kustomize requires. Names are post-namePrefix.
#
# The placement annotation is a hash of which nodes hold which role. Node labels
# alone never move a running pod: re-labelling node A from prefill to decode
# leaves the old prefill pod on A, holding the GPU a new decode pod needs, with
# the Deployment reporting nothing wrong. Changing the hash changes the pod
# template, so any placement change rolls the model servers.
placement="$(for role in "${ROLES[@]}"; do
                 # shellcheck disable=SC2086
                 echo "${role}=$(printf '%s\n' ${ROLE_NODES[${role}]} | sort | tr '\n' ',')"
             done | sha256sum | cut -c1-16)"
SITE_OVERLAY="$(dirname "${OVERLAY}")/site-${INSTANCE}"
rm -rf "${SITE_OVERLAY}"
mkdir -p "${SITE_OVERLAY}"
{
    echo "# Generated by install.sh from the site settings; not a source."
    echo "apiVersion: kustomize.config.k8s.io/v1beta1"
    echo "kind: Kustomization"
    echo "resources:"
    echo "  - ../$(basename "${OVERLAY}")"
    echo "replicas:"
    if [ "${PD}" != "0" ]; then
        echo "  - {name: pd-disaggregation-nvidia-gpu-vllm-prefill, count: ${PD_PREFILL_REPLICAS}}"
        echo "  - {name: pd-disaggregation-nvidia-gpu-vllm-decode, count: ${PD_DECODE_REPLICAS}}"
    else
        echo "  - {name: optimized-baseline-nvidia-gpu-vllm-decode, count: ${AGG_REPLICAS}}"
    fi
    echo "patches:"
    echo "  - target: {kind: Deployment}"
    echo "    patch: |-"
    echo "      apiVersion: apps/v1"
    echo "      kind: Deployment"
    echo "      metadata: {name: any}"
    echo "      spec:"
    echo "        template:"
    echo "          metadata: {annotations: {llm-d.ai/placement: \"${placement}\"}}"
    echo "          spec: {nodeSelector: {${INST_LABEL}: \"${INSTANCE}\"}}"
} > "${SITE_OVERLAY}/kustomization.yaml"
OVERLAY="${SITE_OVERLAY}"
if [ "${PD}" != "0" ]; then
    echo "[install] instance ${INSTANCE}: ${PD_PREFILL_REPLICAS} prefill, ${PD_DECODE_REPLICAS} decode"
else
    echo "[install] instance ${INSTANCE}: ${AGG_REPLICAS} replica(s)"
fi

# Fail here rather than letting a malformed overlay reach the cluster. This is
# also where an upstream base restructuring shows up after a llm-d.ref bump.
RENDERED="${LLMD_DIR}/rendered.yaml"
kubectl kustomize "${OVERLAY}" > "${RENDERED}"

if [ "${DRY_RUN}" != "0" ]; then
    echo "[dry-run] plan:"
    echo "[dry-run]   profile      ${PROFILE} (${PROFILE_DIR})"
    echo "[dry-run]   site file    $([ -f "${SITE_ENV}" ] && echo "${SITE_ENV}" || echo "none (${SITE_ENV} absent)")"
    echo "[dry-run]   namespace    ${NAMESPACE}"
    echo "[dry-run]   router       $(basename "${GUIDE_VALUES}")"
    echo "[dry-run]   images       $(grep -E '^ +image:' "${RENDERED}" | awk '{print $2}' | sort -u | tr '\n' ' ')"
    for d in "${OTHER_DEPLOYS[@]}"; do
        if [ "${CLUSTER_OK}" = 1 ] && kubectl get "${d}" -n "${NAMESPACE}" >/dev/null 2>&1; then
            echo "[dry-run]   would DELETE ${d} (outage)"
        fi
    done
    echo "[dry-run]   instance     ${INSTANCE} on: ${WANT_NODES:-(unchecked)}"
    if [ "${PD}" != "0" ]; then
        echo "[dry-run]   would label  llm-d.ai/pd-prefill=true on: ${ROLE_NODES[prefill]:-(unchecked)}"
        echo "[dry-run]   would label  llm-d.ai/pd-decode=true on: ${ROLE_NODES[decode]:-(unchecked)}"
    fi
    [ -z "${STALE_NODES:-}" ] || echo "[dry-run]   would RELEASE ${STALE_NODES}"
    echo "[dry-run]   would apply  ${DEPLOYS[*]}"
    DRY_RUN_OUT="${DRY_RUN_OUT:-$(mktemp --suffix=.yaml /tmp/llm-d-dry-run.XXXXXX)}"
    cp "${RENDERED}" "${DRY_RUN_OUT}"
    echo "[dry-run] rendered manifests: ${DRY_RUN_OUT}"
    echo "[dry-run] nothing was changed."
    exit 0
fi

# --- Node placement. Done after the DRY_RUN exit because it writes to the
# cluster. Claim this instance's nodes, then set the P/D role labels on them.
# Role labels are cleared first on every node this instance holds, so a node
# moved from decode to prefill does not keep both. ---
for n in ${STALE_NODES:-}; do
    echo "[install] releasing ${n} from instance ${INSTANCE}"
    kubectl label node "${n}" "${INST_LABEL}-" llm-d.ai/pd-prefill- llm-d.ai/pd-decode- >/dev/null
done
for n in ${WANT_NODES}; do
    kubectl label node "${n}" "${INST_LABEL}=${INSTANCE}" --overwrite >/dev/null
    kubectl label node "${n}" llm-d.ai/pd-prefill- llm-d.ai/pd-decode- >/dev/null
done
if [ "${PD}" != "0" ]; then
    for role in prefill decode; do
        for n in ${ROLE_NODES[${role}]}; do
            kubectl label node "${n}" "llm-d.ai/pd-${role}=true" --overwrite >/dev/null
        done
    done
fi
echo "[install] instance ${INSTANCE} holds: ${WANT_NODES}"

# --- Gateway API Inference Extension CRDs ---
# Cluster-wide, so shared with every other instance. GAIE_VERSION=latest (llm-d's
# default) follows upstream; say so when that would change the CRD under them.
if [ -n "${others:-}" ]; then
    echo "[install] note: re-applying the shared InferencePool CRD (now ${have_crd:-unknown};"
    echo "          GAIE ${GAIE_VERSION}) also affects instance(s): ${others}"
fi
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

# --- Optional NodePort to the router, for clients outside the cluster. The
# chart's Service is fixed at ClusterIP, so this is a second Service selecting
# the same proxy pods; it follows the chart's port 80 -> proxy targetPort. ---
if [ -n "${ROUTER_NODEPORT}" ]; then
    rsvc="${GUIDE_NAME}-epp"
    sel="$(kubectl get service "${rsvc}" -n "${NAMESPACE}" -o jsonpath='{.spec.selector}')"
    tport="$(kubectl get service "${rsvc}" -n "${NAMESPACE}" \
             -o jsonpath='{.spec.ports[?(@.port==80)].targetPort}')"
    if [ -z "${sel}" ] || [ -z "${tport}" ]; then
        echo "ERROR: router Service ${rsvc} has no selector or port 80; cannot add a NodePort." >&2
        exit 1
    fi
    kubectl apply -n "${NAMESPACE}" -f - <<NODEPORT
apiVersion: v1
kind: Service
metadata:
  name: ${NODEPORT_SVC}
  labels: {app.kubernetes.io/managed-by: llm-d-install-sh}
spec:
  type: NodePort
  selector: ${sel}
  ports:
    - {name: http, port: 80, targetPort: ${tport}, nodePort: ${ROUTER_NODEPORT}, protocol: TCP}
NODEPORT
    echo "[install] router on every node at :${ROUTER_NODEPORT} (OpenAI API at http://<node>:${ROUTER_NODEPORT}/v1)"
else
    kubectl delete service "${NODEPORT_SVC}" -n "${NAMESPACE}" --ignore-not-found >/dev/null
fi

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
