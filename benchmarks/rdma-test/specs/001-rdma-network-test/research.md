# Research: RDMA Network Test Tool

**Date**: 2026-06-05

## Decision 1: RDMA Crate / Binding Approach

**Decision**: Manual FFI bindings (hand-written `ffi.rs` linking to system libibverbs/librdmacm)

**Rationale**: 
- `async-rdma` 0.5.0 (crates.io) has yanked transitive dependencies (`simd-abstraction`)
- `async-rdma` git master's underlying `rdma-sys` uses bindgen 0.59.2 which panics on the system's MLNX OFED 2510.x headers (anonymous union naming incompatibility)
- `ibverbs` crate (jonhoo) downloads and builds its own rdma-core source via cmake, adding build complexity
- Manual FFI bindings: zero abstraction overhead (ideal for benchmarking), no dependency chain issues, full control over RDMA path, builds on any system with libibverbs-devel installed

**Alternatives considered**:
- `async-rdma` 0.5.0 — broken dependency chain on crates.io
- `async-rdma` git master — bindgen crash on MLNX OFED headers
- `ibverbs` (jonhoo) — viable but downloads/builds its own rdma-core; overkill for known-good system installs
- `rdma-sys` directly — same bindgen issue as async-rdma

## Decision 2: Connection Management

**Decision**: Use librdmacm (RDMA Connection Manager) for connection setup

**Rationale**:
- rdmacm handles address resolution, route resolution, and QP state transitions automatically
- Works transparently across IB and RoCE (handles GID resolution for RoCE)
- Standard approach used by perftest tools (ib_write_bw, ib_write_lat)
- Alternative (manual QP exchange over TCP sockets) requires implementing IB-to-RoCE differences manually

**Alternatives considered**:
- Manual TCP socket for QP info exchange — more code, fragile across IB/RoCE boundary
- Out-of-band configuration file — not practical for dynamic testing

## Decision 3: Async vs Synchronous RDMA Operations

**Decision**: Synchronous polling (busy-poll CQ) for benchmark measurements; tokio only for connection management timeout handling

**Rationale**:
- RDMA benchmarks must minimize measurement noise; async runtime scheduling adds jitter
- Standard perftest tools use synchronous CQ polling for accurate latency measurement
- Connection setup (rdmacm events) benefits from timeout handling via tokio
- Data path (post_send/poll_cq loop) is pure synchronous for accuracy

**Alternatives considered**:
- Fully async (tokio + io_uring-style completion) — adds latency jitter from task scheduling
- Fully synchronous (no tokio) — harder to implement connection timeouts cleanly

## Decision 4: Memory Registration Strategy

**Decision**: Pre-allocate and register one MR per direction at connection time; reuse across all iterations

**Rationale**:
- MR registration is expensive (kernel call, page pinning); must not be in measurement path
- Single MR of configured message size, registered with LOCAL_WRITE | REMOTE_WRITE | REMOTE_READ
- For throughput: client registers local MR, server registers remote-writable MR, exchange rkey/addr via Send/Recv
- For latency: both sides register same-size MR for Send/Recv operations

**Alternatives considered**:
- Per-iteration MR allocation — unacceptable overhead for benchmarking
- Memory pool with multiple MRs — unnecessary for single-stream single-buffer design

## Decision 5: Retry and Partial Results

**Decision**: 3 retries at the connection/RDMA operation level; report partial results on final failure

**Rationale**:
- Per spec clarification: retry up to 3 times, then abort with partial results
- Retry scope: connection failures retry the full connection; mid-test CQ errors retry the individual operation
- Partial results: if N iterations completed before failure, report stats for those N iterations with a warning

**Alternatives considered**:
- No retry (immediate abort) — wastes partial data
- Unlimited retry — could hang indefinitely on fabric issues

## Decision 6: JSON Output Structure

**Decision**: Structured JSON with nested objects for test metadata and results

**Rationale**:
- Machine-parseable for CI integration (per spec FR-011)
- Structure mirrors human-readable output sections
- Uses serde_json for serialization

**Schema**:
```json
{
  "device": "mlx5_0",
  "transport": "RoCE",
  "test": "throughput|latency",
  "config": {
    "message_size": 4096,
    "iterations": 10000,
    "warmup": 100
  },
  "results": {
    "throughput": {
      "bandwidth_gbps": 12.45,
      "message_rate_mpps": 3.04,
      "total_bytes": 40960000,
      "elapsed_seconds": 0.328
    },
    "latency": {
      "min_us": 1.23,
      "max_us": 15.67,
      "mean_us": 1.89,
      "median_us": 1.78,
      "p95_us": 2.34,
      "p99_us": 4.56,
      "jitter_us": 0.45,
      "samples": 10000
    }
  },
  "partial": false,
  "error": null
}
```
