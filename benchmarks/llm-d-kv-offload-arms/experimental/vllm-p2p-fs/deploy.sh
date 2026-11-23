#!/usr/bin/env bash
# Experimental arm 5 (P2P + fs tier, events as certus's): deploy and check.
# Certus is released and the drives are on RAID 0 + XFS at /mnt/kv-tier/numa<N>.
set -u
source "$(dirname "${BASH_SOURCE[0]}")/../../env.sh"
LOG=${LOG:-$OUT_ROOT/arm5fs.log}
# A scratch checkout with install.sh.patch applied and profiles/* copied into
# deploy/llm-d/profiles/a30/ (see README.md). Never the main checkout.
REPO=${ARM5_REPO:?set ARM5_REPO to a scratch checkout with the experiment applied}
SIDECAR_SHA=ddabbfd0a4f50983260e88949d67de67d15adcd7
source <(sed -n '/^log(){/p;/^die(){/p;/^stop_model_servers(){/,/^}/p;/^pin_pd(){/,/^}/p;/^record_pods(){/,/^}/p' $ARMS_DIR/run_sequence.sh)
log "=== arm 5 + fs (experiment): deploy"
[ "$(kubectl -n certus get pods --no-headers 2>/dev/null | wc -l)" = 0 ] || die "certus is still running"
for h in node2 node5; do
  for n in 0 1; do ssh $h "mountpoint -q /mnt/kv-tier/numa$n" || die "$h: /mnt/kv-tier/numa$n not mounted"; done
done
# Each engine's start command empties its tier (as root) before vLLM starts.
log "disk tiers mounted on node2 and node5"
stop_model_servers
( cd $REPO && ARM5_FS=${ARM5_FS:-1} PD=1 OFFLOAD=1 RDMA=1 PD_DECODE_REPLICAS=1 deploy/llm-d/install.sh ) > $OUT_ROOT/install-${ARM5_FS:-1}.log 2>&1 || die "install (see install.log)"
stop_model_servers
pin_pd
record_pods
for p in $(kubectl -n $NS get pods -o name | grep -E "vllm-(decode|prefill)"); do
  l=$(kubectl -n $NS logs $p -c modelserver 2>/dev/null)
  grep -q "\[arm5-fs\] KV events as certus" <<<"$l" || die "${p#pod/}: event filter not loaded"
  grep -q "\[arm5-fs\] GPU .* fs tier" <<<"$l" || die "${p#pod/}: no fs tier"
  log "  ${p#pod/}: $(grep -m1 -o '\[arm5-fs\] GPU .*' <<<"$l" | cut -c1-120)"
done
for p in $(kubectl -n $NS get pods -o name | grep decode); do
  kubectl -n $NS logs $p -c routing-proxy 2>/dev/null | grep -q "$SIDECAR_SHA" || die "${p#pod/}: sidecar is not $SIDECAR_SHA"
done
$REPO/deploy/llm-d/router-check.sh >>$LOG 2>&1 || die "router failing open"
if [ "${ARM5_FS:-1}" = probe ]; then
  for p in $(kubectl -n $NS get pods -o name | grep -E "vllm-(decode|prefill)"); do
    kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -q "^DEBUG .*P2P" || log "  ${p#pod/}: no P2P debug lines yet"
  done
fi
log "router check ok; sidecars on $SIDECAR_SHA; DEPLOY DONE (ARM5_FS=${ARM5_FS:-1})"
