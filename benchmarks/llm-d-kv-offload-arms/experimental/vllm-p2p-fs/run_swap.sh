#!/usr/bin/env bash
# Experimental arm 5 (P2P + fs, events as certus's) at 3.4x overflow: the same
# workload as every overflow arm (40 x 8, 2000-token answers, 6 in flight,
# interleaved), seed 435 as the committed arm 5's option-1 run, from empty
# tiers (each engine empties its disk tier at start), the load watch beside it.
set -u
source "$(dirname "${BASH_SOURCE[0]}")/../../env.sh"
LOG=${LOG:-$OUT_ROOT/arm5fs.log}
source <(sed -n '/^log(){/p;/^die(){/p;/^stop_model_servers(){/,/^}/p;/^pin_pd(){/,/^}/p;/^record_pods(){/,/^}/p' $ARMS_DIR/run_sequence.sh)
RUN=${1:-ov-5fs-swap}
OUT=$OUT_ROOT/$RUN
[ -e $OUT ] || [ -e $OUT-mslogs ] && { echo "$OUT exists"; exit 1; }
log "=== arm 5 + fs (experiment, fs tier before P2P, debug logging): overflow run $RUN from empty tiers"
stop_model_servers
for h in node2 node5; do for n in 0 1; do ssh $h "mountpoint -q /mnt/kv-tier/numa$n" || die "$h numa$n not mounted"; done; done
pin_pd
record_pods
for p in $(kubectl -n $NS get pods -o name | grep -E "vllm-(decode|prefill)"); do
  kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -q "Created secondary tier #0 (fs)" || die "${p#pod/}: no fs tier"
  kubectl -n $NS logs $p -c modelserver 2>/dev/null | grep -q "KV events as certus" || die "${p#pod/}: no event filter"
done
for h in node2 node5; do ssh $h 'for n in 0 1; do echo "$(hostname -s) numa$n files $(find /mnt/kv-tier/numa$n -type f | wc -l)"; done' >> $LOG 2>&1; done
$REPO/deploy/llm-d/router-check.sh >>$LOG 2>&1 || die "router failing open"
LONGDOC_DOC_TOKENS=1000 LONGDOC_QUESTIONS=2 LONGDOC_NUM_DOCS=1 LONGDOC_SEED=9935 \
  python3 -I $REPO/benchmarks/kv-offload-replay/longdoc_router.py --url $ROUTER \
  --output-tokens 200 --ignore-eos --out $OUT_ROOT/warmup-ov.jsonl >/dev/null 2>&1 || die "warm-up failed"
mkdir -p $OUT-mslogs
for p in $(kubectl -n $NS get pods -o name | grep -E "vllm-(decode|prefill)"); do
  kubectl -n $NS logs -f $p -c modelserver > $OUT-mslogs/${p#pod/}.log 2>&1 & LOGPIDS="${LOGPIDS:-} $!"
done
log "checks ok (4 pods, fs tier + event filter on each, router ok, warm-up ok)"
python3 -I $ARMS_DIR/load_watch.py $LOG & LW=$!
$ARMS_DIR/run_arm.sh $RUN 435 >>$LOG 2>&1
kill $LW $LOGPIDS 2>/dev/null; wait $LW 2>/dev/null
python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(" ",{k:d[k] for k in ("wall_s","turns_ok","errors","retired","later_ttft_p50_s","later_ttft_p90_s")})' $OUT_ROOT/$RUN/summary.json | tee -a $LOG
log "pulled: $(grep -c 'outcome\":\"pulled' $OUT_ROOT/$RUN/p2p.log) of $(wc -l < $OUT_ROOT/$RUN/p2p.log) decisions"
python3 -I $ARMS_DIR/experimental/vllm-p2p-fs/p2p_timeline.py $(ls $OUT-mslogs/*prefill*.log) $(ls $OUT-mslogs/*decode*.log) > $OUT-mslogs/p2p_timeline.txt 2>&1
log "OVERFLOW DONE $RUN"
