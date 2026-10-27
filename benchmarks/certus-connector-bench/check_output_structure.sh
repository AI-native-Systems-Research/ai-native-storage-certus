#!/bin/bash
# Check that this checkout's bench_connector_lifecycle.py, run with default flags,
# prints the same lines, labels and fields as a reference checkout's (values
# stripped), the same YAML keys and the same CSV columns. certus-server must be
# running at --shm-path.
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
    # Values, column padding, the output path and leaked-claim lines carry no
    # structure. Only numbers not part of a name are replaced, so field names such
    # as p50_us and p99_us stay distinct.
    sed -E -e 's#[^ ]*/(ref|here)\.(yaml|csv)#OUT#' \
        -e 's/(^|[^A-Za-z0-9_])[0-9]+(\.[0-9]+)?/\1N/g' -e 's/ +/ /g' \
        -e '/Cleared N leaked channel claims/d'
}
for side in ref here; do
    root=$ref; [ "$side" = here ] && root=$here
    python3 "$root/benchmarks/certus-connector-bench/bench_connector_lifecycle.py" "${args[@]}" \
        --output "$tmp/$side.yaml" --csv "$tmp/$side.csv" > "$tmp/$side.out"
    strip < "$tmp/$side.out" > "$tmp/$side.struct"
    strip < "$tmp/$side.yaml" | grep -v "timestamp" > "$tmp/$side.yaml.struct"
    head -1 "$tmp/$side.csv" > "$tmp/$side.csv.struct"     # CSV columns
done
status=0
diff -u "$tmp/ref.struct" "$tmp/here.struct" || status=1
diff -u "$tmp/ref.yaml.struct" "$tmp/here.yaml.struct" || status=1
diff -u "$tmp/ref.csv.struct" "$tmp/here.csv.struct" || status=1
[ $status -eq 0 ] && echo "structure identical (stdout, YAML and CSV columns)"
rm -rf "$tmp"
exit $status
