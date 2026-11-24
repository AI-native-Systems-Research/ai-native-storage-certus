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

# Patch DPDK memory caps so a single spdk_zmalloc can exceed the stock 32 GiB
# per-memseg-list cap. The Certus DRAM tier does one large spdk_zmalloc for the
# whole pool, so two limits bind:
#   RTE_MAX_MEM_MB_PER_LIST  — max MiB in one memseg list (one allocation).
#   RTE_MAX_MEM_MB_PER_TYPE  — max MiB for one (page-size + NUMA-node) type; this
#                              is the real ceiling on a single-node tier.
# Both are raised to 266240 (260 GiB) so a 260 GiB tier fits on one node.
# NOTE: use 1 GiB hugepages (260 segments for a 260 GiB tier); with 2 MiB
# hugepages the segment count exceeds RTE_MAX_MEMSEG_PER_LIST (8192) and the
# allocation fails. deps/spdk is gitignored, so patch here to keep it
# reproducible.
#
# A third limit, RTE_MAX_MEM_MB (global, 524288 = 512 GiB on x86, set in
# dpdk/config/meson.build rather than rte_config.h), also caps each type:
#   max_mem_per_type = MIN(RTE_MAX_MEM_MB_PER_TYPE, RTE_MAX_MEM_MB / n_memtypes)
# where n_memtypes = hugepage sizes x NUMA nodes. On a 2-socket host with both
# 2 MiB and 1 GiB hugepage sizes that is 512 GiB / 4 = 128 GiB per type, so a
# 128 GiB tier (plus DPDK's own heap) cannot fit. Raise it to 1 TiB so the
# per-type cap is 256 GiB on such hosts.
TIER_CAP_MB=266240
GLOBAL_CAP_MB=1048576
DPDK_MESON_CONFIG="${SRC_DIR}/dpdk/config/meson.build"
if [ -f "${DPDK_MESON_CONFIG}" ]; then
    cur="$(sed -n "s/^ *dpdk_conf.set('RTE_MAX_MEM_MB', \([0-9]*\)).*/\1/p" "${DPDK_MESON_CONFIG}" | sort -n | tail -1)"
    if [ -z "${cur}" ]; then
        echo "WARNING: RTE_MAX_MEM_MB not found in ${DPDK_MESON_CONFIG} — DPDK layout changed, not patching."
    elif [ "${cur}" -lt "${GLOBAL_CAP_MB}" ]; then
        echo "Patching RTE_MAX_MEM_MB ${cur} -> ${GLOBAL_CAP_MB} (per-type cap = this / n_memtypes)..."
        sed -i "s/dpdk_conf.set('RTE_MAX_MEM_MB', ${cur})/dpdk_conf.set('RTE_MAX_MEM_MB', ${GLOBAL_CAP_MB})/" "${DPDK_MESON_CONFIG}"
    else
        echo "RTE_MAX_MEM_MB already >= ${GLOBAL_CAP_MB} (${cur}) — leaving as-is."
    fi
fi
DPDK_RTE_CONFIG="${SRC_DIR}/dpdk/config/rte_config.h"
if [ -f "${DPDK_RTE_CONFIG}" ]; then
    for key in RTE_MAX_MEM_MB_PER_LIST RTE_MAX_MEM_MB_PER_TYPE; do
        cur="$(sed -n "s/^#define ${key} \([0-9]*\)/\1/p" "${DPDK_RTE_CONFIG}")"
        if [ -z "${cur}" ]; then
            echo "WARNING: ${key} not found in ${DPDK_RTE_CONFIG} — DPDK layout changed, not patching."
        elif [ "${cur}" -lt "${TIER_CAP_MB}" ]; then
            echo "Patching ${key} ${cur} -> ${TIER_CAP_MB} (allow up to 128G single-node tier)..."
            sed -i "s/^#define ${key} ${cur}/#define ${key} ${TIER_CAP_MB}/" "${DPDK_RTE_CONFIG}"
        else
            echo "${key} already >= ${TIER_CAP_MB} (${cur}) — leaving as-is."
        fi
    done
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
