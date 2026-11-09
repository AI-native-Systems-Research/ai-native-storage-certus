#!/bin/bash
#
# Install system prerequisites for building zyre and its dependencies.
# Targets RHEL/Fedora systems.
#
set -euo pipefail

echo "Installing zyre build prerequisites..."

if command -v dnf &>/dev/null; then
    sudo dnf install -y \
        cmake \
        gcc \
        gcc-c++ \
        make \
        pkg-config \
        libtool \
        autoconf \
        automake \
        clang-devel \
        libuuid-devel
elif command -v yum &>/dev/null; then
    sudo yum install -y \
        cmake \
        gcc \
        gcc-c++ \
        make \
        pkgconfig \
        libtool \
        autoconf \
        automake \
        clang-devel \
        libuuid-devel
else
    echo "ERROR: Neither dnf nor yum found. This script requires RHEL/Fedora." >&2
    exit 1
fi

echo "Done. Prerequisites installed."
