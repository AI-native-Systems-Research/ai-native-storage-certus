# Drift Report: certus-server

**Generated**: 2026-07-10  
**Specs analyzed**: 3  
**Requirements checked**: 45  

## Summary

| Status | Count |
|--------|-------|
| Aligned | 43 |
| Drifted | 1 |
| Not Validated | 2 |
| Unspecced Features | 0 |

---

## Spec 001: gRPC Dispatcher Server

**Overall**: 22/22 requirements aligned

### Aligned Requirements

- **FR-001**: gRPC service exposes lookup, check, remove, populate, touch, clear_memory_tier. Lifecycle operations not exposed via gRPC.
- **FR-002**: Populate accepts list of (key, ipc_handle) pairs, calls dispatcher.populate() per entry, returns per-entry results.
- **FR-003**: Lookup uses dispatcher.batch_lookup() for parallel cold promotion; IPC handles deduplicated within batch via local_ptrs HashMap.
- **FR-004**: Check accepts list of keys, calls dispatcher.check() per key, returns list of CheckResult with boolean exists.
- **FR-005**: Remove accepts list of keys, calls dispatcher.remove() per key, returns per-entry results.
- **FR-005b**: Touch accepts list of keys, calls dispatcher.touch() per key, returns per-entry results.
- **FR-006**: CLI accepts --device-pci (repeatable), --listen (default 0.0.0.0:50051), --tls-cert, --tls-key.
- **FR-007**: EntryResult includes key, success, error_code, error_message. Partial-success model: each entry processed independently.
- **FR-008**: Server auto-initializes dispatcher on startup: SPDK env -> GPU services -> dispatch map -> memory-tier -> dispatcher.
- **FR-008b**: Dispatch map starts fresh each launch (DispatchMapState::default(), no extent manager).
- **FR-008c**: Memory-tier pool registered with CUDA via cudaHostRegister; logs warning and continues on failure.
- **FR-009**: SIGTERM and SIGINT both handled via tokio::signal. Dispatcher shutdown called after serve_with_shutdown completes.
- **FR-010**: Python test client in python-client/ directory.
- **FR-011**: IpcHandle in proto: cuda_ipc_handle (bytes, 64), size (uint32), gpu_device_id (int32).
- **FR-013**: Concurrent request processing via Arc<dyn IDispatcher> (Send + Sync). Fine-grained serialization at IPC cache and pending-stores maps only.
- **FR-014**: TLS enabled when both --tls-cert and --tls-key provided.
- **FR-015**: check_duplicate_keys() pre-validates all batch requests.
- **FR-016**: --device-pci is repeatable; all addresses passed to DispatcherConfig.data_pci_addrs.
- **FR-017**: ClearMemoryTier RPC calls dispatcher.clear_memory_tier(), returns entries_cleared.
- **FR-018**: Global IPC handle cache with reference counting; cudaIpcCloseMemHandle only when refcount reaches zero.
- **FR-019**: cudaSetDevice(gpu_device_id) called before cudaIpcOpenMemHandle when gpu_device_id >= 0.
- **FR-020**: Split-phase store protocol (Reserve/CopyToStore/CommitStore/AbortStore) with PendingStores tracking.
- **FR-021**: Pin/Unpin RPCs with promote flag for combined pin+prefetch.
- **FR-022**: TakeEvents RPC drains eviction events from bounded crossbeam channel (capacity 16384).

### Success Criteria

| SC | Status | Notes |
|----|--------|-------|
| SC-001 | aligned | All 5 operation types work in batch with single round-trip |
| SC-002 | aligned | Batch operations supported up to arbitrary size |
| SC-003 | aligned | Per-entry error reporting with key correlation |
| SC-004 | aligned | Component stack initializes promptly |
| SC-005 | aligned | Python test client exercises acceptance scenarios |
| SC-006 | aligned | Multiple --device-pci arguments work |

---

## Spec 002: Operational Configuration & Lifecycle

