# vLLM workload-driver image for the Certus gRPC connector.
#
# This image is the CLIENT side only: vLLM + the pure-Python gRPC connector +
# the multi-turn workload driver. It offloads KV blocks over gRPC to a
# SEPARATELY-RUN certus-server (which owns SPDK/NVMe/hugepages on the host).
# The server is intentionally NOT built into this image — run it on the host
# (cargo build -p certus-server after deps/build_spdk.sh) and point this image
# at it via CERTUS_SERVER.
#
# Build (context = repo root — needs certus-grpc-connector/ + the dataset):
#   podman build -f certus-grpc-connector/Dockerfile -t certus-grpc-bench .
#
# Run (server reachable at CERTUS_SERVER; --ipc=host so the server can open the
# CUDA IPC handles this container's vLLM process exports):
#   podman run --rm --gpus all --ipc=host \
#       -e CERTUS_SERVER=<host>:50051 \
#       -e HF_TOKEN=<token> \
#       -v $HOME/.cache/huggingface:/root/.cache/huggingface \
#       certus-grpc-bench

FROM docker.io/vllm/vllm-openai:v0.20.0
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
LABEL org.certus.gpu-required="true" \
      org.certus.host-prereq="nvidia-container-toolkit (docker --gpus / podman CDI)"

WORKDIR /workspace

# Connector sources + the shared dataset (paths relative to the repo-root build
# context). torch/numpy/vllm come from the base image.
COPY certus-grpc-connector/ /workspace/certus-grpc-connector/
COPY certus-connector/sharegpt_12turn_450.json /workspace/certus-connector/sharegpt_12turn_450.json

RUN pip install --no-cache-dir grpcio>=1.60.0
RUN pip install --no-cache-dir protobuf>=4.25.0

RUN pip install --no-cache-dir -e /workspace/certus-grpc-connector

COPY certus-grpc-connector/docker-entrypoint.sh /usr/local/bin/certus-entrypoint
RUN chmod +x /usr/local/bin/certus-entrypoint

ENV PYTHONUNBUFFERED=1 \
    CERTUS_SERVER=localhost:50051 \
    NUM_CONVS=450 \
    MODEL=NousResearch/Meta-Llama-3-8B \
    SLAB_SIZE_BYTES=2097152 \
    DATASET_PATH=/workspace/certus-connector/sharegpt_12turn_450.json \
    WORKLOAD=/workspace/certus-grpc-connector/run_multiturn_grpc_certus.py

# Waits for CERTUS_SERVER to accept connections, then runs the workload.
ENTRYPOINT ["/usr/local/bin/certus-entrypoint"]
