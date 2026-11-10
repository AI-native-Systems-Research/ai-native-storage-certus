#!/bin/bash
#
# Build libzmq, czmq, and zyre from source.
#
# Sources are cloned into ./libzmq, ./czmq, ./zyre and installed to ./zyre-build.
# This script is idempotent — re-running will skip already-cloned repos and
# only rebuild if the install directory is missing.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="${SCRIPT_DIR}/zyre-build"

LIBZMQ_REPO="https://github.com/zeromq/libzmq.git"
LIBZMQ_TAG="v4.3.5"
LIBZMQ_SRC="${SCRIPT_DIR}/libzmq"

CZMQ_REPO="https://github.com/zeromq/czmq.git"
CZMQ_TAG="v4.2.1"
CZMQ_SRC="${SCRIPT_DIR}/czmq"

ZYRE_REPO="https://github.com/zeromq/zyre.git"
ZYRE_TAG="v2.0.1"
ZYRE_SRC="${SCRIPT_DIR}/zyre"

NPROC=$(nproc 2>/dev/null || echo 4)

clone_if_needed() {
    local repo="$1" tag="$2" dir="$3"
    if [ ! -d "${dir}/.git" ]; then
        echo "Cloning ${repo} (${tag})..."
        git clone --branch "${tag}" --depth 1 "${repo}" "${dir}"
    else
        echo "Already cloned: ${dir}"
    fi
}

build_cmake_project() {
    local src="$1" name="$2"
    local build_dir="${src}/build"

    echo "Building ${name}..."
    mkdir -p "${build_dir}"
    cd "${build_dir}"
    cmake .. \
        -DCMAKE_INSTALL_PREFIX="${INSTALL_DIR}" \
        -DCMAKE_PREFIX_PATH="${INSTALL_DIR}" \
        -DCMAKE_BUILD_TYPE=Release \
        -DBUILD_TESTING=OFF
    make -j"${NPROC}"
    make install
    cd "${SCRIPT_DIR}"
}

# Clone all repos
clone_if_needed "${LIBZMQ_REPO}" "${LIBZMQ_TAG}" "${LIBZMQ_SRC}"
clone_if_needed "${CZMQ_REPO}" "${CZMQ_TAG}" "${CZMQ_SRC}"
clone_if_needed "${ZYRE_REPO}" "${ZYRE_TAG}" "${ZYRE_SRC}"

# Build in dependency order: libzmq → czmq → zyre
export PKG_CONFIG_PATH="${INSTALL_DIR}/lib/pkgconfig:${INSTALL_DIR}/lib64/pkgconfig:${PKG_CONFIG_PATH:-}"

build_cmake_project "${LIBZMQ_SRC}" "libzmq"
build_cmake_project "${CZMQ_SRC}" "czmq"
build_cmake_project "${ZYRE_SRC}" "zyre"

echo ""
echo "Done. Libraries installed to: ${INSTALL_DIR}"
echo "  Headers: ${INSTALL_DIR}/include/"
echo "  Libs:    ${INSTALL_DIR}/lib/"
