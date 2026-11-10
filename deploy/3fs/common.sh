# common.sh — podman store selection shared by every script in deploy/3fs (sourced).
#
# STORE_DIR puts podman's image/container storage, its runroot, and its temp files
# (layer tars written during build/load/save) under one folder instead of the
# defaults on the root fs (/var/lib/containers, ~/.local/share/containers, /var/tmp):
#
#   STORE_DIR=/home/dwaddington/podman-3fs ./build.sh
#   sudo STORE_DIR=/home/podman-3fs-root ./run.sh
#
# Rootless (build.sh, smoke.sh) and rootful (run.sh, admin.sh, teardown.sh) podman
# cannot share one store, so use a different STORE_DIR for each — run.sh copies the
# image from the builder's store (BUILD_STORE_DIR) into its own on first use.
#
# A ROOTLESS store outside the default location uses fuse-overlayfs: on this host
# /etc/containers/storage.conf sets overlay mountopt "nodev,metacopy=on", and an
# unprivileged user may not mount kernel overlay with metacopy on a new store
# ("failed to mount overlay for metacopy check ... permission denied"). The
# default rootless store is unaffected (it was created before that setting).
# Every rootful script must be given the SAME STORE_DIR, or it will not find the
# container (run.sh saves its choice so admin.sh/teardown.sh pick it up).
#
# Unset = podman's default store.

_3FS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
_3FS_SAVED="$_3FS_DIR/.store-dir.root"

# Rootful scripts default to whatever run.sh last used.
if [[ -z "${STORE_DIR:-}" && $EUID -eq 0 && -f "$_3FS_SAVED" ]]; then
  STORE_DIR="$(cat "$_3FS_SAVED")"
fi

store_flags() {  # store_flags <dir> [rootless] — echo --root/--runroot for a store dir (or nothing)
  [[ -z "$1" ]] && return 0
  printf -- '--root\n%s\n--runroot\n%s\n' "$1/storage" "$1/run"
  # Rootless: kernel overlay + metacopy is refused on a fresh store; fuse-overlayfs works.
  if [[ "${2:-}" == rootless || $EUID -ne 0 ]] && command -v fuse-overlayfs >/dev/null; then
    printf -- '--storage-opt\noverlay.mount_program=%s\n' "$(command -v fuse-overlayfs)"
  fi
}

PODMAN=(podman)
if [[ -n "${STORE_DIR:-}" ]]; then
  mkdir -p "$STORE_DIR"/{storage,run,tmp}
  mapfile -t _flags < <(store_flags "$STORE_DIR")
  PODMAN+=("${_flags[@]}")
  export TMPDIR="$STORE_DIR/tmp"
fi

# need_space <dir> <GiB> <what> — fail early with a clear message instead of ENOSPC mid-copy.
need_space() {
  local avail
  avail=$(df -BG --output=avail "$1" | tail -1 | tr -dc 0-9)
  if (( avail < $2 )); then
    echo "error: only ${avail}G free on $(df --output=target "$1" | tail -1) for $3 (need ~$2G)." >&2
    echo "       Point STORE_DIR at a bigger filesystem, e.g. STORE_DIR=/home/podman-3fs-root" >&2
    exit 1
  fi
}
