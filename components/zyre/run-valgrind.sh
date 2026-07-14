#!/usr/bin/env bash
#
# SC-005 memory-safety check for the zyre bindings.
#
# Miri cannot execute across the FFI boundary into libzmq/czmq/zyre, so memory
# safety is validated with valgrind's memcheck over the test binaries instead.
# The C libraries run background threads and keep global state that memcheck
# flags as leaked/reachable at exit; `valgrind.supp` suppresses reports whose
# allocation stack lies entirely inside those libraries, leaving only reports
# attributable to the Rust bindings — which fail the run (`--error-exitcode`).
#
# Usage:
#   components/zyre/run-valgrind.sh                # lib tests + non-timed integration tests
#   components/zyre/run-valgrind.sh <test-filter>  # restrict to matching tests
#
# Requires: valgrind, and the C deps pre-built at deps/zyre-build/.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SUPP="${SCRIPT_DIR}/valgrind.supp"
FILTER="${1:-}"

command -v valgrind >/dev/null || { echo "valgrind not found" >&2; exit 127; }

cd "${WORKSPACE_ROOT}"

# Build (don't run) the test binaries; capture their paths from JSON output.
echo ">> building zyre test binaries" >&2
mapfile -t TEST_BINS < <(
  cargo test -p zyre --no-run --message-format=json 2>/dev/null \
    | python3 -c 'import sys,json
for line in sys.stdin:
    try: m=json.loads(line)
    except ValueError: continue
    if m.get("profile",{}).get("test") and m.get("executable"):
        print(m["executable"])'
)

VALGRIND=(valgrind --tool=memcheck --leak-check=full
  --show-leak-kinds=definite,indirect
  --errors-for-leak-kinds=definite,indirect
  --error-exitcode=1 --num-callers=30
  "--suppressions=${SUPP}")

# Discovery-dependent tests need relaxed deadlines under valgrind's slowdown.
export ZYRE_TEST_TIMEOUT_SCALE="${ZYRE_TEST_TIMEOUT_SCALE:-40}"

rc=0
for bin in "${TEST_BINS[@]}"; do
  echo ">> valgrind: ${bin} ${FILTER}" >&2
  # Test harness must stay single-threaded (czmq global socket-count assert).
  if ! "${VALGRIND[@]}" "${bin}" ${FILTER:+"${FILTER}"} --test-threads 1 --nocapture; then
    rc=1
  fi
done

if [ "${rc}" -eq 0 ]; then
  echo ">> SC-005: no binding-attributable memcheck errors" >&2
else
  echo ">> SC-005: memcheck reported binding-attributable errors (see above)" >&2
fi
exit "${rc}"
