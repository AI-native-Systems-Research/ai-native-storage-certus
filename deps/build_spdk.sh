#!/bin/bash
#
# Build SPDK from source.
#
# Source is checked out to ./spdk and installed to ./spdk-build.
# By default builds with --without-crypto. Additional configure
# flags can be passed as arguments to this script.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="${SCRIPT_DIR}/spdk"
INSTALL_DIR="${SCRIPT_DIR}/spdk-build"
SPDK_REPO="https://github.com/spdk/spdk.git"

# Clone if not already present
if [ ! -d "${SRC_DIR}/.git" ]; then
    echo "Cloning SPDK..."
    git clone "${SPDK_REPO}" "${SRC_DIR}"
    cd spdk
    git checkout -b v26.01.x origin/v26.01.x
fi

cd "${SRC_DIR}"

# Initialize submodules (DPDK, isa-l, etc.)
echo "Updating submodules..."
git submodule update --init

# Configure
echo "Configuring SPDK..."
./configure --prefix="${INSTALL_DIR}" --without-crypto "$@"

# Build
echo "Building SPDK ($(nproc) jobs)..."
make -j"$(nproc)"

# Install
echo "Installing to ${INSTALL_DIR}..."
make install

echo "Done. SPDK installed to ${INSTALL_DIR}"
