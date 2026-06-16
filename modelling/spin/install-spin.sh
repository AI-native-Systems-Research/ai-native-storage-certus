#!/usr/bin/env bash
set -euo pipefail

REPO_URL="https://github.com/dwaddington/Spin.git"
INSTALL_PREFIX="${INSTALL_PREFIX:-/usr/local}"
BUILD_DIR="${BUILD_DIR:-$(mktemp -d)}"
BRANCH="${BRANCH:-master}"

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Download, build, and install Spin from $REPO_URL

Options:
  --prefix DIR    Install prefix (default: /usr/local)
  --branch NAME   Git branch to checkout (default: master)
  --build-dir DIR Temporary build directory (default: auto)
  --help          Show this help message

Environment variables:
  INSTALL_PREFIX  Same as --prefix
  BUILD_DIR       Same as --build-dir
  BRANCH          Same as --branch

Examples:
  $(basename "$0")                          # Install to /usr/local
  $(basename "$0") --prefix \$HOME/.local   # Install to ~/.local
EOF
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)   INSTALL_PREFIX="$2"; shift 2 ;;
        --branch)   BRANCH="$2"; shift 2 ;;
        --build-dir) BUILD_DIR="$2"; shift 2 ;;
        --help)     usage ;;
        *)          echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

echo "==> Cloning Spin from $REPO_URL (branch: $BRANCH)"
git clone --depth 1 --branch "$BRANCH" "$REPO_URL" "$BUILD_DIR/Spin"

echo "==> Building Spin"
cd "$BUILD_DIR/Spin/Src"
make

echo "==> Installing spin to $INSTALL_PREFIX/bin"
install -d "$INSTALL_PREFIX/bin"
install -m 755 spin "$INSTALL_PREFIX/bin/spin"

echo "==> Cleaning up build directory"
rm -rf "$BUILD_DIR/Spin"

echo "==> Done. Spin installed at $INSTALL_PREFIX/bin/spin"
spin -V 2>/dev/null && echo "==> Version: $(spin -V)" || true
