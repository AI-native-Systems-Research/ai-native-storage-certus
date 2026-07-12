#!/bin/bash
# setup-host.sh — one-time host provisioning for the GPU half of the
# certus-grpc-connector container (RHEL/Fedora, root).
#
# Containers bundle userspace, but the NVIDIA driver userspace (libcuda.so) is
# host-injected at run time by the NVIDIA container runtime — it cannot live in
# the image (it must match the host kernel module). This script installs that
# runtime and generates the CDI spec podman uses to expose GPUs.
#
# This covers ONLY the GPU prerequisite. The SPDK/NVMe/hugepage prep for the
# separately-run certus-server is handled by the repo's tools/configure-bench.sh.
#
# Usage:  sudo ./setup-host.sh
#
# Idempotent: re-running is safe (skips install if already present, regenerates
# the CDI spec).
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "error: must run as root (sudo ./setup-host.sh)" >&2
    exit 1
fi

echo "== 1. NVIDIA driver present? =="
if ! command -v nvidia-smi >/dev/null 2>&1 || ! nvidia-smi -L >/dev/null 2>&1; then
    echo "error: nvidia-smi not working — install/enable the NVIDIA GPU driver first." >&2
    echo "       (The container toolkit needs a working host driver.)" >&2
    exit 1
fi
nvidia-smi -L

echo "== 2. Install nvidia-container-toolkit (dnf) =="
if command -v nvidia-ctk >/dev/null 2>&1; then
    echo "  nvidia-ctk already installed ($(nvidia-ctk --version 2>/dev/null | head -1))"
else
    # NVIDIA libnvidia-container repo (RHEL/Fedora). Uses dnf.
    curl -s -L https://nvidia.github.io/libnvidia-container/stable/rpm/nvidia-container-toolkit.repo \
        -o /etc/yum.repos.d/nvidia-container-toolkit.repo
    dnf install -y nvidia-container-toolkit
fi

echo "== 3. Generate CDI spec for podman (rootless uses CDI, not --gpus) =="
# podman resolves `--device nvidia.com/gpu=<id>` from a CDI spec. Regenerate it
# so it reflects the current GPUs/driver.
mkdir -p /etc/cdi
nvidia-ctk cdi generate --output=/etc/cdi/nvidia.yaml
echo "  wrote /etc/cdi/nvidia.yaml"

echo "== 4. Verify: devices podman can now request =="
nvidia-ctk cdi list 2>/dev/null | grep -E "nvidia.com/gpu" || {
    echo "  warning: no nvidia.com/gpu devices listed — check the CDI spec." >&2
}

cat <<'EOF'

Done. GPU prerequisite is installed.

Next:
  * Server (SPDK/NVMe/hugepages) — separate prep:  sudo tools/configure-bench.sh certus
  * Then launch the workload container:            ./certus-grpc-connector/run-bench.sh
EOF
