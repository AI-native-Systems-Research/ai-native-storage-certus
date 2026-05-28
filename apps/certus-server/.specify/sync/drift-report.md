# Spec Drift Report

Generated: 2026-05-28
Project: certus-server

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 17 |
| Aligned | 14 (82%) |
| Drifted | 3 (18%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 1 |

## Detailed Findings

### Spec: 001-grpc-dispatcher-server - gRPC Dispatcher Server

#### Aligned

- FR-001: gRPC service exposes lookup, check, remove, populate, touch, clear_memory_tier -> `proto/dispatcher.proto`, `src/service.rs`
- FR-002: Populate accepts list of (key, ipc_handle), returns per-entry results -> `src/service.rs:124-175`
- FR-004: Check accepts list of keys, returns list of boolean results -> `src/service.rs:295-317`
- FR-005: Remove accepts list of keys, returns per-entry results -> `src/service.rs:319-341`
- FR-005b: Touch accepts list of keys, returns per-entry results -> `src/service.rs:343-365`
- FR-006: CLI accepts device PCI(s), listen address/port, TLS cert/key paths, memory-tier-size, format flag -> `src/main.rs`
- FR-007: Per-entry results include key, success, error_code, error_message -> `proto/dispatcher.proto`, `src/service.rs:103-119`
- FR-008: Server auto-initializes dispatcher on startup using CLI PCI addresses -> `src/main.rs`
- FR-008b: Dispatch map starts fresh each launch -> confirmed
- FR-008c: Memory-tier pool registered with CUDA via cudaHostRegister -> confirmed
- FR-010: Python test client exercises all gRPC methods with batch operations -> `python-client/`
- FR-013: Multiple connections supported, processing serialized via Mutex -> `src/service.rs:28-29`
- FR-014: Optional TLS via --tls-cert/--tls-key -> `src/main.rs`
- FR-015: Pre-validates batch for duplicate keys, rejects entire batch -> `src/service.rs:38-48`
- FR-016: Multiple --device-pci arguments supported -> `src/main.rs`
- FR-017: ClearMemoryTier gRPC method exposed -> `src/service.rs:367-384`

#### Drifted

- FR-003: Spec says "execute the dispatcher's `lookup()` for each pair server-side" but code now calls `disp.batch_lookup(&valid_batch)` — a single batch call instead of per-entry iteration. The external behavior is equivalent (per-entry results returned) but the internal dispatch is different.
  - Location: `src/service.rs:266`
  - Severity: minor (optimization, externally equivalent)

- FR-009: Spec says "shut down dispatcher when receiving SIGTERM/SIGINT" but code only handles SIGINT via `tokio::signal::ctrl_c()`. SIGTERM is not caught.
  - Location: `src/main.rs`
  - Severity: moderate

- FR-011: Spec assumes IPC handle contains "a memory address and size" transmitted as integers. Implementation uses a 64-byte CUDA IPC handle (`bytes cuda_ipc_handle`) + `uint32 size`. This is correct for cross-process GPU sharing but diverges from original spec text.
  - Location: `proto/dispatcher.proto`
  - Severity: minor (spec should be updated to match better design)

#### Not Implemented

(none)

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| IPC handle caching within batch (open once, reuse for shared handles) | `src/service.rs:190-252` | 62 | Update FR-003 |

## Recommendations

1. **Update FR-003** to reflect that lookup internally calls `batch_lookup` for parallel cold promotion rather than per-entry sequential `lookup()`. Note the external contract (per-entry results) is unchanged.
2. **Fix SIGTERM handling (FR-009)**: Add SIGTERM handler.
3. **Update FR-011**: Reflect actual CUDA IPC mechanism (64-byte opaque handle + size).
