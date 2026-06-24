# Remote Request Handler

RDMA-based endpoint for handling cache lookup requests from peer Certus nodes. A remote node connects via rdma_cm, submits batched lookup requests over a protobuf protocol, and receives results written directly into its memory via RDMA Write.

## Architecture

```
Remote Node (Client)                    This Node (Handler)
┌──────────────────┐                   ┌─────────────────────────────┐
│   test-client    │                   │  remote-request-handler     │
│   or Certus peer │                   │                             │
│                  │──rdma_cm connect─→│  ┌───────────────────────┐  │
│                  │◀─Handshake──────→ │  │      Listener         │  │
│                  │                   │  │  (SessionRegistry)    │  │
│                  │──BatchLookupReq──→│  └──────────┬────────────┘  │
│                  │                   │             │               │
│  ┌────────────┐  │◀─RDMA Write(data)─│  ┌──────────▼────────────┐  │
│  │Result Bufs │  │                   │  │      Session          │  │
│  │ (rkey)     │  │◀─LookupResponse─-─│  │  state machine +      │  │
│  └────────────┘  │                   │  │  batch processing     │  │
│                  │──CloseReq────────→│  │         │             │  │
│                  │◀─CloseResp─────=──│  │         ▼             │  │
└──────────────────┘                   │  │    IDispatcher        │  │
                                       │  └───────────────────────┘  │
                                       └─────────────────────────────┘
```

### Module Layout

| Module                  | Role                                                                                                                                                          |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ffi.rs`                | Raw FFI bindings to libibverbs and librdmacm                                                                                                                  |
| `wrapper.c`             | C helpers for inline ibverbs functions (post_send, post_recv, poll_cq, rdma_write)                                                                            |
| `rdma.rs`               | Safe Rust wrappers: `RdmaListener`, `RdmaConnection`, `MemoryRegion` with RAII cleanup. Mock implementation for unit tests                                    |
| `protocol.rs`           | Protobuf encode/decode via prost. Envelope messages (`RequestMessage`/`ResponseMessage`) multiplex handshake, lookup, and close on a single QP                |
| `session.rs`            | Per-connection state machine: `Connecting → Handshake → Active → Closing → Closed`. Batch validation (max 64 entries), dispatch resolution, response assembly |
| `listener.rs`           | `SessionRegistry` tracking active sessions with max-sessions enforcement                                                                                      |
| `telemetry.rs`          | Feature-gated (`telemetry`) atomic counters for connection rates, throughput, and latency                                                                     |
| `bin/handler_server.rs` | Standalone server binary for testing                                                                                                                          |
| `bin/test_client.rs`    | CLI client that exercises the full connect → handshake → lookup → close path                                                                                  |

### Protocol

Defined in `proto/remote_request.proto`. Uses protobuf over RDMA Send/Recv for control messages:

1. **Handshake** — version check, max batch size advertisement
2. **BatchLookupRequest** — up to 64 entries, each with a 64-bit CacheKey and remote memory target (addr + 32-bit rkey)
3. **BatchLookupResponse** — per-entry success/failure with bytes_written
4. **Close** — graceful teardown with session statistics

Data transfer (lookup results) uses RDMA Write directly into the caller's registered memory buffers — zero-copy on the client side.

### Security Model

Network-level trust only. No application-level authentication. The handler assumes it runs on an isolated RDMA fabric not reachable from untrusted networks.

## Prerequisites

- Linux with RDMA-capable NIC (InfiniBand or RoCE)
- rdma-core userspace libraries: `libibverbs-dev`, `librdmacm-dev` (Debian/Ubuntu) or `rdma-core-devel` (RHEL/Fedora)
- Rust stable toolchain (MSRV 1.75)

## Build

```bash
# Component library + both binaries
cargo build -p remote-request-handler

# Release mode (for benchmarking)
cargo build -p remote-request-handler --release

# With telemetry counters enabled
cargo build -p remote-request-handler --features telemetry

# As part of certus-server-yaml (full-remote profile)
CERTUS_PROFILE=full-remote cargo build -p certus-server-yaml
```

## Test

```bash
# Unit tests (no RDMA hardware required — uses mock RdmaOps)
cargo test -p remote-request-handler

