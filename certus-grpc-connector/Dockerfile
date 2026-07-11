# Multi-stage combined image: builds SPDK + certus-server in a throwaway
# builder, then ships only the binary (+ the gRPC connector) on the vLLM CUDA
# base. This runs the 450-conv / 12-turn workload end to end against a
# certus-server it launches internally on the same NVMe drives that
# certus-connector/run_multiturn_certus.py uses (0000:c1-c4:00.0).
#
# Why multi-stage: SPDK/DPDK link STATICALLY into certus-server
# (spdk-sys/build.rs uses static:+whole-archive for every SPDK/DPDK lib), so
# the runtime needs only the binary plus a few system .so's (libnuma, libuuid,
# libfuse3; cudart/ssl/crypto/pthread come from the vLLM base). The compilers,
# Rust toolchain, SPDK/DPDK source trees and cargo target/ (~10-15 GB) stay in
# the builder and never reach the final image.
#
# Build (context = repo root):
#   podman build -f certus-grpc-connector/Dockerfile -t certus-grpc-bench .
#
# Run:
#   podman run --rm --privileged --gpus all --ipc=host \
#       -v /dev/hugepages:/dev/hugepages \
#       -v $HOME/.cache/huggingface:/root/.cache/huggingface \
#       -e HF_TOKEN=<token> \
#       certus-grpc-bench
#
# Requires the c1-c4 NVMe drives bound to vfio-pci and host hugepages allocated.

# ─────────────────────────── Stage 1: builder ───────────────────────────
FROM docker.io/vllm/vllm-openai:v0.20.0 AS builder
SHELL ["/bin/bash", "-lc"]

# System deps to build SPDK + the CUDA-linked Rust workspace (apt equivalents
# of deps/install_deps.sh; the base is Ubuntu-derived).
RUN apt-get update && apt-get install -y --no-install-recommends \
        git curl build-essential clang libclang-dev pkg-config \
        libnuma-dev libaio-dev uuid-dev libssl-dev \
        libncurses-dev libcunit1-dev libfuse3-dev python3-pip \
        patchelf meson ninja-build \
    && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir pyelftools jinja2 tabulate

# Rust toolchain (workspace MSRV 1.75+).
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal

WORKDIR /src
COPY . /src

# Build SPDK (static libs that spdk-sys links) then the server (release).
RUN deps/build_spdk.sh
RUN cargo build --release -p certus-server \
    && cp /src/target/release/certus-server /certus-server \
    && strip /certus-server || true

# ─────────────────────────── Stage 2: runtime ───────────────────────────
FROM docker.io/vllm/vllm-openai:v0.20.0
ENTRYPOINT []
SHELL ["/bin/bash", "-lc"]

# Only the runtime .so's the statically-linked binary still needs dynamically
# (cudart/ssl/crypto/pthread/dl/m are already in the vLLM base).
RUN apt-get update && apt-get install -y --no-install-recommends \
        libnuma1 libuuid1 libfuse3-3 \
    && rm -rf /var/lib/apt/lists/*

# The server binary (SPDK/DPDK baked in) + the pure-Python connector sources.
COPY --from=builder /certus-server /usr/local/bin/certus-server
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
    SERVER_BIN=/usr/local/bin/certus-server \
    DATASET_PATH=/workspace/certus-connector/sharegpt_12turn_450.json \
    # SPDK disks + tier config mirroring run_multiturn_certus.py:
    DEVICE_PCI="0000:c1:00.0 0000:c2:00.0 0000:c3:00.0 0000:c4:00.0" \
    MEMORY_TIER_SIZE=44G \
    SLAB_SIZE_BYTES=2097152 \
    POLLER_BASE_CPU=1

ENTRYPOINT ["/usr/local/bin/certus-entrypoint"]
