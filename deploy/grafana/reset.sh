#!/bin/bash
#
# reset.sh - Reset all collected metrics data (clears Prometheus storage)
#
# Usage:
#   ./reset.sh
#
# This restarts Prometheus with a clean storage volume, effectively
# clearing all historical metric data from Grafana dashboards.
# The OTel Collector and Grafana remain running.
#
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

echo "Resetting Prometheus metrics data..."
docker compose stop prometheus
docker compose rm -f prometheus
docker volume rm -f grafana_prometheus-data 2>/dev/null || true
docker compose up -d prometheus

echo ""
echo "Done. All historical metrics have been cleared."
echo "New data will appear within ~10 seconds of the next certus-server export cycle."
