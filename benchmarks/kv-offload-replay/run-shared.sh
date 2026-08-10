#!/bin/bash
V=/mnt/certus1/venv_vllm/bin/python   # vLLM 0.20.0 venv
DATASET_PATH=$PWD/../../certus-connector/sharegpt_12turn_450.json \
	    NUM_CONVS=450 \
	    MAX_MODEL_LEN=8192 \
	    OUTPUT_TOKENS=150 \
	    MAX_NUM_SEQS=64 \
	    GPU_MEM_UTIL=0.90 \
	    DRAM=$((32 * (1<<30))) \
	    MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 HF_HOME=/mnt/certus1/hf_cache \
	    DISK_DEV=nvme7n1 \
	    PYTHONPATH=/mnt/certus1/llm-d-kv-cache/kv_connectors/llmd_fs_backend \
$V run_fs_bench_450.py 2>&1 | tee ss_450.log