**Overall**: 13/13 requirements aligned, 1 minor drift (cosmetic, code-side)

### Aligned Requirements

- **FR-001**: --drive-count supported, conflicts_with = "device_pci" enforced by clap.
- **FR-002**: NVMe discovery via SPDK devices() filtered by class code 0x010802, sorted NUMA-0 first, first N selected.
- **FR-003**: If nvme_devices.len() < count, returns descriptive error.
- **FR-004**: resolve_device_addresses() returns Err if neither --device-pci nor --drive-count specified.
- **FR-005**: --format flag; format_on_init passed to DispatcherConfig.
- **FR-006**: parse_size() handles K/M/G suffixes (case-insensitive), DEFAULT_MEMORY_TIER_SIZE = 2 GiB.
- **FR-007**: --poller-base-cpu passed to DispatcherConfig as poller_base_cpu: Option<usize>.
- **FR-008**: --max-eviction-attempts with default_value_t = 2048, passed to DispatcherConfig.
- **FR-009**: validate_pci_address() / parse_pci_address() validates DDDD:BB:DD.F format.
- **FR-010**: FlushToSsd RPC blocks until complete, returns jobs_flushed.
- **FR-011**: Touch accepts promote bool field; fires background promote_to_memory_tier.
- **FR-012**: cudaHostRegister on memory-tier pool with warning fallback.
- **FR-013**: Memory-tier pool allocated on NUMA node of first selected NVMe drive.

### Drifted (code-side only)

| Req | Severity | Issue |
|-----|----------|-------|
| FR-006 | minor | CLI struct doc comment says "Defaults to 256M" (src/main.rs:44) but actual default is 2 GiB. Spec is correct; only the help text shown to users is wrong. |

**Recommendation**: Fix the doc comment from "Defaults to 256M" to "Defaults to 2G" to match the actual default.

### Success Criteria

| SC | Status | Notes |
|----|--------|-------|
| SC-001 | aligned | --drive-count auto-selects NVMe devices |
| SC-002 | aligned | Without --format, extent managers recover |
| SC-003 | aligned | --poller-base-cpu configures pinning |
| SC-004 | aligned | Pool size logged in MiB |

---

## Spec 003: OpenTelemetry Observability

**Overall**: 11/11 FRs aligned, 2 success criteria not validated

### Aligned Requirements

- **FR-001**: `otel` feature flag gates all OTel code.
- **FR-002**: MetricExporter with tonic/gRPC targeting --otel-endpoint.
- **FR-003**: --otel-service-name configures service.name attribute (default "certus-server").
- **FR-004**: PeriodicReader with 10-second interval.
- **FR-005**: ops.total incremented by entry count per batch (total entries processed); ops.errors, op.duration_us, batch.size all present with `op` attribute.
- **FR-006**: entries_cleared and jobs_flushed counters present.
- **FR-007**: Cold pipeline histograms: ssd_read_us, gpu_dma_us, stream_sync_us, total_us, prep_us, finalize_us.
- **FR-008**: Hot pipeline histogram: hot.gpu_dma_us.
- **FR-009**: Populate pipeline histograms: gpu_d2h_us, alloc_us, total_us.
- **FR-010**: PipelineStageMetrics injected via disp_comp.set_pipeline_metrics().
- **FR-011**: entries_cleared and jobs_flushed initialized to 0.

### Not Validated (Success Criteria)

| SC | Status | Issue |
|----|--------|-------|
| SC-002 | not validated | "Pipeline stage metrics correlate with hardware-measured latencies within 10%." No automated validation harness exists. Requires hardware test environment. |
| SC-003 | not validated | "Disabling otel feature produces binary with zero OTel dependencies." Assumed correct from cfg gating but no CI check confirms the dependency tree. |

---

## Action Items

| Priority | Action | Type |
|----------|--------|------|
| Low | Fix --memory-tier-size help text from "256M" to "2G" in src/main.rs:44 | Code fix |
| Low | Add CI check for zero OTel deps when feature disabled (SC-003 validation) | CI |
