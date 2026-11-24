#!/usr/bin/env bash
# One overflow run: fleet scraper beside the long-doc driver in interleaved order,
# then the EPP's p2p decisions for the run's window. Same workload for every arm.
# Usage: run_arm.sh <name> <seed>   (writes $OUT_ROOT/<name>, replacing it)
set -u
source "$(dirname "${BASH_SOURCE[0]}")/env.sh"
NAME=$1 SEED=$2
NCONV=${NCONV:-40} INFLIGHT=${INFLIGHT:-6} OUTTOK=${OUTTOK:-2000}
cd $REPO
OUT=$OUT_ROOT/$NAME; rm -rf $OUT; mkdir -p $OUT
kubectl -n $NS get pods -o wide > $OUT/pods.txt
python3 benchmarks/synthetic-agentic/scrape_prom.py --k8s --interval 10 --out $OUT/prom.log 2>$OUT/scrape.err & SP=$!
sleep 8
START=$(date -u +%Y-%m-%dT%H:%M:%S)
LONGDOC_DOC_TOKENS=4000 LONGDOC_QUESTIONS=8 LONGDOC_NUM_DOCS=$NCONV LONGDOC_SEED=$SEED \
  python3 -I benchmarks/kv-offload-replay/longdoc_router.py --url $ROUTER \
  --num-convs $NCONV --concurrency $INFLIGHT --order interleaved \
  --output-tokens $OUTTOK --ignore-eos --out $OUT/turns.jsonl > $OUT/summary.json 2>$OUT/driver.err
RC=$?
sleep 12; kill -TERM $SP; wait $SP
E=$(kubectl -n $NS get pods --no-headers | grep epp | grep Running | awk '{print $1}')
kubectl -n $NS logs $E -c epp --since-time="${START}Z" 2>&1 | grep "p2p decision" > $OUT/p2p.log
echo "RUN $NAME DONE rc=$RC"
