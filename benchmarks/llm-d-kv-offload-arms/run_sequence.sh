#!/usr/bin/env bash
# Overflow arms 1-3 again, each from EMPTY certus tiers. Before every arm: stop
# the model servers, restart certus (format-on-start: a fresh superblock, every
# tier empty), check the tiers read 0, bring the model servers back (prefill
# pinned to node5 GPU1 behind a placeholder pod, as every P/D arm ran), then the
# router check. Same workload and seeds as the full-tier runs (401-403), so each
# arm sees the same documents and only the starting tier state differs.
set -u
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
LOG=${LOG:-$OUT_ROOT/run_sequence.log}
PREFIX=${PREFIX:-fx2}    # run names are <prefix>-<arm>-arm<arm>[-r<repeat>]; the recorded runs used c13 and fx2
CERTUS_TAG=${CERTUS_TAG:-fc2a2b80}   # every Certus instance must run certus:<tag>
NODES="node2 node5"   # ssh names
SIDECAR=ghcr.io/llm-d/llm-d-router-disagg-sidecar@sha256:adea9eda69259ab60737079c635f5158cfbd31d1e9eadd26ee1c1633a4f835ed
SIDECAR_SHA=ddabbfd0a4f50983260e88949d67de67d15adcd7
log(){ echo "$(date -u +%H:%M:%S) $*" | tee -a $LOG; }
die(){ log "FATAL: $*"; exit 1; }

