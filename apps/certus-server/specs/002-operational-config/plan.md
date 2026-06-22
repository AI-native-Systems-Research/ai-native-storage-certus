# Implementation Plan: Operational Configuration & Lifecycle

**Branch**: `002-operational-config` | **Date**: 2026-06-18 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

Operational configuration enhancements that allow the certus-server to auto-discover NVMe drives, control extent manager lifecycle (format vs recovery), tune memory-tier sizing, pin poller threads to CPU cores, and expose data-persistence operations (FlushToSsd, Touch with promote) to clients.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `clap` 4 (CLI parsing with derive macros, `conflicts_with` for mutual exclusion)
- `spdk-env` (device discovery via `ISPDKEnv::devices()`)
- `dispatcher` (DispatcherConfig fields: `format_on_init`, `poller_base_cpu`, `max_eviction_attempts`)
- `memory-tier` (configurable pool size via `initialize(size)`)
- `gpu-services` (CUDA host registration for pinned DMA)
- `tonic` 0.12 (gRPC service with `FlushToSsd` and `Touch.promote`)

## Architecture

### CLI Parsing Layer

```
Cli (clap::Parser)
├── --device-pci (Vec<String>) ─── conflicts_with drive_count
├── --drive-count (Option<usize>)
├── --listen (String, default "0.0.0.0:50051")
├── --memory-tier-size (Option<usize>, value_parser = parse_size)
├── --format (bool flag)
├── --poller-base-cpu (Option<usize>)
├── --max-eviction-attempts (usize, default 2048)
├── --tls-cert / --tls-key (Optional TLS)
└── --otel-endpoint / --otel-service-name (Optional OTel)
```

### Device Resolution Flow

```
resolve_device_addresses()
├── If --device-pci provided → validate PCI format → return addresses
├── If --drive-count provided → return empty (deferred to post-SPDK-init)
└── Otherwise → error

initialize_component_stack()
├── SPDK init
├── If device_pci_addrs empty (drive-count mode):
│   ├── spdk_iface.devices()
│   ├── Filter by NVMe class code (0x010802)
│   ├── Sort by NUMA node (0 first)
│   └── Take first N
└── Continue with resolved addresses
```

### gRPC Extensions

```
Dispatcher service (proto)
├── FlushToSsd() → dispatcher.flush_to_ssd() (blocking)
└── Touch(keys, promote) → dispatcher.touch(key) + optional async promote
```

## Key Design Decisions

1. **Deferred device resolution**: `--drive-count` resolution happens after SPDK init because device enumeration requires an initialized SPDK environment. The CLI validation step accepts an empty device list when drive-count is set.

2. **NUMA-aware selection**: Devices on NUMA node 0 are preferred because the server process typically runs on node 0. This avoids cross-NUMA DMA traffic.

3. **Fire-and-forget promotion**: `Touch` with `promote = true` spawns the promotion in a separate `spawn_blocking` task after sending the response. This keeps touch latency low while enabling background warming.

4. **Graceful CUDA registration failure**: If `cudaHostRegister` fails for the memory-tier pool, the server continues rather than aborting. The staged (non-pinned) transfer path is used as fallback.

## Project Structure

```text
apps/certus-server/
├── src/
│   ├── main.rs          # CLI, parse_size, parse_pci_address, initialize_component_stack
│   └── service.rs       # FlushToSsd, Touch with promote
├── proto/
│   └── dispatcher.proto # FlushToSsdRequest/Response, BatchTouchRequest.promote field
└── specs/
    └── 002-operational-config/
```

## Testing

- Integration: Python test client exercises FlushToSsd and Touch with promote
- Manual: Verify `--drive-count` selects correct devices via server log output
- Manual: Verify poller pinning via `/proc/<pid>/task/*/status` → `Cpus_allowed`
