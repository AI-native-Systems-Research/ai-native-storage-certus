# Dockerfile for certus-server-yaml (SPDK + RDMA)
#
# Single-stage build: builds SPDK, then the Rust application.
# Rocky Linux 9 is binary-compatible with RHEL 9 and freely available.

FROM rockylinux:9 AS builder

# Install system dependencies for SPDK and Rust build
RUN dnf install -y epel-release && \
    dnf config-manager --set-enabled crb && \
    dnf install -y \
        fuse3-devel fuse3-libs \
        numactl-libs numactl numactl-devel \
        libuuid-devel libaio-devel \
        ncurses-devel openssl-devel \
        clang clang-devel \
        glibc-headers glibc-devel \
        gcc gcc-c++ make pkgconfig \
        CUnit CUnit-devel \
        python3-pip patchelf \
        rdma-core-devel libibverbs librdmacm \
        git nasm autoconf automake libtool unzip protobuf-compiler && \
    dnf clean all

# CUDA toolkit (build-time only; runtime uses nvidia-container-toolkit to inject host libs)
RUN dnf config-manager --add-repo https://developer.download.nvidia.com/compute/cuda/repos/rhel9/x86_64/cuda-rhel9.repo && \
    dnf install -y cuda-cudart-devel-12-6 cuda-nvcc-12-6 && \
    dnf clean all
ENV PATH="/usr/local/cuda-12.6/bin:${PATH}"

# GDRCopy (build-time stub — the real library is on the host)
RUN mkdir -p /usr/local/gdrcopy/lib && \
    echo 'void gdr_open(){}' | gcc -shared -x c - -o /usr/local/gdrcopy/lib/libgdrapi.so && \
    mkdir -p /usr/local/gdrcopy/include && \
    printf '#ifndef GDRAPI_H\n#define GDRAPI_H\ntypedef void* gdr_t;\n#endif\n' > /usr/local/gdrcopy/include/gdrapi.h

# Python tools needed by SPDK build (meson/ninja)
RUN pip3 install meson jinja2 pyelftools tabulate ninja uv

# Install Rust toolchain
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /build

# Copy the full source tree
COPY . .

# Build SPDK
RUN cd deps && bash build_spdk.sh

# Build certus-server-yaml (spdk feature is the default)
RUN cargo build --release -p certus-server-yaml

# --- Runtime stage ---
FROM rockylinux:9

RUN dnf install -y epel-release && \
    dnf config-manager --add-repo https://developer.download.nvidia.com/compute/cuda/repos/rhel9/x86_64/cuda-rhel9.repo && \
    dnf install -y \
        numactl-libs numactl \
        libuuid libaio \
        openssl-libs \
        fuse3-libs \
        rdma-core libibverbs librdmacm \
        pciutils \
        cuda-cudart-12-6 && \
    dnf clean all

# GDRCopy stub (certus links against it but only calls it when GPU-direct is active)
COPY --from=builder /usr/local/gdrcopy/lib/libgdrapi.so /usr/local/lib64/libgdrapi.so
RUN ldconfig

# Tell nvidia-container-toolkit to inject CUDA libraries from the host
ENV NVIDIA_VISIBLE_DEVICES=all
ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility

# Copy the built binary
COPY --from=builder /build/target/release/certus-server-yaml /usr/local/bin/certus-server-yaml

# Copy SPDK scripts/tools that may be needed at runtime (setup.sh for hugepages/vfio)
COPY --from=builder /build/deps/spdk/scripts/setup.sh /opt/spdk/scripts/setup.sh
COPY --from=builder /build/deps/spdk/scripts/common.sh /opt/spdk/scripts/common.sh

ENTRYPOINT ["certus-server-yaml"]
