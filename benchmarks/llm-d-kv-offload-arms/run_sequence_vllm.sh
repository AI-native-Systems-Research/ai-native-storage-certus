#!/usr/bin/env bash
# Overflow comparison, arms 1-3 (certus): new images, 8 GiB memory tier per
# instance, 64 sessions x 8 turns in interleaved order. Each arm is deployed,
# checked (images, KV size, tier size, connector) and warmed before it runs.
# Order 1 2 3, then repeats 3 2 1 if REPEAT=1, so drift cannot favour one arm.
set -u
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
LOG=${LOG:-$OUT_ROOT/run_sequence_vllm.log}
NODES="node2 node5"   # ssh names
CERTUS_IMG=certus:04548e47
VLLM_IMG=llm-d-vllm-certus:vllm0.30.0-04548e47
log(){ echo "$(date -u +%H:%M:%S) $*" | tee -a $LOG; }
die(){ log "FATAL: $*"; exit 1; }

check_certus(){
  local n=0 bad=0
  for p in $(kubectl -n certus get pods -o name); do
    img=$(kubectl -n certus get $p -o jsonpath='{.spec.containers[0].image}')
    [[ $img == *"$CERTUS_IMG" ]] || { log "  $p runs $img"; bad=1; }
    kubectl -n certus logs $p 2>/dev/null | grep -q "transfer endpoint: listening" && n=$((n+1))
    kubectl -n certus logs $p 2>/dev/null | grep -m1 -oE "memory-tier-size=[0-9]+ MiB" | sed "s|^|  ${p#pod/}: |" | tee -a $LOG
  done
  [ $bad = 0 ] && [ $n -ge 4 ]
}
wait_certus(){
  for i in $(seq 1 60); do check_certus >/dev/null 2>&1 && { check_certus; log "certus: 4 instances on $CERTUS_IMG, endpoints up"; return 0; }; sleep 10; done
  die "certus not up on $CERTUS_IMG after 10 min"
}
check_vllm(){  # every model server on the new image, GPU KV 37,184 tokens
  for p in $(kubectl -n $NS get pods -o name | grep -E "prefill|decode"); do
    img=$(kubectl -n $NS get $p -o jsonpath='{.spec.containers[?(@.name=="modelserver")].image}')
    [[ $img == *"$VLLM_IMG" ]] || die "${p#pod/} runs $img"
    kv=$(kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -m1 -oE "GPU KV cache size: [0-9,]+ tokens")
    log "  ${p#pod/}: $kv"
    [[ $kv == *"37,184"* ]] || die "${p#pod/}: unexpected GPU KV ($kv)"
  done
}
warmup(){
  LONGDOC_DOC_TOKENS=1000 LONGDOC_QUESTIONS=2 LONGDOC_NUM_DOCS=1 LONGDOC_SEED=$1 \
    python3 -I $REPO/benchmarks/kv-offload-replay/longdoc_router.py --url $ROUTER \
    --output-tokens 200 --ignore-eos --out $OUT_ROOT/warmup-$1.jsonl >/dev/null 2>&1 \
    && log "warm-up ok" || die "warm-up failed"
}
deploy_arm(){  # deploy_arm <1|2|3>
  case $1 in
    1) log "deploy arm 1: P/D + certus fetch"
       ( cd $REPO && PD=1 WITH_CERTUS=1 RDMA=1 deploy/llm-d/install.sh ) > $OUT_ROOT/ov-install-$1.log 2>&1 || die "arm 1 install"
       kubectl -n $NS get deploy pd-disaggregation-nvidia-gpu-vllm-decode \
         -o jsonpath='{.spec.template.spec.containers[?(@.name=="modelserver")].env[*].name}' \
         | tr ' ' '\n' | grep -qx CERTUS_KV_EVENTS || die "arm 1: decode lacks CERTUS_KV_EVENTS" ;;
    2) log "deploy arm 2: P/D + certus, fetch off (decode publishes no KV events)"
       ( cd $REPO && PD=1 WITH_CERTUS=1 RDMA=1 deploy/llm-d/install.sh ) > $OUT_ROOT/ov-install-$1.log 2>&1 || die "arm 2 install"
       kubectl -n $NS set env deploy/pd-disaggregation-nvidia-gpu-vllm-decode CERTUS_KV_EVENTS- >>$LOG 2>&1
       kubectl -n $NS rollout status deploy/pd-disaggregation-nvidia-gpu-vllm-decode --timeout=30m >>$LOG 2>&1 || die "arm 2 rollout"
       # The live container env, not the whole object: kubectl's last-applied
       # annotation still records the install's spec, events included.
       kubectl -n $NS get deploy pd-disaggregation-nvidia-gpu-vllm-decode \
         -o jsonpath='{.spec.template.spec.containers[?(@.name=="modelserver")].env[*].name}' \
         | tr ' ' '\n' | grep -qx CERTUS_KV_EVENTS && die "arm 2: decode still has CERTUS_KV_EVENTS"
       for p in $(kubectl -n $NS get pods -o name | grep decode); do
         kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -q "KV events on" && die "arm 2: ${p#pod/} publishes KV events"
       done
       log "  decode publishes no KV events" ;;
    4) log "deploy arm 4: P/D + vLLM tiered (CPU + fs), no certus"
       for n in $NODES; do
         for d in 0 1; do ssh -o BatchMode=yes $n "mountpoint -q /mnt/kv-tier/numa$d" || die "arm 4: /mnt/kv-tier/numa$d not mounted on $n"; done
       done
       [ -z "$(kubectl -n certus get pods -o name 2>/dev/null)" ] || die "arm 4: certus still running"
       # Free the GPUs first, then start prefill alone and let it take its GPU
       # before decode starts: otherwise decode, allowed on both nodes, can
       # take both of the prefill node's GPUs and leave prefill Pending.
       kubectl -n $NS delete deploy optimized-baseline-nvidia-gpu-vllm-decode --ignore-not-found --wait=true >>$LOG 2>&1
       kubectl -n $NS scale deploy pd-disaggregation-nvidia-gpu-vllm-decode --replicas=0 >>$LOG 2>&1 || true
       ( cd $REPO && PD=1 RDMA=1 TIERED=1 PD_DECODE_REPLICAS=1 deploy/llm-d/install.sh ) > $OUT_ROOT/ov-install-$1a.log 2>&1 || die "arm 4 install (prefill first)"
       ( cd $REPO && PD=1 RDMA=1 TIERED=1 deploy/llm-d/install.sh ) > $OUT_ROOT/ov-install-$1.log 2>&1 || die "arm 4 install"
       kubectl -n $NS get deploy pd-disaggregation-nvidia-gpu-vllm-decode -o yaml | grep -q TieringOffloadingSpec || die "arm 4: decode lacks TieringOffloadingSpec"
       for p in $(kubectl -n $NS get pods -o name | grep -E "prefill|decode"); do
         kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -m1 "\[tiered\]" | sed "s|^|  ${p#pod/}: |" | tee -a $LOG
       done ;;
    5) log "deploy arm 5: P/D + vLLM CPU tier + P2P pull (llm-d P2P guide), no certus"
       [ -z "$(kubectl -n certus get pods -o name 2>/dev/null)" ] || die "arm 5: certus still running"
       # Prefill first, then decode, as arm 4: decode may otherwise take both of
       # the prefill node's GPUs.
       kubectl -n $NS scale deploy pd-disaggregation-nvidia-gpu-vllm-decode --replicas=0 >>$LOG 2>&1 || true
       ( cd $REPO && PD=1 RDMA=1 OFFLOAD=1 PD_DECODE_REPLICAS=1 deploy/llm-d/install.sh ) > $OUT_ROOT/ov-install-$1a.log 2>&1 || die "arm 5 install (prefill first)"
       ( cd $REPO && PD=1 RDMA=1 OFFLOAD=1 deploy/llm-d/install.sh ) > $OUT_ROOT/ov-install-$1.log 2>&1 || die "arm 5 install"
       for p in $(kubectl -n $NS get pods -o name | grep -E "prefill|decode"); do
         kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -q "Created secondary tier #0 (p2p)" || die "arm 5: ${p#pod/} has no p2p tier"
         # The recorded arm-5 run had PYTHONHASHSEED unset: with it pinned (as llm-d's
         # P2P guide says) the router named no pull source on vLLM 0.30. The profile
         # no longer sets it; this checks nothing else did.
         [ -z "$(kubectl -n $NS get $p -o jsonpath='{.spec.containers[?(@.name=="modelserver")].env[?(@.name=="PYTHONHASHSEED")].value}')" ] || die "arm 5: ${p#pod/} sets PYTHONHASHSEED (the router then names no pull source)"
         log "  ${p#pod/} on $(kubectl -n $NS get $p -o jsonpath='{.spec.nodeName}' | cut -d. -f1): p2p tier up, $(kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -m1 -oE 'ZmqTransport [0-9.:]+')"
       done ;;
    3) log "deploy arm 3: aggregated + certus (4 pods, no NIXL)"
       ( cd $REPO && PD=0 WITH_CERTUS=1 deploy/llm-d/install.sh ) > $OUT_ROOT/ov-install-$1.log 2>&1 || die "arm 3 install" ;;
  esac
  check_vllm; warmup $((9400+$1))
}
run(){  # run <idx> <arm>
  log "run $1: arm $2 seed $((400+$1))"
  $ARMS_DIR/run_arm.sh ov-$1-arm$2 $((400+$1)) >>$LOG 2>&1
  python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(" ",{k:d[k] for k in ("wall_s","turns_ok","errors","retired","later_ttft_p50_s","later_ttft_p90_s")})' $OUT_ROOT/ov-$1-arm$2/summary.json | tee -a $LOG
}

log "start"
case "${ARMS:-1 2 3}" in 4|5|"4 5") ;; *) wait_certus ;; esac
i=${START_IDX:-0}
for arm in ${ARMS:-1 2 3}; do i=$((i+1)); deploy_arm $arm; run $i $arm; done
if [ "${REPEAT:-0}" = 1 ]; then
  for arm in 3 2 1; do i=$((i+1)); deploy_arm $arm; run $i $arm; done
fi
log "ALL DONE"
