#!/usr/bin/env bash
#
# router-check.sh - is llm-d's router actually scheduling, or failing open?
#
# The router's envoy proxy health-checks the endpoint picker (EPP) and, when it
# finds it unhealthy, fails open: requests go round-robin to the model servers
# with no P/D decision, no prefix affinity and no fetch, and every one of them
# still succeeds. Nothing visible breaks, so a benchmark run against a
# failed-open router measures the wrong thing. This checks envoy's own view.
#
#   deploy/llm-d/router-check.sh          exit 0 if envoy sees the EPP healthy
#
# Env: NAMESPACE (default llm-d-quickstart), GUIDE_NAME (default quickstart),
#      ROUTER_CHECK_WAIT_S (default 120): how long to wait for a fresh router.

set -euo pipefail

NAMESPACE="${NAMESPACE:-llm-d-quickstart}"
GUIDE_NAME="${GUIDE_NAME:-quickstart}"
WAIT_S="${ROUTER_CHECK_WAIT_S:-120}"
PORT="${ROUTER_CHECK_ADMIN_PORT:-19400}"

pod="$(kubectl -n "${NAMESPACE}" get pods -l "llm-d-router-gateway=${GUIDE_NAME}-epp" \
        --field-selector status.phase=Running -o name | head -1)"
[ -n "${pod}" ] || { echo "router-check: no running ${GUIDE_NAME}-epp pod in ${NAMESPACE}" >&2; exit 1; }
images="$(kubectl -n "${NAMESPACE}" get "${pod}" -o jsonpath='{range .spec.containers[*]}{.image}{" "}{end}')"

kubectl -n "${NAMESPACE}" port-forward "${pod}" "${PORT}:19000" >/dev/null 2>&1 &
pf=$!
trap 'kill ${pf} 2>/dev/null || true' EXIT
deadline=$((SECONDS + WAIT_S))
stats=""
while [ ${SECONDS} -lt ${deadline} ]; do
    sleep 3
    stats="$(curl -s --max-time 5 "http://127.0.0.1:${PORT}/stats" 2>/dev/null \
             | grep -E '^cluster\.ext_proc\.(membership_healthy|health_check\.(success|failure))' || true)"
    if grep -q 'membership_healthy: 1' <<<"${stats}"; then
        echo "router-check: OK  ${pod#pod/}  ${images}"
        echo "${stats}" | sed 's/^/  /'
        exit 0
    fi
done
echo "router-check: FAILED  envoy sees the EPP as unhealthy: requests fail open (no P/D, no fetch)" >&2
echo "  ${pod#pod/}  ${images}" >&2
echo "${stats:-  (no envoy admin stats)}" | sed 's/^/  /' >&2
exit 1
