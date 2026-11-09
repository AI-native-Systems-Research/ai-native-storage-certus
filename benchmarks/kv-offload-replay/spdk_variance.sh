#!/bin/bash
# Run Certus-SPDK v0.26.0 variant N times to characterize run-to-run variance.
# Identical config to the original 589.9s run. Sequential (single GPU/SSD set).
cd /home/dwaddington/ai-native-storage-certus || exit 2
N="${1:-4}"
BASE=/mnt/certus1/kvprofile-spdk026-var
SUMMARY="$BASE/summary.csv"
mkdir -p "$BASE"
echo "run,wall_s,tokens_per_sec,gen_per_s,status" > "$SUMMARY"

for i in $(seq 1 "$N"); do
  LOGDIR="$BASE/run$i"
  mkdir -p "$LOGDIR"
  echo "[var] === run $i/$N starting ==="
  bash benchmarks/kv-offload-replay/profile_all_unstable.sh \
    --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --device-pci 0000:63:00.0 \
    --model-fs /mnt/certus1 \
    --model NousResearch/Meta-Llama-3-8B \
    --only certus-spdk \
    --logdir "$LOGDIR" > "$LOGDIR/nohup.out" 2>&1
  # parse the results.json for this run
  if [ -f "$LOGDIR/results.json" ]; then
    line=$(grep -aoE '"variant": "Certus-SPDK"[^}]*' "$LOGDIR/results.json")
    wall=$(echo "$line" | grep -aoE '"wall_s": [0-9.]+' | grep -aoE '[0-9.]+')
    tps=$(echo "$line" | grep -aoE '"tokens_per_sec": [0-9.]+' | grep -aoE '[0-9.]+')
    gps=$(grep -aoE '\(([0-9.]+) gen/s\)' "$LOGDIR/nohup.out" | tail -1 | grep -aoE '[0-9.]+')
    st=$(echo "$line" | grep -aoE '"status": "[A-Z]+"' | grep -aoE '[A-Z]+$')
    echo "$i,$wall,$tps,$gps,$st" >> "$SUMMARY"
    echo "[var] run $i DONE: wall=${wall}s tokens/s=${tps} status=${st}"
  else
    echo "$i,,,,NORESULT" >> "$SUMMARY"
    echo "[var] run $i FAILED — no results.json"
  fi
  # brief settle between runs (SPDK teardown / GPU release)
  sleep 10
done
echo "[var] ALL $N RUNS DONE"
cat "$SUMMARY"
