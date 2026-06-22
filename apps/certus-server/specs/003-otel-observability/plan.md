# Implementation Plan: OpenTelemetry Observability

**Branch**: `003-otel-observability` | **Date**: 2026-06-18 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

Optional OpenTelemetry metrics export for the certus-server, providing operational counters (ops, errors, clears, flushes), latency histograms per operation type, and fine-grained pipeline stage timing for cold-path, hot-path, and populate flows.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `opentelemetry` 0.27 (meter API)
- `opentelemetry_sdk` 0.27 with `rt-tokio` and `metrics` features (SDK meter provider, periodic reader)
- `opentelemetry-otlp` 0.27 with `metrics` and `grpc-tonic` features (OTLP gRPC exporter)
- `dispatcher::PipelineMetrics` trait (callback interface for stage timing)

**Compile-time gating**: All OTel code is behind `#[cfg(feature = "otel")]`. The `otel` feature is opt-in; default builds have zero OTel overhead.

## Architecture

### Metrics Initialization Flow

```
main()
├── CLI: --otel-endpoint, --otel-service-name
├── Metrics::init(endpoint, service_name)
│   ├── MetricExporter::builder().with_tonic().with_endpoint()
│   ├── PeriodicReader::builder(exporter, Tokio).with_interval(10s)
│   ├── SdkMeterProvider::builder().with_reader().with_resource()
│   └── Create all instruments from meter("certus-server")
└── disp_comp.set_pipeline_metrics(Arc::new(metrics.pipeline.clone()))
```

### Recording Points

```
DispatcherService (service.rs)
├── populate() → record_op("populate", count, errors, duration)
├── lookup()   → record_op("lookup", count, errors, duration)
├── check()    → record_op("check", count, 0, duration)
├── remove()   → record_op("remove", count, errors, duration)
├── touch()    → record_op("touch", count, errors, duration)
├── clear_memory_tier() → entries_cleared.add(n)
└── flush_to_ssd()      → jobs_flushed.add(n)

DispatcherComponent (via PipelineMetrics trait)
├── Cold path: ssd_read, gpu_dma, stream_sync, total, prep, finalize
├── Hot path: gpu_dma
└── Populate: gpu_d2h, alloc, total
```

### Key Design Decisions

1. **Feature-gated compilation**: Zero-cost when disabled. The `otel` feature gates all imports, struct fields, and recording calls via `#[cfg(feature = "otel")]`.

2. **Trait-based pipeline metrics**: The `dispatcher::PipelineMetrics` trait decouples the dispatcher crate from OTel. The server provides the concrete implementation (`PipelineStageMetrics`) that records to OTel histograms.

3. **Pre-initialized counters**: `entries_cleared` and `jobs_flushed` are initialized with `add(0)` so they appear in Prometheus/Grafana before any operations occur (avoids "no data" gaps in dashboards).

4. **10-second export interval**: Balances metric freshness against export overhead. The periodic reader flushes accumulated metric data every 10 seconds via OTLP gRPC.

## Project Structure

```text
apps/certus-server/src/
├── telemetry.rs    # Metrics struct, PipelineStageMetrics, init()
├── main.rs         # OTel CLI args, conditional init, set_pipeline_metrics
└── service.rs      # record_op() calls in each RPC handler
```

## Dependencies

- Dispatcher crate must define and export the `PipelineMetrics` trait
- Dispatcher component must implement `set_pipeline_metrics(Arc<dyn PipelineMetrics>)`
- OTLP collector must be reachable at the configured endpoint

## Testing

- Integration: Start server with `--otel-endpoint`, run workload, verify metrics in Prometheus/Grafana
- Unit: Verify `Metrics::init()` creates all expected instruments (can use in-memory exporter)
- Negative: Verify binary without `otel` feature compiles and runs without OTel flags
