#!/bin/bash
V=/mnt/certus1/venv_vllm/bin/python   # vLLM 0.20.0 venv
DATASET_PATH=$PWD/../../certus-connector/sharegpt_12turn_450.json \
	    NUM_CONVS=450 \
	    MAX_MODEL_LEN=8192 \
	    OUTPUT_TOKENS=150 \
	    MAX_NUM_SEQS=64 \
	    GPU_MEM_UTIL=0.90 \
	    CPU_BYTES=$((32 * (1 << 30))$((48 * (1<<30)))) \
	    MODEL=NousResearch/Meta-Llama-3-8B \
	    HF_HUB_OFFLINE=1 \
	    KV_TRACING=0 \ 
	    $V run_multiturn_offloading.py 2>&1 | tee cpu_offload_450.log

