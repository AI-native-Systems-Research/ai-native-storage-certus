### Example: Running CPU-Tiering+FS plugin with SharedGPT data and patched/fixed vLLM.

Runs 12 rounds with open (barrier/sync) scheduling - you can see the experiment run for 12 rounds only. By default DISK_DIR_HOST=/mnt/certus1/kv-fs-tier which is where the spill happens. This defaults to shared GPT data.

```bash
[dwaddington@node0 kv-offload-replay]$ IMAGE=certus-offload-bench-fix026 NUM_CONVS=200 OUTPUT_TOKENS=150 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.95 MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 ./run-docker-cputier.sh
```

On a single A100, the wall clock time is ~223s.

```bash
[run] done. wall=223.8s generations=2400 rounds=12
```

### Example: Running CPU-Tiering+FS plugin with synthetic data and patched/fixed vLLM, closed loop.

Runs 12 rounds with closed (async) scheduling. The output shows the async progress. By default DISK_DIR_HOST=/mnt/certus1/kv-fs-tier which is where the spill happens.

```bash
[dwaddington@node0 kv-offload-replay]$ WORKLOAD_MODE=async ACTIVE_SESSIONS=80 DATASET_HOST=/home/dwaddington/ai-native-storage-certus-internal/data/synth-1K-M50.json NUM_CONVS=100 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.95 MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 ./run-docker-cputier-patched.sh
```

Should show:

```bash
[run] loaded 100 conversations  (human-turn count: min=36 median=50 max=68)
```

On a single A100, the wall clock time is ~1477s.

```bash
[run] done. wall=1477.0s generations=4398 rounds=52
```

### Example: Running CPU-Tiering+FS plugin with synthetic data and patched/fixed vLLM, closed loop, RAID-0 filesystem.

Create filesystem and mount to /mnt/ssdraid0, then create a subdir `data`:

```bash
[dwaddington@node0 kv-offload-replay]$ DISK_DIR_HOST=/mnt/ssdraid0/data WORKLOAD_MODE=async ACTIVE_SESSIONS=80 DATASET_HOST=/home/dwaddington/ai-native-storage-certus-internal/data/synth-1K-M50.json NUM_CONVS=100 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.95 MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 ./run-docker-cputier-patched.sh
```

On a single A100 and 4-device s/w RAID-0, the wall clock time is ~938s.

```bash
[run] done. wall=938.5s generations=4421 rounds=53
```

### Example: Running Certus with synthetic data and patched/fixed vLLM, closed loop.

Default is to use 3x SSD devices. Default memory tier size is 32G.

```bash
[dwaddington@node0 kv-offload-replay]$ WORKLOAD_MODE=async ACTIVE_SESSIONS=80 DATASET_HOST=/home/dwaddington/ai-native-storage-certus-internal/data/synth-1K-M50.json NUM_CONVS=100 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.95 MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 ./run-docker-certus-shmq.sh
```

On a single A100 and 3 NVMe devices, the wall clock time is ~826s

```bash
[run] DONE rounds=53 generations=4395 elapsed=826.3s (5.3 gen/s)
```

### Example: Running Certus with synthetic data and patched/fixed vLLM, closed loop on single device.

Added DEVICE_PCI="0000:61:00.0"

```bash
[dwaddington@node0 kv-offload-replay]$ DEVICE_PCI="0000:61:00.0" WORKLOAD_MODE=async ACTIVE_SESSIONS=80 DATASET_HOST=/home/dwaddington/ai-native-storage-certus-internal/data/synth-1K-M50.json NUM_CONVS=100 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.95 MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 ./run-docker-certus-shmq.sh
```

On a single A100 and a single NVMe device, the wall clock time is ~821s.

```bash
[run] DONE rounds=53 generations=4403 elapsed=821.1s (5.4 gen/s)
```

### Example: Running Certus with synthetic data and patched/fixed vLLM, closed loop across 2 GPUs.

```bash
[dwaddington@node0 kv-offload-replay]$ TENSOR_PARALLEL_SIZE=2 CHANNELS=64 WORKLOAD_MODE=async ACTIVE_SESSIONS=80 DATASET_HOST=/home/dwaddington/ai-native-storage-certus-internal/data/synth-1K-M50.json NUM_CONVS=100 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.95 MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 ./run-docker-certus-shmq.sh
```

On two A100s and 3 NVMe devices, the wall clock time is ~622s.

```bash
[run] DONE rounds=51 generations=4392 elapsed=621.3s (7.1 gen/s)
```

### Example: Running CPU-Tiering+FS plugin with synthetic data and patched/fixed vLLM, closed loop, with 2 GPUs.

```bash
[dwaddington@node0 kv-offload-replay]$ TENSOR_PARALLEL_SIZE=2 CHANNELS=64 WORKLOAD_MODE=async ACTIVE_SESSIONS=80 DATASET_HOST=/home/dwaddington/ai-native-storage-certus-internal/data/synth-1K-M50.json NUM_CONVS=100 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.95 MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 ./run-docker-cputier-patched.sh
```

On two A100s and single-device FS, the wall clock time is ~799s.

```bash
[run] done. wall=799.5s generations=4388 rounds=58
```

### Example: Running CPU-Tiering+FS plugin with synthetic data and patched/fixed vLLM, closed loop, with 2 GPUs.

With 3-device RAID-0

```bash
[dwaddington@node0 kv-offload-replay]$ TENSOR_PARALLEL_SIZE=2 CHANNELS=64 DISK_DIR_HOST=/mnt/ssdraid0/data WORKLOAD_MODE=async ACTIVE_SESSIONS=80 DATASET_HOST=/home/dwaddington/ai-native-storage-certus-internal/data/synth-1K-M50.json NUM_CONVS=100 MAX_NUM_SEQS=64 GPU_MEM_UTIL=0.95 MODEL=NousResearch/Meta-Llama-3-8B HF_HUB_OFFLINE=1 ./run-docker-cputier-patched.sh
```

On two A100s and 3-device FS, the wall clock time is ~815s.

```bash
[run] done. wall=815.2s generations=4403 rounds=53
```