certus_used(){  # total bytes used across all certus memory tiers
  local tot=0 p port n ip u
  for p in $(kubectl -n certus get pods -o name); do
    port=$(kubectl -n certus get $p -o jsonpath='{.spec.containers[0].env[?(@.name=="CERTUS_METRICS_PORT")].value}')
    n=$(kubectl -n certus get $p -o jsonpath='{.spec.nodeName}')
    ip=$(kubectl get node $n -o jsonpath='{.status.addresses[?(@.type=="InternalIP")].address}')
    u=$(curl -s --max-time 5 http://$ip:$port/metrics | awk '/^certus_memory_tier_used_bytes /{print $2}')
    [ -n "$u" ] || { echo "unreadable"; return; }
    tot=$(python3 -c "print(int($tot+$u))")
  done
  echo $tot
}
stop_model_servers(){
  for d in $(kubectl -n $NS get deploy -o name | grep -E "vllm-(decode|prefill)|optimized-baseline"); do
    kubectl -n $NS scale $d --replicas=0 >/dev/null
  done
  kubectl -n $NS wait --for=delete pod -l llm-d.ai/role=decode --timeout=300s >/dev/null 2>&1 || true
  kubectl -n $NS wait --for=delete pod -l llm-d.ai/role=prefill --timeout=300s >/dev/null 2>&1 || true
  for i in $(seq 1 60); do
    [ "$(kubectl -n $NS get pods --no-headers 2>/dev/null | grep -c -E 'vllm-(decode|prefill)|optimized-baseline')" = 0 ] && return 0; sleep 5
  done
  die "model servers did not stop"
}
certus_drives_ok(){  # every instance pinned a poller to all 4 of its own (NUMA-local) drives (still checked after the Certus fix fc2a2b80)
  local p bad=""
  for p in $(kubectl -n certus get pods -o name); do
    local l numa own pinned missing
    l=$(kubectl -n certus logs $p 2>/dev/null)
    numa=$(kubectl -n certus get $p -o jsonpath='{.metadata.labels.numa}')
    [ -n "$numa" ] || numa=$(grep -o -m1 "pinned to NUMA-[01]" <<<"$l" | grep -o "[01]$")
    # Own drives: the init container's list ("NUMA <n> drives: a,b,c,d").
    own=$(kubectl -n certus logs $p -c discover-drives 2>/dev/null | grep -o -m1 "drives: .*" | cut -d' ' -f2 | tr ',' ' ')
    pinned=$(grep -o "auto-pinning poller for [0-9a-f:.]*" <<<"$l" | awk '{print $4}')
    missing=""; for d in $own; do grep -qx "$d" <<<"$pinned" || missing="$missing $d"; done
    [ "$(wc -w <<<"$own")" = 4 ] && [ -z "$missing" ] || bad="$bad ${p#pod/}($(kubectl -n certus get $p -o jsonpath='{.spec.nodeName}' | cut -d. -f1) NUMA $numa: own=[$own] unpinned=[$missing])"
  done
  [ -z "$bad" ] && return 0
  log "certus drives short:$bad"; return 1
}
fresh_certus(){
  # Two instances per server start together, and SPDK's enumeration opens every
  # drive's VFIO group, so one can hold the other's drives and leave it with
  # only some pinned (EAL: "Device or resource busy"): loads then run at a
  # quarter speed on that server for the whole run. Restart until all have 4.
  local try
  for try in 1 2 3; do
    kubectl -n certus delete pod -l app=certus-server --wait=true >/dev/null
    for ds in certus-server-numa0 certus-server-numa1; do kubectl -n certus rollout status ds/$ds --timeout=10m >/dev/null 2>&1 || die "certus rollout"; done
    for i in $(seq 1 60); do
      n=0; for p in $(kubectl -n certus get pods -o name); do kubectl -n certus logs $p 2>/dev/null | grep -q "transfer endpoint: listening" && n=$((n+1)); done
      [ $n -ge 4 ] && break; sleep 10
    done
    [ $n -ge 4 ] || die "certus endpoints not up"
    certus_drives_ok && break
    [ $try = 3 ] && die "certus instances short of drives after 3 restarts"
    log "restarting certus (try $((try + 1)) of 3)"
  done
  for i in $(seq 1 12); do u=$(certus_used); [ "$u" != unreadable ] && break; sleep 5; done
  [ "$u" = 0 ] || die "certus tiers not empty after restart ($u bytes)"
  for p in $(kubectl -n certus get pods -o name); do
    kubectl -n certus logs $p 2>/dev/null | grep -q "batched piece copies on" || die "${p#pod/}: batched piece copies not on"
  done
  for p in $(kubectl -n certus get pods -o name); do
    kubectl -n certus get $p -o jsonpath='{.spec.containers[0].image}' | grep -q "certus:$CERTUS_TAG$" || die "${p#pod/}: not on certus:$CERTUS_TAG"
    kubectl -n certus logs $p 2>/dev/null | grep -q "Restricting to 4 PCI device" || die "${p#pod/}: no 4-drive allow-list"
  done
  log "certus restarted fresh: 4 instances on $CERTUS_TAG, allow-listed and 4 drives pinned each, tiers 0 bytes used, batched piece copies on"
}
pin_pd(){  # prefill onto node5 GPU1 (A1:00) behind a placeholder holding GPU0, then 3 decode
  local i m P
  for i in $(seq 1 10); do
    sed "s|%%PREFILL_NODE%%|$(node_label $PREFILL_NODE)|; s|%%NS%%|$NS|" $ARMS_DIR/gpu-hold.yaml | kubectl apply -f - >/dev/null
    kubectl -n $NS wait --for=condition=Ready pod/gpu-hold --timeout=180s >/dev/null || die "placeholder not ready"
    m=$(kubectl -n $NS exec gpu-hold -- sh -c 'ls /dev/nvidia[0-9]* 2>/dev/null' | grep -oE '[0-9]+$')
    [ "$m" = 0 ] && break
    kubectl -n $NS delete pod gpu-hold --grace-period=0 --wait=true >/dev/null 2>&1
  done
  [ "$m" = 0 ] || die "placeholder never held node5 GPU0"
  kubectl -n $NS scale deploy pd-disaggregation-nvidia-gpu-vllm-prefill --replicas=1 >/dev/null
  kubectl -n $NS rollout status deploy/pd-disaggregation-nvidia-gpu-vllm-prefill --timeout=30m >/dev/null 2>&1 || die "prefill rollout"
  P=$(kubectl -n $NS get pods -o name | grep prefill)
  [ "$(kubectl -n $NS exec $P -c modelserver -- nvidia-smi --query-gpu=pci.bus_id --format=csv,noheader)" = "00000000:A1:00.0" ] || die "prefill not on GPU1"
  kubectl -n $NS delete pod gpu-hold --grace-period=0 --wait=true >/dev/null 2>&1
  kubectl -n $NS scale deploy pd-disaggregation-nvidia-gpu-vllm-decode --replicas=3 >/dev/null
  kubectl -n $NS rollout status deploy/pd-disaggregation-nvidia-gpu-vllm-decode --timeout=30m >/dev/null 2>&1 || die "decode rollout"
}
record_pods(){
  local p
  local n=0
  for p in $(kubectl -n $NS get pods -o name | grep -E "vllm-(decode|prefill)|optimized-baseline"); do
    n=$((n+1))
    log "  ${p#pod/} $(kubectl -n $NS get $p -o jsonpath='{.spec.nodeName}' | cut -d. -f1) $(kubectl -n $NS exec $p -c modelserver -- nvidia-smi --query-gpu=pci.bus_id --format=csv,noheader 2>/dev/null) $(kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -m1 -oE 'GPU KV cache size: [0-9,]+')"
    kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -q "GPU KV cache size: 37,184" || die "${p#pod/}: GPU KV not 37,184"
  done
  [ $n = 4 ] || die "expected 4 model-server pods, found $n"
}
AGG=optimized-baseline-nvidia-gpu-vllm-decode
aggenv(){ kubectl -n $NS get deploy $AGG -o jsonpath='{.spec.template.spec.containers[?(@.name=="modelserver")].env[*].name}' | tr ' ' '\n'; }
aggval(){ kubectl -n $NS get deploy $AGG -o jsonpath="{.spec.template.spec.containers[?(@.name==\"modelserver\")].env[?(@.name==\"$1\")].value}"; }
check_sidecars(){  # check_sidecars <pod pattern>
  local p
  for p in $(kubectl -n $NS get pods -o name | grep "$1"); do
    kubectl -n $NS logs $p -c routing-proxy 2>/dev/null | grep -q "$SIDECAR_SHA" || die "${p#pod/}: sidecar is not $SIDECAR_SHA"
  done
  log "sidecars ($1) on $SIDECAR_SHA"
}
deploy_arm(){  # The profiles pin the sidecar by digest (certus-host-setup ac42370a); checked here.
  case $1 in
    1) ( cd $REPO && PD=1 WITH_CERTUS=1 RDMA=1 PD_DECODE_REPLICAS=1 deploy/llm-d/install.sh ) > $OUT_ROOT/$PREFIX-install-$1.log 2>&1 || die "arm 1 install"
       stop_model_servers; pin_pd; check_sidecars decode
       kubectl -n $NS get deploy pd-disaggregation-nvidia-gpu-vllm-decode -o jsonpath='{.spec.template.spec.containers[?(@.name=="modelserver")].env[*].name}' | tr ' ' '\n' | grep -qx CERTUS_KV_EVENTS || die "arm 1: decode lacks CERTUS_KV_EVENTS" ;;
    2) ( cd $REPO && PD=1 WITH_CERTUS=1 RDMA=1 PD_DECODE_REPLICAS=1 deploy/llm-d/install.sh ) > $OUT_ROOT/$PREFIX-install-$1.log 2>&1 || die "arm 2 install"
       stop_model_servers
       kubectl -n $NS set env deploy/pd-disaggregation-nvidia-gpu-vllm-decode -c modelserver CERTUS_KV_EVENTS- >/dev/null
       pin_pd; check_sidecars decode
       kubectl -n $NS get deploy pd-disaggregation-nvidia-gpu-vllm-decode -o jsonpath='{.spec.template.spec.containers[?(@.name=="modelserver")].env[*].name}' | tr ' ' '\n' | grep -qx CERTUS_KV_EVENTS && die "arm 2: decode still has CERTUS_KV_EVENTS"
       for p in $(kubectl -n $NS get pods -o name | grep decode); do kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -q "KV events on" && die "arm 2: ${p#pod/} publishes KV events"; done ;;
    3|3a|3b)
       # One install for the aggregated arms (certus-fetch), then each arm's
       # env set by hand before the pods start:
       #   3   prompts only (vLLM's default), no KV events: the original arm 3
       #   3a  answers stored, no KV events: no fetch source can be named
       #   3b  answers stored, KV events: fetch on a session move
       ( cd $REPO && WITH_CERTUS=1 FETCH=1 deploy/llm-d/install.sh ) > $OUT_ROOT/$PREFIX-install-$1.log 2>&1 || die "arm $1 install"
       kubectl -n $NS scale deploy $AGG --replicas=0 >/dev/null
       for i in $(seq 1 60); do [ "$(kubectl -n $NS get pods --no-headers | grep -c optimized-baseline)" = 0 ] && break; sleep 5; done
       case $1 in
         3)  kubectl -n $NS set env deploy/$AGG -c modelserver CERTUS_KV_EVENTS- CERTUS_OFFLOAD_PROMPT_ONLY=true >/dev/null ;;
         3a) kubectl -n $NS set env deploy/$AGG -c modelserver CERTUS_KV_EVENTS- CERTUS_OFFLOAD_PROMPT_ONLY=false >/dev/null ;;
         3b) kubectl -n $NS set env deploy/$AGG -c modelserver CERTUS_KV_EVENTS='tcp://*:5556' CERTUS_OFFLOAD_PROMPT_ONLY=false >/dev/null ;;
       esac
       case $1 in 3b) aggenv | grep -qx CERTUS_KV_EVENTS || die "3b lacks KV events" ;; *) aggenv | grep -qx CERTUS_KV_EVENTS && die "$1 has KV events" ;; esac
       [ "$(aggval CERTUS_OFFLOAD_PROMPT_ONLY)" = "$([ $1 = 3 ] && echo true || echo false)" ] || die "$1: CERTUS_OFFLOAD_PROMPT_ONLY wrong"
       kubectl -n $NS scale deploy $AGG --replicas=4 >/dev/null
       kubectl -n $NS rollout status deploy/$AGG --timeout=30m >/dev/null 2>&1 || die "arm $1 rollout"
       sleep 40   # the entrypoint annotates in the background
       check_sidecars optimized-baseline
       for p in $(kubectl -n $NS get pods -o name | grep optimized); do
         kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -q "annotated certus.ai/transfer-endpoint" || die "${p#pod/}: not annotated"
       done
       log "arm $1: prompt-only=$(aggval CERTUS_OFFLOAD_PROMPT_ONLY), KV events $(aggenv | grep -qx CERTUS_KV_EVENTS && echo on || echo off)" ;;
  esac
}
run_arm(){  # run_arm <arm>: fresh certus, deploy, checks, warm-up, run
  local arm=$1 seed=$((400+${1%[ab]}))
  log "=== arm $arm (seed $seed): fresh start"
  # kubelet evicts pods when node2's /home (its image filesystem) has < 23.8 GB free.
  local free; free=$(ssh node2 'df -B1 --output=avail /home | tail -1')
  [ "$free" -gt 32000000000 ] || die "node2 /home has only $((free/1000000000)) GB free; kubelet evicts below 23.8 GB"
  [ "$(kubectl get node node2 -o jsonpath='{range .status.conditions[?(@.type=="DiskPressure")]}{.status}{end}')" = False ] || die "node2 has DiskPressure"
  stop_model_servers
  fresh_certus
  deploy_arm $arm
  record_pods
  $REPO/deploy/llm-d/router-check.sh >>$LOG 2>&1 || die "router failing open"
  log "router check ok"
  LONGDOC_DOC_TOKENS=1000 LONGDOC_QUESTIONS=2 LONGDOC_NUM_DOCS=1 LONGDOC_SEED=$((9800+${arm%[ab]})) \
    python3 -I $REPO/benchmarks/kv-offload-replay/longdoc_router.py --url $ROUTER \
    --output-tokens 200 --ignore-eos --out $OUT_ROOT/warmup-$PREFIX-$arm.jsonl >/dev/null 2>&1 \
    && log "warm-up ok" || die "warm-up failed"
  log "tiers used before run: $(certus_used) bytes (warm-up only)"
  python3 -I $ARMS_DIR/load_watch.py $LOG & local LW=$!
  local out=$PREFIX-$arm-arm$arm${REP:+-r$REP}
  [ -e $OUT_ROOT/$out ] && die "$out already exists; not overwriting"
  $ARMS_DIR/run_arm.sh $out $seed >>$LOG 2>&1
  kill $LW 2>/dev/null; wait $LW 2>/dev/null
  log "load-watch: $(grep -c 'load-watch WARN' $LOG) slow-load warnings in the log so far"
  python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(" ",{k:d[k] for k in ("wall_s","turns_ok","errors","retired","later_ttft_p50_s","later_ttft_p90_s")})' $OUT_ROOT/$out/summary.json | tee -a $LOG
  f=0; for p in $(kubectl -n certus get pods -o name); do port=$(kubectl -n certus get $p -o jsonpath='{.spec.containers[0].env[?(@.name=="CERTUS_METRICS_PORT")].value}'); n=$(kubectl -n certus get $p -o jsonpath='{.spec.nodeName}'); ip=$(kubectl get node $n -o jsonpath='{.status.addresses[?(@.type=="InternalIP")].address}'); v=$(curl -s http://$ip:$port/metrics | awk '/^certus_fetch_requests_total /{print $2}'); f=$((f+${v%.*})); done
  log "certus fetch requests in the run: $f"
  case $arm in 1|3b) [ "$f" -ge 1 ] || log "INVALID: arm $arm made no certus fetch" ;; *) [ "$f" = 0 ] || log "INVALID: arm $arm fetched ($f)" ;; esac
  log "ARM $arm DONE"
}

log "start: overflow arms 1-3 from empty certus tiers"
if [ -n "${REPEATS:-}" ]; then
  # Alternate the arms within each repeat, so drift over the hours falls on all.
  for REP in $REPEATS; do for arm in ${ARMS:-1 2 3}; do run_arm $arm; done; done
else
  for arm in ${ARMS:-1 2 3}; do run_arm $arm; done
fi
log "ALL DONE"
