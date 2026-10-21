#!/bin/bash
# admin.sh — run admin_cli inside the running cluster container (ROOT).
#
#   sudo ./admin.sh list-nodes
#   sudo ./admin.sh list-chains
#   sudo ./admin.sh list-targets
#   sudo ./admin.sh                # no args: nodes + chains summary
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"   # same STORE_DIR run.sh used (saved)
NAME="${NAME:-certus-3fs}"
run() {
  "${PODMAN[@]}" exec "$NAME" bash -c \
    '/opt/3fs/bin/admin_cli --cfg /opt/3fs/etc/admin_cli.toml --config.user_info.token "$(cat /data/state/token.txt)" "$1"' _ "$1"
}
if [[ $# -eq 0 ]]; then
  run list-nodes; run list-chains
else
  run "$*"
fi
