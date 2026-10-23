# Data Model: RDMA Network Test Tool

**Date**: 2026-06-05

## Entities

### RdmaDevice

Represents a discovered RDMA-capable network device.

| Field | Type | Description |
|-------|------|-------------|
| name | String | Device name (e.g., "mlx5_0") |
| transport | Enum(InfiniBand, RoCE) | Link layer type, detected from sysfs |
| port_state | Enum(Active, Down, Init) | Current port state |
| port_num | u8 | Port number (typically 1) |
| gid_index | u8 | GID index for addressing |

### TestConfig

Configuration for a benchmark run, derived from CLI arguments.

| Field | Type | Constraints |
|-------|------|-------------|
| mode | Enum(Server, Client) | Required subcommand |
| test_type | Enum(Throughput, Latency, All) | Default: All |
| address | String | Server: bind address; Client: target address |
| port | u16 | Default: 7471 |
| message_size | usize | Must be > 0, default: 4096 |
| iterations | u64 | Must be > 0, default: 10000 |
| warmup | u64 | Default: 100 |
| device | Option<String> | If None, auto-detect first active device |
| output_format | Enum(Human, Json) | Default: Human |

### RdmaConnection

A live RDMA connection between client and server.

| Field | Type | Description |
|-------|------|-------------|
| cm_id | *mut rdma_cm_id | RDMA CM connection identifier |
| pd | *mut ibv_pd | Protection domain |
| cq | *mut ibv_cq | Completion queue (shared send/recv) |
| qp | *mut ibv_qp | Queue pair (RC type) |
| local_mr | *mut ibv_mr | Local memory region for send/write |
| remote_mr_info | RemoteMrInfo | Remote side's rkey + addr (for RDMA Write) |

### RemoteMrInfo

Exchanged during connection setup for one-sided operations.

| Field | Type | Description |
|-------|------|-------------|
| addr | u64 | Remote virtual address |
| rkey | u32 | Remote access key |
| size | u32 | Remote buffer size |

### ThroughputResult

| Field | Type | Description |
|-------|------|-------------|
| total_bytes | u64 | Total bytes transferred |
| elapsed | Duration | Wall-clock time for measured iterations |
| iterations | u64 | Number of completed iterations |
| bandwidth_gbps | f64 | Computed: total_bytes / elapsed / 1e9 |
| message_rate_mpps | f64 | Computed: iterations / elapsed / 1e6 |
| partial | bool | True if test aborted before completion |

### LatencyResult

| Field | Type | Description |
|-------|------|-------------|
| samples | Vec<Duration> | Per-iteration one-way latency (RTT/2) |
| min | Duration | Minimum observed |
| max | Duration | Maximum observed |
| mean | Duration | Arithmetic mean |
| median | Duration | 50th percentile |
| p95 | Duration | 95th percentile |
| p99 | Duration | 99th percentile |
| stddev | Duration | Standard deviation (jitter) |
| partial | bool | True if test aborted before completion |

## State Transitions

### Connection Lifecycle

```
[Init] → resolve_addr → [AddrResolved] → resolve_route → [RouteResolved]
  → connect/accept → [Established] → run_test → [Complete] → disconnect → [Closed]
                                         ↓ (failure)
                                    [Retry 1..3] → [Established] or [Failed]
```

### Server State

```
[Listening] → cm_event(CONNECT_REQUEST) → [Accepting] → cm_event(ESTABLISHED)
  → [Running Test] → [Done] → disconnect → [Exit]
```

### Client State

```
[Connecting] → resolve_addr → resolve_route → connect → cm_event(ESTABLISHED)
  → [Running Test] → [Done] → disconnect → [Exit]
```

## Data Exchange Protocol

1. **Connection setup**: rdmacm handles QP creation and state transitions
2. **MR exchange** (for throughput test): After connection established, server sends its MR info (addr, rkey, size) to client via RDMA Send/Recv
3. **Throughput test**: Client performs RDMA Write to server's MR; server waits for completion signal via Send/Recv
4. **Latency test**: Client sends message, server echoes back (Send/Recv ping-pong)
5. **Completion**: Client sends "done" signal, both sides disconnect