# Unit tests including telemetry module
cargo test -p remote-request-handler --features telemetry

# Clippy + format check
cargo clippy -p remote-request-handler -- -D warnings
cargo fmt -p remote-request-handler --check
```

### Integration Test (requires RDMA hardware)

On the server node (e.g., 10.0.0.100):

```bash
cargo run -p remote-request-handler --release --bin handler-server -- \
    --addr 10.0.0.100 --port 18515
```

On the client node (e.g., 10.0.0.101):

```bash
cargo run -p remote-request-handler --release --bin test-client -- \
    --addr 10.0.0.100 --port 18515 --batch-size 64 --iterations 1000
```

Both binaries can also run on the same RDMA-capable host (loopback over the RDMA interface).

### End-to-End Test with certus-server-yaml (requires RDMA + GPU)

Start certus-server-yaml with the full-remote profile:

```bash
CERTUS_PROFILE=full-remote cargo build -p certus-server-yaml --release

target/release/certus-server-yaml --device-path /dev/null --rdma-port 18515
```

Run the Python end-to-end test (populates cache via gRPC, lookups via RDMA):

```bash
cd apps/python
python3 test-remote.py \
    --grpc-server localhost:50051 \
    --rdma-server 10.0.0.100 \
    --rdma-port 18515 \
    --object-size 4M \
    --batch-size 16 \
    --iterations 5 \
    --check-integrity
```

`test-remote.py` options:

| Parameter          | Default            | Description                              |
| ------------------ | ------------------ | ---------------------------------------- |
| `--grpc-server`    | `localhost:50051`  | gRPC endpoint for cache populate         |
| `--rdma-server`    | `localhost`        | RDMA handler address                     |
| `--rdma-port`      | `18515`            | RDMA handler port                        |
| `--object-size`    | `4M`               | Size per cache object (e.g. 128K, 4M, 1G)|
| `--batch-size`     | `16`               | Entries per RDMA lookup batch            |
| `--iterations`     | `10`               | Number of RDMA lookup iterations         |
| `--check-integrity`| disabled           | Verify all lookups resolve successfully  |
| `--gpu-device`     | `0`                | CUDA device ordinal for populate         |

Example output:

```
Phase 1: Populate (gRPC):   80/80 objects, 0.015 GB/s
Phase 2: Lookup (RDMA):     282.6 us/batch, 17.7 us/entry, 221.004 GB/s
Phase 3: Integrity Check:   [✓] PASS: All 80 lookups succeeded
```

## Profile / Benchmark

The test client reports per-batch and per-entry latency:

```bash
# Throughput test: max batch size, many iterations, release mode
cargo run -p remote-request-handler --release --bin test-client -- \
    --addr <handler-ip> --port 18515 \
    --batch-size 64 --iterations 10000
```

Example output on ConnectX-6 (loopback):

```
Completed 1000 iterations (64000 total entries) in 121.190ms
Average: 121.2 us/batch, 1.9 us/entry
```

To measure with telemetry (server-side metrics):

```bash
cargo run -p remote-request-handler --release --features telemetry --bin handler-server -- \
    --addr 0.0.0.0 --port 18515
```

### Key Metrics

| Metric        | Description                                                                         |
| ------------- | ----------------------------------------------------------------------------------- |
| us/batch      | End-to-end latency for one 64-entry batch (Send request + dispatch + Send response) |
| us/entry      | Per-lookup amortized cost                                                           |
| connections/s | Session establishment rate (with telemetry feature)                                 |
| bytes/s       | Data transfer throughput (with telemetry feature)                                   |

## Configuration

| Parameter      | Binary | Default                 | Description                  |
| -------------- | ------ | ----------------------- | ---------------------------- |
| `--addr`       | both   | `0.0.0.0` / `127.0.0.1` | Bind/connect address         |
| `--port`       | both   | `18515`                 | rdma_cm listener port        |
| `--batch-size` | client | `16`                    | Entries per batch (max 64)   |
| `--iterations` | client | `1`                     | Number of batch rounds       |
| `--client-id`  | client | `test-client`           | Identifier sent in handshake |
