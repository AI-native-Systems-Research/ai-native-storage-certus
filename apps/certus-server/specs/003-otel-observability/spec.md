# Feature Specification: OpenTelemetry Observability

**Feature Branch**: `003-otel-observability`
**Created**: 2026-06-18
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> ⚠️ This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## User Scenarios & Testing

### User Story 1 - Enable Metrics Export (Priority: P1)

An operator starts certus-server with `--otel-endpoint` pointing at an OTLP-compatible collector. The server exports dispatcher operation metrics every 10 seconds.

**Why this priority**: Observability is essential for production monitoring and performance debugging.

**Acceptance Scenarios**:

1. **Given** a running OTLP collector at `http://localhost:4317`, **When** the server starts with `--otel-endpoint http://localhost:4317`, **Then** metrics appear in the configured backend within 10 seconds of the first operation.
2. **Given** the server is compiled without `--features otel`, **When** `--otel-endpoint` is specified, **Then** a warning is logged and no metrics are exported.
3. **Given** no `--otel-endpoint` is specified, **When** the server starts, **Then** no OTel metrics infrastructure is initialized.

---

### User Story 2 - Monitor Dispatcher Operation Metrics (Priority: P1)

An SRE observes operation throughput, error rates, and latency distributions via dashboard queries against `certus.dispatcher.*` metrics.

**Why this priority**: Core operational metrics for SLO tracking and alerting.

**Acceptance Scenarios**:

1. **Given** the server is processing requests with OTel enabled, **When** the SRE queries `certus.dispatcher.ops.total`, **Then** counts are broken down by operation type (populate, lookup, check, remove, touch).
2. **Given** some operations fail, **When** the SRE queries `certus.dispatcher.ops.errors`, **Then** error counts are broken down by operation type.
3. **Given** operations complete, **When** the SRE queries `certus.dispatcher.op.duration_us`, **Then** latency histograms are available per operation type.

---

### User Story 3 - Monitor Pipeline Stage Latency (Priority: P2)

A performance engineer drills into cold-path and hot-path pipeline stages to identify bottlenecks via `certus.pipeline.*` metrics.

**Why this priority**: Pipeline stage metrics pinpoint which hardware or software stage is the bottleneck (SSD read, GPU DMA, stream sync, eviction).

**Acceptance Scenarios**:

1. **Given** cold-path lookups are occurring, **When** the engineer queries `certus.pipeline.cold.*` metrics, **Then** per-stage histograms (ssd_read, gpu_dma, stream_sync, prep, finalize, total) are available.
2. **Given** hot-path lookups are occurring, **When** the engineer queries `certus.pipeline.hot.gpu_dma_us`, **Then** memory-tier to GPU DMA latency is reported.
3. **Given** populate operations are occurring, **When** the engineer queries `certus.pipeline.populate.*`, **Then** gpu_d2h, alloc, and total histograms are available.

---

## Requirements

### Functional Requirements

- **FR-001**: System MUST support optional OpenTelemetry metrics export via the `otel` compile-time feature flag. When the feature is not compiled, OTel code is completely excluded.
- **FR-002**: When `--otel-endpoint` is specified and the `otel` feature is enabled, the server MUST initialize an OTLP gRPC metric exporter targeting the specified endpoint.
- **FR-003**: The `--otel-service-name` argument MUST configure the `service.name` resource attribute (default: `certus-server`).
- **FR-004**: Metrics MUST be exported periodically with a 10-second interval.
- **FR-005**: The server MUST export the following dispatcher-level metrics with `op` attribute:
  - `certus.dispatcher.ops.total` (counter): Total entries processed per operation type (incremented by the entry count of each batch, not by 1 per batch invocation)
  - `certus.dispatcher.ops.errors` (counter): Failed entries per operation type
  - `certus.dispatcher.op.duration_us` (histogram): Operation latency in microseconds (per batch)
  - `certus.dispatcher.batch.size` (histogram): Entries per batch request
- **FR-006**: The server MUST export the following aggregate counters:
  - `certus.dispatcher.entries_cleared` (counter): Total memory-tier entries cleared
  - `certus.dispatcher.jobs_flushed` (counter): Total background jobs flushed to SSD
