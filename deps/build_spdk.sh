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
    cd "${SRC_DIR}"   
    git checkout -b v26.01.x origin/v26.01.x
fi

cd "${SRC_DIR}"

# Initialize submodules (DPDK, isa-l, etc.)
echo "Updating submodules..."
git submodule update --init

# Patch DPDK memseg limit so a single spdk_zmalloc can exceed the stock 32 GiB
# per-memseg-list cap (RTE_MAX_MEM_MB_PER_LIST). The Certus DRAM tier does one
# large spdk_zmalloc for the whole pool; without this a >32 GiB tier fails.
# deps/spdk is gitignored, so apply the patch here to keep it reproducible.
DPDK_RTE_CONFIG="${SRC_DIR}/dpdk/config/rte_config.h"
if [ -f "${DPDK_RTE_CONFIG}" ]; then
    if grep -q '#define RTE_MAX_MEM_MB_PER_LIST 32768' "${DPDK_RTE_CONFIG}"; then
        echo "Patching RTE_MAX_MEM_MB_PER_LIST 32768 -> 65536 (allow >32G single alloc)..."
        sed -i 's/#define RTE_MAX_MEM_MB_PER_LIST 32768/#define RTE_MAX_MEM_MB_PER_LIST 65536/' "${DPDK_RTE_CONFIG}"
    else
        echo "RTE_MAX_MEM_MB_PER_LIST already patched (or unexpected value) — leaving as-is."
    fi
fi

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
