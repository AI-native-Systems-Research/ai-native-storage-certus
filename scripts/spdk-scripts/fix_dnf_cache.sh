#!/usr/bin/env bash
# fix_dnf_cache.sh
# Diagnose and attempt to fix common DNF/yum cache repo I/O issues.
# Usage: sudo ./fix_dnf_cache.sh [--yes]

set -euo pipefail
CACHE_DIR=/var/cache/dnf
AUTO_YES=0

for arg in "$@"; do
    case "$arg" in
    -y | --yes) AUTO_YES=1 ;;
    -h | --help)
        cat <<EOF
Usage: $0 [--yes]

This script will:
 - Run diagnostics on ${CACHE_DIR}
 - Try to repair ownership/permissions
 - Clean and (if possible) remove stale cache contents
 - Rebuild the DNF metadata cache with 'dnf makecache'
 - Optionally install a set of development packages required by bindgen

Pass --yes to skip the interactive confirmation for the package install step.
EOF
        exit 0
        ;;
    *)
        echo "Unknown argument: $arg" >&2
        exit 2
        ;;
    esac
done

if command -v sudo >/dev/null 2>&1; then
    SUDO=sudo
else
    SUDO=
fi

echo "== DNF cache repair helper =="
echo "Cache dir: ${CACHE_DIR}"

# Diagnostics
echo "\n-- Diagnostics --"
mount | grep " ${CACHE_DIR} " || true
if [ -d "${CACHE_DIR}" ]; then
    ls -ld "${CACHE_DIR}"
    echo "Disk usage for ${CACHE_DIR}:"
    df -h "${CACHE_DIR}" || true
    echo "Recent kernel messages (tail):"
    dmesg | tail -n 50 || true
else
    echo "${CACHE_DIR} does not exist"
fi

# Test write access
echo "\n-- Test write access --"
TESTFILE="${CACHE_DIR}/.dnf_test_$$"
if $SUDO bash -c "echo test > '${TESTFILE}' 2>/dev/null"; then
    $SUDO rm -f "${TESTFILE}"
    echo "Write test succeeded (cache directory writable)."
else
    echo "Write test failed. Attempting to fix ownership/permissions."
    echo "Creating ${CACHE_DIR} (if missing) and setting mode 0755"
    $SUDO mkdir -p "${CACHE_DIR}"
    $SUDO chown root:root "${CACHE_DIR}" || true
    $SUDO chmod 0755 "${CACHE_DIR}" || true
    # re-test
    if $SUDO bash -c "echo test > '${TESTFILE}' 2>/dev/null"; then
        $SUDO rm -f "${TESTFILE}"
        echo "Write test succeeded after permission fix."
    else
        echo "Write test still failing. Possible I/O error or read-only filesystem."
        echo "Check system logs and disk health; aborting automated fixes."
        exit 1
    fi
fi

# Attempt to run dnf clean
echo "\n-- dnf clean --"
if $SUDO dnf clean all; then
    echo "dnf clean all succeeded."
else
    echo "dnf clean all had errors (will attempt to remove cache subdirs)."
fi

# Remove cache contents cautiously
echo "\n-- Removing cache contents --"
# Try to ensure CACHE_DIR is on a local block device before rm -rf
FS_SOURCE=$(findmnt -n -o SOURCE --target "${CACHE_DIR}" || true)
if [ -n "${FS_SOURCE}" ] && (echo "${FS_SOURCE}" | grep -qE '^/dev/|^tmpfs|^/run/'); then
    echo "Cache appears to be on local device (${FS_SOURCE}). Removing contents..."
    $SUDO rm -rf "${CACHE_DIR}"/* || true
    $SUDO mkdir -p "${CACHE_DIR}"
    $SUDO chown root:root "${CACHE_DIR}" || true
    $SUDO chmod 0755 "${CACHE_DIR}" || true
else
    echo "Cache mount source: ${FS_SOURCE:-<unknown>}"
    echo "Not removing files automatically because cache directory is not clearly on a local device."
fi

# Try to rebuild cache
echo "\n-- Rebuilding DNF cache --"
if $SUDO dnf makecache; then
    echo "dnf makecache completed successfully."
else
    echo "dnf makecache failed. Check network, repos and system logs."
    exit 1
fi

# Optionally install packages
PACKAGES=(clang libclang-devel glibc-headers glibc-devel gcc gcc-c++ make pkgconfig)
if [ "$AUTO_YES" -eq 1 ]; then
    INSTALL_ANS=Y
else
    read -r -p "Install development packages (${PACKAGES[*]}) now? [y/N] " INSTALL_ANS
fi

if [[ "$INSTALL_ANS" =~ ^[Yy]$ ]]; then
    echo "Installing packages: ${PACKAGES[*]}"
    $SUDO dnf install -y "${PACKAGES[@]}"
    echo "Package install finished."
else
    echo "Skipping package install."
fi

echo "\n== Done =="
exit 0
