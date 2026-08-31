# vLLM workload-driver image for the Certus shared-memory (shmq) connector.
#
# This image is the CLIENT side only: vLLM + the pure-Python shmq connector +
# the multi-turn workload driver. It offloads KV blocks over a /dev/shm mailbox
# to a SEPARATELY-RUN certus-server (which owns SPDK/NVMe/hugepages on the
# host). The server is intentionally NOT built into this image — run it on the
# host (cargo build -p certus-server after deps/build_spdk.sh) and point
# this image at its mailbox via SHM_PATH.
#
# The mailbox lives on the host's /dev/shm; --ipc=host makes it (and the CUDA
# IPC handles) visible inside the container. There is NO network transport here:
# no grpcio, no protobuf, no wire codegen — just ctypes + mmap over /dev/shm.
#
# Build (context = repo root — needs certus-shmq-connector/ + the dataset):
#   podman build -f certus-shmq-connector/Dockerfile -t certus-shmq-bench .
#
# Run (server's mailbox reachable via --ipc=host; GPU exposed so the server can
# open the CUDA IPC handles this container's vLLM process exports):
#   podman run --rm --device nvidia.com/gpu=all --ipc=host \
#       -e SHM_PATH=/dev/shm/certus-shmq \
#       -e HF_TOKEN=<token> \
#       -v $HOME/.cache/huggingface:/root/.cache/huggingface \
#       certus-shmq-bench

# vLLM base-image version. Supported (built + smoke-tested): 0.20.0, 0.22.0,
# 0.23.0, 0.24.0, 0.26.0. The connector self-adapts at RUN time via
# certus_shmq_connector.compat (version detection + capability matrix) — this
# arg only selects the base image, no code keys off it. Build another version:
#   podman build --build-arg VLLM_VERSION=0.22.0 -f certus-shmq-connector/Dockerfile \
#       -t certus-shmq-bench:vllm0.22 .
ARG VLLM_VERSION=0.23.0
FROM docker.io/vllm/vllm-openai:v${VLLM_VERSION}
ENTRYPOINT []

# GPU is REQUIRED at run time: vLLM loads the model on the GPU and the connector
# exports CUDA IPC handles from the KV cache. WHICH GPU is a run-time choice —
# select it on the run command, not here (do NOT bake NVIDIA_VISIBLE_DEVICES
# into the image; that would pre-select all GPUs and defeat --gpus):
#   docker:  --gpus all   |  --gpus '"device=0"'
#   podman:  --device nvidia.com/gpu=all  |  --device nvidia.com/gpu=0
# Those flags set NVIDIA_VISIBLE_DEVICES and trigger the NVIDIA container
# runtime (nvidia-container-toolkit / CDI) to inject the chosen GPU + the host's
# libcuda.so. That runtime is a HOST prerequisite — it cannot be installed into
# the image, because libcuda.so is the host driver, matched to the host kernel.
#
# --ipc=host is ALSO required at run time: it shares the host IPC namespace so
# (a) the server can open this container's CUDA IPC handles and (b) this
# container sees the server's /dev/shm mailbox file.
LABEL org.certus.gpu-required="true" \
      org.certus.host-prereq="nvidia-container-toolkit (docker --gpus / podman CDI); --ipc=host"

WORKDIR /workspace

# Connector sources + the shared dataset (paths relative to the repo-root build
# context). torch/numpy/vllm come from the base image. NOTE: no grpcio/protobuf
# install here — the shmq connector's transport is pure ctypes + mmap.
COPY certus-shmq-connector/ /workspace/certus-shmq-connector/
# The shmq driver imports the shared workload modules from the sibling bench dir
# (sys.path insert of ../benchmarks/kv-offload-replay -> /workspace/benchmarks/...):
# run_multiturn_common (engine/telemetry setup), run_multiturn_sync_batched (the
# batched loop) and run_multiturn_async (WORKLOAD_MODE=async) — all three needed.
COPY benchmarks/kv-offload-replay/run_multiturn_common.py /workspace/benchmarks/kv-offload-replay/
COPY benchmarks/kv-offload-replay/run_multiturn_sync_batched.py /workspace/benchmarks/kv-offload-replay/
COPY benchmarks/kv-offload-replay/run_multiturn_async.py /workspace/benchmarks/kv-offload-replay/
# TRACE_OFFLOAD=1 wraps the connector in the generic tracer (tracing_connector.py).
# vLLM resolves kv_connector_module_path="tracing_connector" via importlib in the
# scheduler AND each worker subprocess; those spawned procs do NOT inherit the
# driver's runtime sys.path insert, so the module dir is put on PYTHONPATH below.
COPY benchmarks/kv-offload-replay/tracing_connector.py /workspace/benchmarks/kv-offload-replay/
COPY data/sharegpt_12turn_450.json /workspace/data/sharegpt_12turn_450.json

RUN pip install --no-cache-dir -e /workspace/certus-shmq-connector

COPY certus-shmq-connector/docker-entrypoint.sh /usr/local/bin/certus-entrypoint
RUN chmod +x /usr/local/bin/certus-entrypoint

ENV PYTHONUNBUFFERED=1 \
    PYTHONPATH=/workspace/benchmarks/kv-offload-replay \
    SHM_PATH=/dev/shm/certus-shmq \
    NUM_CONVS=450 \
    MODEL=ibm-granite/granite-4.1-8b \
    DTYPE=bfloat16 \
    SLAB_SIZE_BYTES=2097152 \
    DATASET_PATH=/workspace/data/sharegpt_12turn_450.json \
    WORKLOAD=/workspace/certus-shmq-connector/run_multiturn_shmq_certus.py

# Waits for the SHM_PATH mailbox to reach READY, then runs the workload.
ENTRYPOINT ["/usr/local/bin/certus-entrypoint"]
