#!/usr/bin/env bash
#
# spec-sync-hash.sh — deterministic content hash of the inputs a component's
# spec-sync analysis reads. Printed as a single sha256 hex digest to stdout.
#
# Usage:
#   scripts/spec-sync-hash.sh <component-dir>
#
# The digest is stamped into <component-dir>/.specify/sync/drift-report.md
# (field: spec_sync_inputs_sha256) when /component-sync-specs is run, and the
# CI "Spec-Sync Gate" recomputes it to detect a report that went stale because
# the code or specs changed after the last sync.
#
# Inputs hashed:
#   - <component-dir>/src/**          (*.rs)
#   - <component-dir>/specs/**        (*.md — spec.md, plan.md, data-model.md, ...)
#   - components/interfaces/src/**    (*.rs)   folded in: transitive interface drift
#   - components/interfaces/specs/**  (*.md)
#
# The interface tree is folded into *every* component's hash so that an
# interface change invalidates the reports of the components that depend on it.
# It is NOT folded into interfaces' own hash (that would be circular).
#
# Excluded implicitly: build output (target/) and the .specify/ artifacts
# themselves — neither lives under src/ or specs/, so neither is walked.
#
# The listing is sorted for determinism and the *listing* is hashed (not just
# the concatenated bytes), so additions, deletions, and renames all change the
# digest even when surviving file contents do not.
set -euo pipefail

dir="${1:?usage: spec-sync-hash.sh <component-dir>}"
dir="${dir%/}"

# Resolve repo root from this script's location so the hash is independent of
# the caller's working directory.
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

iface="components/interfaces"

candidates=("$dir/src" "$dir/specs")
if [ "$dir" != "$iface" ]; then
  candidates+=("$iface/src" "$iface/specs")
fi

# Keep only paths that exist; find errors on a missing start path.
roots=()
for p in "${candidates[@]}"; do
  [ -e "$p" ] && roots+=("$p")
done

if [ ${#roots[@]} -eq 0 ]; then
  echo "spec-sync-hash: no src/ or specs/ found under '$dir'" >&2
  exit 2
fi

find "${roots[@]}" -type f \( -name '*.rs' -o -name '*.md' \) ! -path '*/target/*' -print0 \
  | LC_ALL=C sort -z \
  | xargs -0 sha256sum \
  | sha256sum \
  | awk '{print $1}'
