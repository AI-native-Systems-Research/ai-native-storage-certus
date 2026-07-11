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

WORKDIR /workspace

# Connector sources + the shared dataset (paths relative to the repo-root build
# context). torch/numpy/vllm come from the base image.
COPY certus-grpc-connector/ /workspace/certus-grpc-connector/
COPY certus-connector/sharegpt_12turn_450.json /workspace/certus-connector/sharegpt_12turn_450.json

RUN pip install --no-cache-dir grpcio>=1.60.0 protobuf>=4.25.0 \
    && pip install --no-cache-dir -e /workspace/certus-grpc-connector

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
