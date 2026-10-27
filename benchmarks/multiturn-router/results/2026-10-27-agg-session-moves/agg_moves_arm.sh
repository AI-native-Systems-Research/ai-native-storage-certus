#!/usr/bin/env bash
# agg_moves_arm.sh <name> <concurrency> <sessions> : one multiturn run through the
# aggregated router, with the EPP log followed for the run (needs EPP --v=4) and its
# picks matched to the turns by send time (moves_from_epp.py).
set -u
cd /home/scooter/ai-native-storage-certus
ns=llm-d-quickstart; NAME=$1 C=$2 S=$3
R=$(kubectl get svc -n $ns quickstart-epp -o jsonpath='{.spec.clusterIP}')
RI=$(kubectl get pods -n $ns -o wide --no-headers | awk '/^render/{print $6}')
E=$(kubectl get pods -n $ns --no-headers | awk '/epp/ && /Running/{print $1}')
OUT=/tmp/agg-$NAME; rm -rf $OUT; mkdir -p $OUT
# The EPP log rotates within minutes at --v=4: follow it for the whole run.
kubectl logs -n $ns $E -c epp -f --since=1s 2>/dev/null | grep --line-buffered 'TargetEndpoints' > $OUT/picks.log &
LP=$!
sleep 2
python3 benchmarks/multiturn-router/multiturn_router.py --url http://$R --render http://$RI:8000 \
  --conversations data/sharegpt_12turn_450.json --sessions $S --concurrency $C --turns 5 \
  --answer-tokens 1100 --tag "AGG-$NAME-$(date +%s)" --out $OUT/turns.jsonl > $OUT/summary.json 2>$OUT/driver.err
sleep 5; kill $LP 2>/dev/null; wait $LP 2>/dev/null
python3 "$(dirname "$0")/moves_from_epp.py" $OUT/turns.jsonl $OUT/picks.log | tee $OUT/moves.json
