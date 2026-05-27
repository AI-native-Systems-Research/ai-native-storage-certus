# Spec Drift Report

Generated: 2026-05-05
Project: certus-server

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 14 |
| Aligned | 12 (86%) |
| Drifted | 2 (14%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 1 |

## Detailed Findings

### Spec: 001-grpc-dispatcher-server - gRPC Dispatcher Server

#### Aligned

- FR-001: gRPC service exposes lookup, check, remove, populate; no lifecycle methods in proto -> `proto/dispatcher.proto:6-18`, `src/service.rs:122-276`
- FR-002: Populate accepts list of (key, ipc_handle), returns per-entry results -> `src/service.rs:123-174`
- FR-003: Lookup accepts list of (key, ipc_handle), returns per-entry results -> `src/service.rs:176-225`
- FR-004: Check accepts list of keys, returns list of boolean results -> `src/service.rs:229-249`
- FR-005: Remove accepts list of keys, returns per-entry results -> `src/service.rs:253-275`
- FR-006: CLI accepts metadata PCI, data PCI(s), listen address/port, TLS cert/key paths -> `src/main.rs:27-47`
- FR-007: Per-entry results include key, success, error_code, error_message -> `proto/dispatcher.proto:41-46`, `src/service.rs:102-119`
- FR-008: Server auto-initializes dispatcher on startup using CLI PCI addresses -> `src/main.rs:219`
- FR-010: Python test client exercises all gRPC methods with batch operations -> `python-client/test_client.py`
- FR-013: Multiple connections supported, processing serialized via Mutex -> `src/main.rs:220` (Arc<Mutex<...>>)
- FR-014: Optional TLS via --tls-cert/--tls-key, plaintext when absent -> `src/main.rs:228-234`
- FR-015: Pre-validates batch for duplicate keys, rejects entire batch -> `src/service.rs:37-47`

#### Drifted

- FR-009: Spec says "shut down dispatcher when receiving SIGTERM/SIGINT" but code only handles SIGINT via `tokio::signal::ctrl_c()`. SIGTERM is not caught.
  - Location: `src/main.rs:244`
  - Severity: moderate
  - Fix: Add `tokio::signal::unix::signal(SignalKind::terminate())` alongside ctrl_c

- FR-011: Spec assumes IPC handle contains "a memory address (or opaque identifier) and a size field" transmitted as a 64-bit integer. Implementation uses a 64-byte CUDA IPC handle (`bytes cuda_ipc_handle`) instead of a raw pointer. This is functionally superior (correct for cross-process GPU sharing) but diverges from spec text.
  - Location: `proto/dispatcher.proto:22-27`
  - Severity: minor (spec should be updated to match the better design)

#### Not Implemented

(none)

### Success Criteria

- SC-001: Aligned - Batch operations supported, test client validates with 10-entry batches (4 round-trips)
- SC-002: Aligned - `test_large_batch` exercises 1000 entries without timeout
- SC-003: Aligned - Per-entry error reporting with key, code, and message
- SC-004: Cannot verify statically (runtime startup timing)
- SC-005: Aligned - Python test client has pass/fail output for all scenarios

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| CUDA IPC handle open/close | `src/service.rs:65-99` | 35 | Update 001 FR-011 assumptions |

## Inter-Spec Conflicts

(none - only one spec analyzed)

## Recommendations

1. **Fix SIGTERM handling (FR-009)**: Add a SIGTERM signal handler using `tokio::signal::unix::signal(SignalKind::terminate())` and select on both it and ctrl_c in the shutdown future. This is a moderate gap since `kill <pid>` (default SIGTERM) won't trigger graceful shutdown.
2. **Update spec FR-011 and Assumptions**: The CUDA IPC handle design (`bytes cuda_ipc_handle` + `uint32 size`) is superior to the spec's raw-pointer assumption. Update the spec to reflect the actual cross-process GPU memory sharing mechanism and remove the assumption about "same address space."
