# Combined image: builds SPDK + certus-server + the gRPC connector on the
# vLLM CUDA base, then runs the 450-conv / 12-turn workload against a
# certus-server it launches internally on the same NVMe drives that
# certus-connector/run_multiturn_certus.py uses (0000:c1-c4:00.0).
#
# The server needs real hardware access, so the container must run with:
#   --privileged (or --cap-add SYS_ADMIN + --device /dev/vfio)
#   --gpus all --ipc=host
#   -v /dev/hugepages:/dev/hugepages  (host hugepages for the DRAM tier)
# and the c1-c4 NVMe drives must be bound to vfio-pci on the host first.
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
# NOTE: this image compiles SPDK (v26.01.x) and the Rust workspace; the build
# is large and long. It could not be built in the original dev environment
# (SPDK absent, limited disk) — it targets a provisioned GPU+NVMe host.

FROM docker.io/vllm/vllm-openai:v0.20.0
ENTRYPOINT []
SHELL ["/bin/bash", "-lc"]

# ── System deps for SPDK + Rust + the CUDA-linked components ──
# (Base is Ubuntu-derived; install the apt equivalents of deps/install_deps.sh.)
RUN apt-get update && apt-get install -y --no-install-recommends \
        git curl build-essential clang libclang-dev pkg-config \
        libnuma-dev libaio-dev libuuid1 uuid-dev libssl-dev \
        libncurses-dev libcunit1-dev libfuse3-dev python3-pip \
        patchelf meson ninja-build \
    && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir pyelftools jinja2 tabulate

# ── Rust toolchain (workspace MSRV 1.75+) ──
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal

WORKDIR /src
# Copy the whole repo (needed: components/, apps/, deps/ scripts). The .git and
# heavy build artifacts are excluded via .dockerignore.
COPY . /src

# ── Build SPDK into deps/spdk-build (what spdk-sys/build.rs links against) ──
RUN deps/build_spdk.sh

# ── Build the server (SPDK + CUDA/gpu feature) ──
RUN cargo build --release -p certus-server

# ── Install the pure-Python gRPC connector into the vLLM env ──
RUN pip install --no-cache-dir grpcio>=1.60.0 protobuf>=4.25.0 \
    && pip install --no-cache-dir -e /src/certus-grpc-connector

COPY certus-grpc-connector/docker-entrypoint.sh /usr/local/bin/certus-entrypoint
RUN chmod +x /usr/local/bin/certus-entrypoint

ENV PYTHONUNBUFFERED=1 \
    CERTUS_SERVER=localhost:50051 \
    NUM_CONVS=450 \
    MODEL=NousResearch/Meta-Llama-3-8B \
    # SPDK disks + tier config mirroring run_multiturn_certus.py:
    DEVICE_PCI="0000:c1:00.0 0000:c2:00.0 0000:c3:00.0 0000:c4:00.0" \
    MEMORY_TIER_SIZE=44G \
    SLAB_SIZE_BYTES=2097152 \
    POLLER_BASE_CPU=1

ENTRYPOINT ["/usr/local/bin/certus-entrypoint"]
