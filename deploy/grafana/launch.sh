#!/bin/bash
#
# launch.sh - Start the Certus observability stack (OTel Collector + Prometheus + Grafana)
#
# Usage:
#   ./launch.sh          # start in background
#   ./launch.sh --logs   # start and tail logs
#
# Once running:
#   - Grafana UI:        http://localhost:3000
#   - Prometheus UI:     http://localhost:9090
#   - OTLP gRPC:        localhost:4317
#   - OTLP HTTP:        localhost:4318
#
# Run certus-server with:
#   certus-server --drive-count 4 --format --otel-endpoint http://localhost:4317
#
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

echo "Starting Certus observability stack..."
docker compose up -d

echo ""
echo "Stack is running:"
echo "  Grafana:         http://localhost:3000"
echo "  Prometheus:      http://localhost:9090"
echo "  OTLP endpoint:   localhost:4317 (gRPC) / localhost:4318 (HTTP)"
echo ""
echo "Pre-built dashboard: Certus Dispatcher (auto-provisioned)"
echo ""
echo "To stop: ./stop.sh"

if [[ "${1:-}" == "--logs" ]]; then
    docker compose logs -f
fi
