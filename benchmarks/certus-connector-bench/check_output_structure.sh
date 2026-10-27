#!/bin/bash
# Check that this checkout's bench_connector_lifecycle.py, run with default flags,
# prints the same lines, labels and fields as a reference checkout's (numbers
# stripped). certus-server must be running at --shm-path.
#
#   check_output_structure.sh <reference checkout> [benchmark args...]
#
# Exit 0 when the structures match; prints the diff otherwise.
set -euo pipefail
ref=$1; shift
here="$(cd "$(dirname "$0")/../.." && pwd)"
args=(--min-duration 1 --num-blocks 64 --working-set 64 "$@")
tmp=$(mktemp -d)
strip() {
    # Numbers, column padding, the output path and leaked-claim lines carry no structure.
    sed -E -e 's/[0-9]+(\.[0-9]+)?/N/g' -e 's/ +/ /g' -e 's#[^ ]*/(ref|here)\.(yaml|csv)#OUT#' \
        -e '/Cleared N leaked channel claims/d'
}
for side in ref here; do
    root=$ref; [ "$side" = here ] && root=$here
    python3 "$root/benchmarks/certus-connector-bench/bench_connector_lifecycle.py" "${args[@]}" \
        --output "$tmp/$side.yaml" > "$tmp/$side.out"
    strip < "$tmp/$side.out" > "$tmp/$side.struct"
    strip < "$tmp/$side.yaml" | grep -v "timestamp" > "$tmp/$side.yaml.struct"
done
status=0
diff -u "$tmp/ref.struct" "$tmp/here.struct" || status=1
diff -u "$tmp/ref.yaml.struct" "$tmp/here.yaml.struct" || status=1
[ $status -eq 0 ] && echo "structure identical (stdout and YAML)"
rm -rf "$tmp"
exit $status
