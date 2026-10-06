g/replicas: 8/s/replicas: 8/replicas: 3/
/^  replicas: 3$/a
  # 3 replicas on exactly 3 GPUs (a two-GPU node plus a one-GPU node) leaves no spare GPU for a
  # surge pod, so a rolling update must free one before claiming one. Without
  # this, maxSurge defaults to 25% and the rollout deadlocks: the old ReplicaSet
  # holds every GPU and never goes Ready, so it is never scaled down.
  strategy:
    rollingUpdate:
      maxSurge: 0
      maxUnavailable: 1
.
g@Qwen/Qwen3-32B@s@Qwen/Qwen3-32B@Qwen/Qwen3-8B@
g/--tensor-parallel-size=2/s/--tensor-parallel-size=2/--tensor-parallel-size=1/
/tensor-parallel-size=1/a
            - "--gpu-memory-utilization=0.90"
            # A30 is 24Gi: ~15.3Gi of bf16 weights plus activations and CUDA
            # graphs leave only ~4.1Gi for KV cache. Qwen3-8B advertises a
            # 40960 context, which needs 5.62Gi, so vLLM refuses to start.
            # 16384 needs ~2.25Gi, leaving room for concurrent requests.
            - "--max-model-len=16384"
.
g/cpu: '8'/s/cpu: '8'/cpu: '4'/
g/cpu: '16'/s/cpu: '16'/cpu: '8'/
g/memory: 96Gi/s/memory: 96Gi/memory: 64Gi/
g/memory: 128Gi/s/memory: 128Gi/memory: 96Gi/
g@nvidia.com/gpu: 2@s@nvidia.com/gpu: 2@nvidia.com/gpu: 1@
/name: HF_TOKEN/
.,+4s/^            /&\# /
/^          env:$/a
            # This image has HOME=/root and sets no HF_HOME, so HuggingFace
            # downloads ~16Gi of weights to /root/.cache on the container's
            # writable layer -- which sits under containerd's root on /home,
            # i.e. kubelet's imagefs. Every pod gets its own copy, and three of
            # them cross the imagefs.available<15% eviction threshold (22.2Gi
            # of 148Gi), so pods are evicted mid-download. Redirect both caches
            # into the mounted /.cache volume (a hostPath shared by every pod on
            # the node): one copy per node instead of one per pod, and it
            # survives pod restarts so a restart no longer re-downloads 16Gi.
            - name: HF_HOME
              value: /.cache/huggingface
            - name: VLLM_CACHE_ROOT
              value: /.cache/vllm
.
/^        - name: torch-compile-cache$/
+1c
          hostPath:
            path: /home/llm-d-cache
            type: DirectoryOrCreate
.
x
