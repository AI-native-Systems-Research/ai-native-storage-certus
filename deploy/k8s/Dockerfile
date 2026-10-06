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

# Build libzmq -> czmq -> zyre into deps/zyre-build. The remote-lookup transport
# uses zyre for peer discovery (group SHOUT / directed WHISPER), so the
# full-remote build below links against it. deps/install_zyre_deps.sh calls sudo,
# which does not exist in a build stage, so its package list is inlined.
RUN dnf install -y cmake gcc gcc-c++ make pkg-config libtool autoconf automake \
        clang-devel libuuid-devel && dnf clean all
RUN cd deps && bash build_zyre.sh

# Build certus-server-yaml with the FULL-REMOTE profile.
#
# The previous `cargo build --release -p certus-server-yaml` took the default
# feature set, which is just ["spdk"]. That omits zyre and the RDMA
# initiator/responder, so the resulting server still ACCEPTS --rl-group but has
# no remote-lookup transport behind it -- instances silently never exchange
# entries. The initiator/responder also need their OWN `rdma` features, or they
# compile in stub mode rather than driving real hardware RDMA.
#
# Mirrors scripts/build-certus-full-remote-spdk.sh -- keep the two in step.
# Exposed as build args so build-image-and-push.sh can override them.
ARG CERTUS_PROFILE=full-remote
ARG CERTUS_FEATURES="spdk,rdma,remote-lookup-rdma-initiator/rdma,remote-lookup-rdma-responder/rdma"
ENV PKG_CONFIG_PATH=/build/deps/zyre-build/lib/pkgconfig:/build/deps/zyre-build/lib64/pkgconfig
ENV LD_LIBRARY_PATH=/build/deps/zyre-build/lib:/build/deps/zyre-build/lib64
RUN CERTUS_PROFILE="${CERTUS_PROFILE}" cargo build --release \
        -p certus-server-yaml \
        --features "${CERTUS_FEATURES}"

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

# libzmq / czmq / zyre for the remote-lookup transport. Copied wholesale from the
# builder because RHEL9 has no packages at these pinned versions (libzmq v4.3.5,
# czmq v4.2.1, zyre v2.0.1). Registered via ld.so.conf rather than
# LD_LIBRARY_PATH so the binary resolves them however it is invoked -- `ldd` on a
# host-built server shows libzyre.so.2/libczmq.so.4 as "not found" precisely
# because that build relies on the caller exporting LD_LIBRARY_PATH.
COPY --from=builder /build/deps/zyre-build/ /opt/zyre-build/
RUN printf '/opt/zyre-build/lib\n/opt/zyre-build/lib64\n' > /etc/ld.so.conf.d/zyre.conf && \
    ldconfig

# Copy the built binary
COPY --from=builder /build/target/release/certus-server-yaml /usr/local/bin/certus-server-yaml

# Copy SPDK scripts/tools that may be needed at runtime (setup.sh for hugepages/vfio)
COPY --from=builder /build/deps/spdk/scripts/setup.sh /opt/spdk/scripts/setup.sh
COPY --from=builder /build/deps/spdk/scripts/common.sh /opt/spdk/scripts/common.sh

ENTRYPOINT ["certus-server-yaml"]
