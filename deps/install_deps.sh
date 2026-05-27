#!/bin/bash
#
# Install all system and Python dependencies required to build SPDK.
#
# Run order (from the repo root):
#   1. deps/install_deps.sh   — install system packages and Python tools (this script)
#   2. deps/build_spdk.sh     — clone, build, and install SPDK to deps/spdk-build/
#
# Prerequisites (RHEL 9):
#   - CodeReady Builder repo must be enabled for CUnit-devel:
#       sudo subscription-manager repos --enable codeready-builder-for-rhel-9-x86_64-rpms
#     Or if using CRB alias:
#       sudo dnf config-manager --set-enabled crb
#

# Enable CRB repo for CUnit-devel (no-op if already enabled)
sudo dnf config-manager --set-enabled crb 2>/dev/null || true

sudo dnf install -y fuse3-devel fuse3-libs numactl-libs numactl numactl-devel libuuid-devel libaio-devel ncurses-devel openssl-devel
sudo dnf install -y clang clang-devel glibc-headers glibc-devel gcc gcc-c++ make pkgconfig CUnit CUnit-devel
sudo dnf install -y python3-pip patchelf

# CUDA toolkit (required for gpu-services component)
sudo dnf install -y cuda-toolkit

pip install meson jinja2 pyelftools tabulate ninja uv
sudo ln -sf "$(python3 -c 'import shutil; print(shutil.which("meson"))')" /usr/bin/meson
sudo ln -sf "$(python3 -c 'import shutil; print(shutil.which("ninja"))')" /usr/bin/ninja
