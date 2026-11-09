#!/usr/bin/env bash
# One arm of a multi-turn measurement through the llm-d router: the fleet
# scraper (scrape_prom.py --k8s) beside the prompt-level driver, then the EPP's
# p2p decisions for the arm's window. Results in $OUT_DIR/mt-<name>/.
# Usage: run_arm.sh <name> <tag>   (a fresh tag per arm: arms share no cache)
set -u
NAME=$1 TAG=$2
cd /home/scooter/ai-native-storage-certus
R=$(kubectl get svc -n llm-d-quickstart quickstart-epp -o jsonpath='{.spec.clusterIP}')
RI=$(kubectl get pod -n llm-d-quickstart $(kubectl get pods -n llm-d-quickstart --no-headers | grep render | grep Running | awk '{print $1}') -o jsonpath='{.status.podIP}')
OUT=${OUT_DIR:-/tmp}/mt-$NAME; rm -rf $OUT; mkdir -p $OUT
python3 benchmarks/synthetic-agentic/scrape_prom.py --k8s --interval 10 --out $OUT/prom.log 2>$OUT/scrape.err & SP=$!
sleep 8
START=$(date -u +%Y-%m-%dT%H:%M:%S)
python3 benchmarks/multiturn-router/multiturn_router.py --url http://$R --render http://$RI:8000 \
  --conversations data/sharegpt_12turn_450.json --sessions 12 --concurrency 3 --turns 5 \
  --answer-tokens 1100 --tag "$TAG" --out $OUT/turns.jsonl > $OUT/summary.json 2>$OUT/driver.err
sleep 12; kill -TERM $SP; wait $SP
E=$(kubectl get pods -n llm-d-quickstart --no-headers | grep epp | grep Running | awk '{print $1}')
# Only this arm's decisions: the EPP log spans earlier runs too.
kubectl logs -n llm-d-quickstart $E -c epp --since-time="${START}Z" 2>&1 | grep "p2p decision" > $OUT/p2p.log
echo "ARM $NAME DONE"
