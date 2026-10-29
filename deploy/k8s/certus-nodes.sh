#!/usr/bin/env bash
#
# Claim, release and list nodes for per-developer certus deployments.
#
# A deployment selects its nodes by the VALUE of one label
# (certus.ai/worker=<owner>; see render-manifests.sh and README 4.1.3), so a node
# belongs to exactly one deployment at a time. Changing that label by hand is
# how a node is silently taken from someone else, or briefly runs two certus
# instances on drives and hugepages only one can hold. This script makes the
# handover explicit and safe:
#
#   * claim refuses a node another owner holds, unless --steal;
#   * it waits for the node's current certus pods to exit before returning, so
#     the new owner's DaemonSet never starts beside the old one's;
#   * the owner is also recorded in an annotation with who and when, so
#     `list` shows whose nodes are whose.
#
# Usage:
#   deploy/k8s/certus-nodes.sh list
#   deploy/k8s/certus-nodes.sh claim   <owner> <node>... [--steal]
#   deploy/k8s/certus-nodes.sh release <owner> <node>...
#
# <owner> is the deployment's CERTUS_NODE_SELECTOR_VALUE: a short name such as
# your user name. "true" is the shared deployment's value (namespace certus).
# Releasing returns the node to no one; give it back to the shared deployment
# with `claim true <node>`.
#
# Env:
#   CERTUS_NODE_SELECTOR_KEY   label key (default: certus.ai/worker)
#   CERTUS_NODES_WAIT_S        how long claim waits for old pods (default: 300)
set -euo pipefail

KEY="${CERTUS_NODE_SELECTOR_KEY:-certus.ai/worker}"
NOTE="certus.ai/claimed-by"
WAIT_S="${CERTUS_NODES_WAIT_S:-300}"

die() { echo "certus-nodes: $*" >&2; exit 1; }

owner_of() {          # owner_of <node> -> the label value, or empty
    kubectl get node "$1" -o "jsonpath={.metadata.labels.${KEY//./\\.}}" 2>/dev/null
}

valid_owner() {       # a label value: <=63 chars, alnum at both ends, [-_.] inside
    [[ "$1" =~ ^[A-Za-z0-9]([-A-Za-z0-9_.]{0,61}[A-Za-z0-9])?$ ]]
}

certus_pods_on() {    # certus-server pods (any namespace) still on <node>
    kubectl get pods -A -l app=certus-server \
        --field-selector "spec.nodeName=$1" -o name 2>/dev/null
}

wait_drained() {      # wait_drained <node>: no certus-server pod left on it
    local node="$1" deadline=$((SECONDS + WAIT_S))
    while [ -n "$(certus_pods_on "$node")" ]; do
        if [ "$SECONDS" -ge "$deadline" ]; then
            certus_pods_on "$node" >&2
            die "certus pods still on $node after ${WAIT_S}s; not claiming"
        fi
        sleep 2
    done
}

cmd_list() {
    kubectl get nodes -L "$KEY" \
        -o custom-columns="NODE:.metadata.name,STATUS:.status.conditions[?(@.type=='Ready')].status,OWNER:.metadata.labels.${KEY//./\\.},CLAIMED:.metadata.annotations.${NOTE//./\\.}"
}

cmd_claim() {
    local owner="$1"; shift
    valid_owner "$owner" || die "'$owner' is not a valid label value"
    local steal=false nodes=() n
    for n in "$@"; do
        case "$n" in --steal) steal=true ;; *) nodes+=("$n") ;; esac
    done
    [ "${#nodes[@]}" -gt 0 ] || die "claim: name at least one node"
    # Check every node before touching any, so a refusal changes nothing.
    for n in "${nodes[@]}"; do
        kubectl get node "$n" >/dev/null || die "no node $n"
        local cur; cur="$(owner_of "$n")"
        if [ -n "$cur" ] && [ "$cur" != "$owner" ] && [ "$steal" = false ]; then
            die "$n belongs to '$cur'. Ask them, or pass --steal."
        fi
    done
    for n in "${nodes[@]}"; do
        if [ "$(owner_of "$n")" = "$owner" ]; then
            echo "$n: already $owner's"
            continue
        fi
        # Unlabel first: the old owner's DaemonSet stops matching and its pods
        # exit, and only then does the new owner's start, so the drives,
        # hugepages, mailboxes and ports are never held by two instances.
        kubectl label node "$n" "${KEY}-" >/dev/null 2>&1 || true
        echo "$n: waiting for the previous certus pods to exit"
        wait_drained "$n"
        kubectl label node "$n" "${KEY}=${owner}" --overwrite >/dev/null
        kubectl annotate node "$n" \
            "${NOTE}=${owner} by ${USER:-unknown} at $(date -u +%Y-%m-%dT%H:%M:%SZ)" \
            --overwrite >/dev/null
        echo "$n: now $owner's"
    done
}

cmd_release() {
    local owner="$1"; shift
    [ "$#" -gt 0 ] || die "release: name at least one node"
    local n
    for n in "$@"; do
        local cur; cur="$(owner_of "$n")"
        [ "$cur" = "$owner" ] || die "$n is '${cur:-unowned}', not '$owner'; not releasing"
    done
    for n in "$@"; do
        kubectl label node "$n" "${KEY}-" >/dev/null
        kubectl annotate node "$n" "${NOTE}-" >/dev/null 2>&1 || true
        echo "$n: released (waiting for $owner's certus pods to exit)"
        wait_drained "$n"
    done
}

case "${1:-}" in
    list)    cmd_list ;;
    claim)   [ "$#" -ge 3 ] || die "usage: claim <owner> <node>... [--steal]"; shift; cmd_claim "$@" ;;
    release) [ "$#" -ge 3 ] || die "usage: release <owner> <node>..."; shift; cmd_release "$@" ;;
    *)       sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
