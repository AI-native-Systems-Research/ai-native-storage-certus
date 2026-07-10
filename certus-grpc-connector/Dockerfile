# vLLM client image for the Certus gRPC connector.
#
# This image runs the vLLM side of the multi-turn workload: it loads the model,
# uses CertusGrpcOffloadingSpec to offload KV blocks over gRPC to a *separately
# running* certus-server (the server needs SPDK + NVMe bound to vfio-pci +
# hugepages, which live on the host, not in this container).
#
# Build:
#   docker build -f certus-grpc-connector/Dockerfile -t certus-grpc-bench .
#
# Run (against a server reachable at $CERTUS_SERVER; needs GPU + IPC to the
# server's CUDA context, so share the host IPC namespace and GPUs):
#   docker run --rm --gpus all --ipc=host \
#       -e CERTUS_SERVER=<host>:50051 \
#       -e HF_TOKEN=<token> \
#       -v $HOME/.cache/huggingface:/root/.cache/huggingface \
#       certus-grpc-bench
#
# The build context must be the repo root (the image copies both
# certus-grpc-connector/ and the dataset under certus-connector/).

# vLLM 0.20.0 ships a matching CUDA/torch stack; use its published image as the
# base so we don't fight torch/CUDA version drift.
# Fully-qualified registry name so both docker and podman (which won't prompt
# for short-name resolution) can pull it.
FROM docker.io/vllm/vllm-openai:v0.20.0

# vllm/vllm-openai sets an ENTRYPOINT for the OpenAI server; we drive python
# directly instead.
ENTRYPOINT []

WORKDIR /workspace

# Connector sources + the shared dataset (paths relative to the repo-root build
# context).
COPY certus-grpc-connector/ /workspace/certus-grpc-connector/
COPY certus-connector/sharegpt_12turn_450.json /workspace/certus-connector/sharegpt_12turn_450.json

# grpc runtime + build tools for stub regeneration (stubs are already checked
# in, but keep tooling available). torch/numpy/vllm come from the base image.
RUN pip install --no-cache-dir grpcio>=1.60.0 grpcio-tools>=1.60.0 protobuf>=4.25.0 \
    && pip install --no-cache-dir -e /workspace/certus-grpc-connector

ENV PYTHONUNBUFFERED=1 \
    CERTUS_SERVER=localhost:50051 \
    NUM_CONVS=450 \
    MODEL=NousResearch/Meta-Llama-3-8B

# Default: the 450-conv / 12-turn multi-turn workload against the gRPC connector.
CMD ["python3", "/workspace/certus-grpc-connector/run_multiturn_grpc_certus.py"]
