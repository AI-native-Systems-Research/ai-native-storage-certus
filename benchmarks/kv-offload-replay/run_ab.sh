#!/usr/bin/env bash
# A/B: patched-tracing vs stock no-tracing, identical 20-conv workload.
cd /home/dwaddington/certus/benchmarks/kv-offload-replay
V=/mnt/certus1/venv_vllm/bin/python

common_env() {
  export HF_HOME=/mnt/certus1/hf_cache
  export HF_HUB_OFFLINE=1
  export DATASET_PATH=$PWD/../../certus-connector/sharegpt_12turn_450.json
  export NUM_CONVS=20
  export MAX_MODEL_LEN=8192
  export OUTPUT_TOKENS=150
  export MAX_NUM_SEQS=64
  export GPU_MEM_UTIL=0.90
  export CPU_BYTES=$((32 * (1<<30)))
  export MODEL=NousResearch/Meta-Llama-3-8B
}

echo "############## RUN A: PATCHED TRACING (KV_TRACING=1) ##############"
common_env; export KV_TRACING=1
rm -f offloading_*.jsonl
$V run_multiturn_offloading.py > ab_traced.log 2>&1
echo "A_EXIT=$?"
echo "traced trace-file sizes:"; ls -lh offloading_*.jsonl 2>/dev/null | awk '{print "  "$5, $9}'
cp -f offloading_trace_*.jsonl /tmp/ab_traced_connector.jsonl 2>/dev/null || true

echo "############## RUN B: STOCK NO-TRACING (KV_TRACING=0) ##############"
common_env; export KV_TRACING=0
rm -f offloading_*.jsonl
$V run_multiturn_offloading.py > ab_baseline.log 2>&1
echo "B_EXIT=$?"

echo "AB_DONE"
