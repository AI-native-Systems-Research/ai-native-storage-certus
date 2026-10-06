#!/usr/bin/env bash
#
# Render deploy/k8s/*.yaml.tpl -> *.yaml. Does NOT build or push an image.
#
# Split out of build-image-and-push.sh so that changing a deployment knob --
# which namespace, which nodes, which image, which remote-lookup group -- does
# not require rebuilding a ~30GB image to re-stamp one string.
#
# Required:
#   CERTUS_REGISTRY, CERTUS_REPO
#
# Optional:
#   CERTUS_IMAGE   image name (default: certus)
#   CERTUS_TAG     image tag  (default: the repo's short SHA -- immutable, so two
#                  deployments testing different certus builds cannot collide on
#                  a shared mutable tag like `latest`)
#   CERTUS_NAMESPACE              target namespace (default: certus)
#   CERTUS_NODE_SELECTOR_KEY      node label key   (default: certus.ai/worker)
#   CERTUS_NODE_SELECTOR_VALUE    node label value (default: true)
#   CERTUS_RL_GROUP               SEEDS the certus-config ConfigMap's rl-group
#                                 key. It is only a default: the live switch is
#                                 the ConfigMap in the cluster, which the pod
#                                 spec references rather than embeds. Empty =>
#                                 each instance forms its own single-node group,
#                                 i.e. remote-lookup off.
#
# Running two deployments side by side: give each its own namespace, its own
# node-selector value, and its own image tag, e.g.
#
#   CERTUS_NAMESPACE=certus-foo CERTUS_NODE_SELECTOR_VALUE=foo \
#   CERTUS_RL_GROUP=foo CERTUS_TAG=$(git rev-parse --short HEAD) \
#     deploy/k8s/render-manifests.sh
#
# Their node sets must be disjoint, and that is forced by hardware rather than
# convention: the NVMe drives are bound to vfio exclusively, hugepages are
# consumed per instance, and the shm mailbox paths and metrics ports are fixed.
# Two deployments on one node would collide on all four.
set -euo pipefail

if [ ! -d components ]; then
    echo "ERROR: run from the top-level source directory." >&2
    exit 1
fi

: "${CERTUS_REGISTRY:?set CERTUS_REGISTRY (registry hostname)}"
: "${CERTUS_REPO:?set CERTUS_REPO (repository path within the registry)}"

CERTUS_IMAGE="${CERTUS_IMAGE:-certus}"
if [ -z "${CERTUS_TAG:-}" ]; then
    CERTUS_TAG="$(git rev-parse --short HEAD 2>/dev/null || echo latest)"
fi
CERTUS_NAMESPACE="${CERTUS_NAMESPACE:-certus}"
CERTUS_NODE_SELECTOR_KEY="${CERTUS_NODE_SELECTOR_KEY:-certus.ai/worker}"
CERTUS_NODE_SELECTOR_VALUE="${CERTUS_NODE_SELECTOR_VALUE:-true}"
CERTUS_RL_GROUP="${CERTUS_RL_GROUP:-}"

# The ConfigMap's data must OMIT the key when no group is configured. An absent
# key is handled by the pod spec's `optional: true`, so "off" holds regardless of
# the server version; a blank key is rejected by certus-server at startup, since
# it can only come from a mistake.
if [ -n "${CERTUS_RL_GROUP}" ]; then
    CERTUS_RL_GROUP_DATA="{\"rl-group\": \"${CERTUS_RL_GROUP}\"}"
else
    CERTUS_RL_GROUP_DATA="{}"
fi

echo "[render] namespace=${CERTUS_NAMESPACE} nodes=${CERTUS_NODE_SELECTOR_KEY}=${CERTUS_NODE_SELECTOR_VALUE}"
echo "[render] image=${CERTUS_REGISTRY}/${CERTUS_REPO}/${CERTUS_IMAGE}:${CERTUS_TAG}"
if [ -n "${CERTUS_RL_GROUP}" ]; then
    echo "[render] rl-group default=\"${CERTUS_RL_GROUP}\" (remote-lookup ON)"
else
    echo "[render] rl-group default empty (remote-lookup OFF; each instance isolated)"
fi

for tpl in deploy/k8s/*.yaml.tpl; do
    out="${tpl%.tpl}"
    sed -e "s|%%CERTUS_REGISTRY%%|${CERTUS_REGISTRY}|g" \
        -e "s|%%CERTUS_REPO%%|${CERTUS_REPO}|g" \
        -e "s|%%CERTUS_IMAGE%%|${CERTUS_IMAGE}|g" \
        -e "s|%%CERTUS_TAG%%|${CERTUS_TAG}|g" \
        -e "s|%%CERTUS_NAMESPACE%%|${CERTUS_NAMESPACE}|g" \
        -e "s|%%CERTUS_NODE_SELECTOR_KEY%%|${CERTUS_NODE_SELECTOR_KEY}|g" \
        -e "s|%%CERTUS_NODE_SELECTOR_VALUE%%|${CERTUS_NODE_SELECTOR_VALUE}|g" \
        -e "s|%%CERTUS_RL_GROUP_DATA%%|${CERTUS_RL_GROUP_DATA}|g" \
        -e "s|%%CERTUS_RL_GROUP%%|${CERTUS_RL_GROUP}|g" \
        "$tpl" > "$out"
    echo "  Generated: ${out}"
done
