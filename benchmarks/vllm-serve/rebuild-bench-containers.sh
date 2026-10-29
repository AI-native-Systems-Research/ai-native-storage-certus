#!/bin/bash
# rebuild-bench-containers.sh — rebuild the container images the vllm-serve
# benchmarks run, from the current checkout, in one go. A thin wrapper over the
# per-image build scripts (it does not duplicate their logic):
#
#   shmq    localhost/certus-shmq-connector:vllm<ver>   certus-shmq-connector/build_connector_container.sh
#           (Certus arms; /mnt/certus1 podman store)
#   cputier certus-offload:vllm<ver>                    benchmarks/kv-offload-replay/build_cputier_container.sh
#           (cputier arms; default podman store)
#   hf3fs   vllm-hf3fs-bench:vllm<ver>                  benchmarks/vllm-serve/build_h3fs_container.sh
#           (3FS arms; default store; heavy build, so not in the default set)
#
# The shmq image bakes the connector source in (the serve scripts do not mount it),
# so it must be rebuilt after any connector change before a Certus measurement.
#
# Before each build the existing image is re-tagged <tag>-pre-<YYYYMMDD_HHMMSS>, so
# earlier results stay reproducible; KEEP_BACKUP=0 skips that. Refuses to run while
# a container from an image it would rebuild is running (a benchmark is in flight),
# unless FORCE=1.
#
#   ./rebuild-bench-containers.sh                   # shmq + cputier on vLLM 0.30.0
#   ./rebuild-bench-containers.sh shmq              # just the Certus connector image
#   ./rebuild-bench-containers.sh shmq cputier hf3fs
#   VLLM_VERSION=0.29.0 ./rebuild-bench-containers.sh
#   NO_CACHE=1 ./rebuild-bench-containers.sh cputier # forwarded to the build script
#   ./rebuild-bench-containers.sh --dry-run          # print what would be built
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

VLLM_VERSION="${VLLM_VERSION:-0.30.0}"
VLLM_VERSION="${VLLM_VERSION#v}"
KEEP_BACKUP="${KEEP_BACKUP:-1}"
SHMQ_STORE=(--root "${PODMAN_STORE:-/mnt/certus1/podman/storage}"
            --runroot "${PODMAN_RUNROOT:-/mnt/certus1/podman/run}")
STAMP="$(date +%Y%m%d_%H%M%S)"

DRY_RUN=0 TARGETS=()
for a in "$@"; do
  case "$a" in
    --dry-run) DRY_RUN=1 ;;
    -h|--help) sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    shmq|cputier|hf3fs) TARGETS+=("$a") ;;
    *) echo "error: unknown target '$a' (shmq | cputier | hf3fs | --dry-run)" >&2; exit 2 ;;
  esac
done
(( ${#TARGETS[@]} )) || TARGETS=(shmq cputier)

# Per target: build script, podman store flags, image ref (from the script itself).
script_for(){ case "$1" in
  shmq)    echo "${REPO_ROOT}/certus-shmq-connector/build_connector_container.sh" ;;
  cputier) echo "${REPO_ROOT}/benchmarks/kv-offload-replay/build_cputier_container.sh" ;;
  hf3fs)   echo "${SCRIPT_DIR}/build_h3fs_container.sh" ;;
esac; }
pm(){  # podman in the store the target's image lives in
  local t="$1"; shift
  if [[ "$t" == shmq ]]; then podman "${SHMQ_STORE[@]}" "$@"; else podman "$@"; fi
}
image_for(){ "$(script_for "$1")" --print-image "$VLLM_VERSION"; }

log(){ echo "[$(date +%H:%M:%S)] $*"; }

command -v podman >/dev/null || { echo "error: podman not on PATH" >&2; exit 1; }
for t in "${TARGETS[@]}"; do
  [[ -x "$(script_for "$t")" ]] || { echo "error: build script missing: $(script_for "$t")" >&2; exit 1; }
done

# Refuse while a benchmark is using one of these images.
if [[ "${FORCE:-0}" != 1 ]]; then
  busy=0
  for t in "${TARGETS[@]}"; do
    img="$(image_for "$t")"
    ids=$(pm "$t" ps -q --filter "ancestor=${img}" 2>/dev/null)
    if [[ -n "$ids" ]]; then
      echo "error: ${img} is in use by running container(s): ${ids//$'\n'/ }" >&2
      busy=1
    fi
  done
  (( busy )) && { echo "       stop the benchmark first, or set FORCE=1" >&2; exit 1; }
fi

log "checkout: ${REPO_ROOT} @ $(git -C "$REPO_ROOT" rev-parse --short HEAD) ($(git -C "$REPO_ROOT" branch --show-current))"
if [[ -n "$(git -C "$REPO_ROOT" status --porcelain -- certus-shmq-connector benchmarks/kv-offload-replay 2>/dev/null | grep -v '^??')" ]]; then
  log "WARNING: uncommitted changes under certus-shmq-connector/ or benchmarks/kv-offload-replay/ will be baked in"
fi
log "vLLM ${VLLM_VERSION}; targets: ${TARGETS[*]}"

declare -A RESULT
failed=0
for t in "${TARGETS[@]}"; do
  img="$(image_for "$t")"
  log "===== ${t}: ${img} ====="
  if (( DRY_RUN )); then
    log "dry run: would back up and run $(script_for "$t") ${VLLM_VERSION}"
    RESULT[$t]="dry run"; continue
  fi

  old_id=$(pm "$t" image inspect "$img" --format '{{.Id}}' 2>/dev/null)
  if [[ -n "$old_id" && "$KEEP_BACKUP" == 1 ]]; then
    pm "$t" tag "$img" "${img}-pre-${STAMP}" && log "backed up ${old_id:0:12} as ${img}-pre-${STAMP}"
  fi

  start=$(date +%s)
  if "$(script_for "$t")" "$VLLM_VERSION"; then
    new_id=$(pm "$t" image inspect "$img" --format '{{.Id}}' 2>/dev/null)
    if [[ -z "$new_id" ]]; then
      RESULT[$t]="FAILED (build ok but image not found)"; failed=1
    elif [[ "$new_id" == "$old_id" ]]; then
      RESULT[$t]="unchanged ${new_id:0:12} (nothing changed since last build)"
    else
      RESULT[$t]="rebuilt ${new_id:0:12} in $(( $(date +%s) - start ))s"
    fi
  else
    RESULT[$t]="FAILED (see build output above)"; failed=1
  fi
  log "${t}: ${RESULT[$t]}"
done

echo
log "summary (vLLM ${VLLM_VERSION}):"
for t in "${TARGETS[@]}"; do printf '  %-8s %-52s %s\n' "$t" "$(image_for "$t")" "${RESULT[$t]}"; done
(( KEEP_BACKUP == 1 && ! DRY_RUN )) && echo "  previous images kept as <tag>-pre-${STAMP} (remove with podman rmi when no longer needed)"
exit "$failed"