- **FR-007**: The server MUST export pipeline stage histograms for the cold path:
  - `certus.pipeline.cold.ssd_read_us` (with `drive` attribute)
  - `certus.pipeline.cold.gpu_dma_us`
  - `certus.pipeline.cold.stream_sync_us`
  - `certus.pipeline.cold.total_us` (with `drive` attribute)
  - `certus.pipeline.cold.prep_us`
  - `certus.pipeline.cold.finalize_us`
- **FR-008**: The server MUST export pipeline stage histograms for the hot path:
  - `certus.pipeline.hot.gpu_dma_us`
- **FR-009**: The server MUST export pipeline stage histograms for populate:
  - `certus.pipeline.populate.gpu_d2h_us`
  - `certus.pipeline.populate.alloc_us`
  - `certus.pipeline.populate.total_us`
- **FR-010**: Pipeline metrics MUST be injected into the dispatcher component via `set_pipeline_metrics()` so the dispatcher can record stage timings during request processing.
- **FR-011**: Counter metrics (`entries_cleared`, `jobs_flushed`) MUST be initialized with value 0 so they appear in backends immediately (before any operations occur).

### Metric Semantic Conventions

| Metric Name | Type | Unit | Attributes | Description |
|-------------|------|------|------------|-------------|
| `certus.dispatcher.ops.total` | Counter | entries | `op` | Total entries processed (batch size added per call) |
| `certus.dispatcher.ops.errors` | Counter | ops | `op` | Failed entries |
| `certus.dispatcher.op.duration_us` | Histogram | µs | `op` | End-to-end RPC latency |
| `certus.dispatcher.batch.size` | Histogram | entries | `op` | Batch size distribution |
| `certus.dispatcher.entries_cleared` | Counter | entries | — | Memory tier clears |
| `certus.dispatcher.jobs_flushed` | Counter | jobs | — | SSD flush completions |
| `certus.pipeline.cold.ssd_read_us` | Histogram | µs | `drive` | NVMe read wait |
| `certus.pipeline.cold.gpu_dma_us` | Histogram | µs | — | GPU DMA enqueue |
| `certus.pipeline.cold.stream_sync_us` | Histogram | µs | — | GPU stream sync |
| `certus.pipeline.cold.total_us` | Histogram | µs | `drive` | Total cold pipeline |
| `certus.pipeline.cold.prep_us` | Histogram | µs | — | Eviction + slot insert |
| `certus.pipeline.cold.finalize_us` | Histogram | µs | — | Dispatch-map re-reg |
| `certus.pipeline.hot.gpu_dma_us` | Histogram | µs | — | Memory→GPU DMA |
| `certus.pipeline.populate.gpu_d2h_us` | Histogram | µs | — | GPU D2H copy |
| `certus.pipeline.populate.alloc_us` | Histogram | µs | — | Allocation + eviction |
| `certus.pipeline.populate.total_us` | Histogram | µs | — | Total populate |

## Key Entities

- **Metrics**: Struct holding all OTel instrument handles (counters, histograms) and the meter provider.
- **PipelineStageMetrics**: Struct implementing `dispatcher::PipelineMetrics` trait, bridging the dispatcher's timing callbacks to OTel histogram recordings.
- **SdkMeterProvider**: OpenTelemetry SDK provider managing the OTLP exporter and periodic reader.

## Dependencies

- **Spec 001**: gRPC Dispatcher Server (service layer where metrics are recorded)
- **Spec 002**: Operational Configuration (CLI flags for endpoint and service name)
- **Crate `dispatcher`**: Defines `PipelineMetrics` trait and `set_pipeline_metrics()` method

## Success Criteria

- **SC-001**: With OTel enabled, all 16 metrics appear in the configured backend within 20 seconds of a workload run.
- **SC-002**: Pipeline stage metrics correlate with hardware-measured latencies (within 10% for NVMe read, GPU DMA).
- **SC-003**: Disabling the `otel` feature produces a binary with zero OTel dependencies and no runtime overhead.
