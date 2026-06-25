# Quickstart: RDMA Remote Request Handler

## Prerequisites

- Linux with RDMA-capable NIC (InfiniBand or RoCE) **or** SoftRoCE (rxe) for development
- rdma-core userspace libraries installed (`libibverbs-dev`, `librdmacm-dev` on Debian/Ubuntu; `rdma-core-devel` on RHEL/Fedora)
- Rust stable toolchain (MSRV 1.75)
- protoc (Protocol Buffers compiler) — for proto compilation via prost-build

## Setting up SoftRoCE for Development

```bash
# Load the SoftRoCE kernel module
sudo modprobe rdma_rxe

# Create a software RDMA device on your network interface (e.g., eth0)
sudo rdma link add rxe0 type rxe netdev eth0

# Verify RDMA device is available
ibv_devices
```

## Building

```bash
# From the component directory
cd components/remote-request-handler

# Build the component library
cargo build

# Build the test client
cargo build --bin test-client

# Build with telemetry support
cargo build --features telemetry
```

## Running the Test Client

```bash
# Start a handler instance (requires an executive or test harness)
# The handler listens on a configurable port (e.g., 18515)

# Run the test client against a local handler
cargo run --bin test-client -- --addr 192.168.1.100 --port 18515 --batch-size 16 --iterations 100
```

## Running Tests

```bash
# Unit tests (no hardware required — RDMA calls are mocked)
cargo test

# Integration tests (requires SoftRoCE or RDMA hardware)
cargo test --features integration-test

# All tests single-threaded (for CI or shared hardware)
cargo test -- --test-threads 1
```

## Configuration

The handler is configured at component initialization time:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| port | u16 | (required) | TCP port for rdma_cm listener |
| max_sessions | u32 | 100 | Maximum concurrent connections |
| max_batch_size | u32 | 64 | Maximum entries per lookup batch |

## Profile Integration

To run with the full-remote profile:

```bash
cd apps/certus-server-yaml
cargo run -- --profile full-remote --port 18515
```

## Architecture Overview

```
Remote Node                           This Node (Handler)
┌─────────────┐                      ┌──────────────────────┐
│ Test Client │                      │ RemoteRequestHandler │
│  or Certus  │───rdma_cm connect──→ │                      │
│   Node      │                      │  ┌──────────────┐   │
│             │◀──Handshake────────→ │  │   Listener   │   │
│             │                      │  └──────┬───────┘   │
│             │──BatchLookupReq────→ │         │           │
│             │                      │  ┌──────▼───────┐   │
│  ┌───────┐  │◀─RDMA Write(data)── │  │   Session    │   │
│  │ Memory│  │                      │  │              │   │
│  │ (rkey)│  │◀─BatchLookupResp─── │  │  IDispatcher │   │
│  └───────┘  │                      │  └──────────────┘   │
│             │──CloseReq──────────→ │                      │
│             │◀─CloseResp────────── │                      │
└─────────────┘                      └──────────────────────┘
```

## Key Design Decisions

1. **RDMA Write for data, Send/Recv for control**: Lookup results are written directly to caller memory (zero-copy on caller side); protocol messages use Send/Recv.
2. **Protobuf envelope messages**: All control messages are wrapped in `RequestMessage`/`ResponseMessage` oneof envelopes for multiplexing on a single QP.
3. **No authentication**: Security relies on RDMA fabric isolation (not internet-facing).
4. **Version handshake**: First message must be HandshakeRequest; version mismatch rejects immediately.
5. **64-entry batch limit**: Prevents unbounded resource consumption per request.
