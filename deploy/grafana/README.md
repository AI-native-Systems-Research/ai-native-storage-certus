# Certus Grafana Observability Stack

Local Docker-based observability stack for Certus metrics.

## Architecture

```
certus-server --otel-endpoint localhost:4317
       │
       ▼ (OTLP gRPC)
┌──────────────────┐
│  OTel Collector  │ ──► Prometheus exporter (:8889)
└──────────────────┘
                           │
                           ▼ (scrape)
                    ┌──────────────┐
                    │  Prometheus  │
                    └──────────────┘
                           │
                           ▼ (query)
                    ┌──────────────┐
                    │   Grafana    │ ──► http://localhost:3000
                    └──────────────┘
```

## Quick Start

```bash
# 1. Launch the stack
./launch.sh

# 2. Build and run certus-server with OTel enabled
cargo build -p certus-server --features otel --release
./target/release/certus-server \
  --drive-count 4 --format \
  --otel-endpoint http://localhost:4317

# 3. Open Grafana
open http://localhost:3000
# Navigate to Dashboards → Certus → "Certus Dispatcher"
```

## Stopping

```bash
./stop.sh          # preserves metric history
./stop.sh --clean  # deletes everything
```

## Pre-built Dashboard

The "Certus Dispatcher" dashboard is auto-provisioned with:

- **Operations/sec** — rate by operation type (populate, lookup, check, remove, touch)
- **Error rate** — failed operations per second
- **Latency percentiles** — P50/P95/P99 per operation type (µs)
- **Batch size** — average entries per gRPC request
- **Memory-tier clears** — total entries evicted
- **SSD flush jobs** — background writes completed

## Ports

| Service        | Port  | Purpose             |
|----------------|-------|---------------------|
| OTel Collector | 4317  | OTLP gRPC receiver  |
| OTel Collector | 4318  | OTLP HTTP receiver  |
| Prometheus     | 9090  | Metrics storage/UI  |
| Grafana        | 3000  | Dashboard UI        |

## Requirements

- Docker with Compose v2 (`docker compose`)
- certus-server built with `--features otel`
