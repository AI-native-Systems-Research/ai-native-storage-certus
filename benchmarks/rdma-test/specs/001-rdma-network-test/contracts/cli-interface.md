# CLI Interface Contract: rdma-test

## Command Structure

```
rdma-test [GLOBAL OPTIONS] <SUBCOMMAND> <SUBCOMMAND OPTIONS>
```

## Global Options

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--device` | `-d` | String | auto-detect | RDMA device name |
| `--port` | `-p` | u16 | 7471 | Connection management port |
| `--size` | `-s` | usize | 4096 | Message size in bytes (>0) |
| `--iterations` | `-n` | u64 | 10000 | Number of test iterations (>0) |
| `--test` | `-t` | Enum | all | Test type: write, read, send, recv, latency, all |
| `--warmup` | `-w` | u64 | 100 | Warmup iterations |
| `--output` | `-o` | Enum | human | Output format: human, json |

## Subcommands

### `server`

Listen for incoming RDMA connections.

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--address` | `-a` | String | 0.0.0.0 | Address to bind to |

### `client`

Connect to a server and run benchmarks.

| Flag | Short | Type | Default | Description |
|------|-------|------|---------|-------------|
| `--address` | `-a` | String | *required* | Server address to connect to |

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Test completed successfully |
| 1 | RDMA error or test failure |
| 2 | Invalid arguments / validation error |

## Output Contract (Human-Readable)

For bandwidth test kinds, the section heading's parenthetical label reflects which test ran: `RDMA Write` (`--test write`), `RDMA Read` (`--test read`), `Send` (`--test send`), or `Recv` (`--test recv`). Under `--test all`, one such section is printed per test kind, in the order write, read, send, recv, latency.

```
=== RDMA Device Check ===
  libibverbs: found
  Devices found: 1
    mlx5_0 (RoCE) - 4: ACTIVE

=== RDMA Throughput Test (RDMA Write) ===
  Message size: 4096 bytes
  Iterations:   10000
  Elapsed:      0.328 s
  Bandwidth:    12.45 GB/s
  Message rate: 3.04 Mmsg/s
  Total data:   39.06 MB

=== RDMA Latency Test (Send/Recv) ===
  Message size: 64 bytes
  Samples: 10000
  Min:     1.23 us
  Max:     15.67 us
  Mean:    1.89 us
  Median:  1.78 us
  P95:     2.34 us
  P99:     4.56 us
  Jitter:  0.45 us (stddev)
```

## Output Contract (JSON)

The `results` object contains one key per test kind that was actually run (`write`, `read`, `send`, `recv`, `latency`); keys for test kinds that were not selected are omitted entirely (not `null`). Under `--test all` (the default) all five keys are present; under a single test kind (e.g. `--test write`) only that one key is present.

```json
{
  "device": "mlx5_0",
  "transport": "RoCE",
  "test": "all",
  "config": {
    "message_size": 4096,
    "iterations": 10000,
    "warmup": 100
  },
  "results": {
    "write": {
      "bandwidth_gbps": 12.45,
      "message_rate_mpps": 3.04,
      "total_bytes": 40960000,
      "elapsed_seconds": 0.328
    },
    "read": {
      "bandwidth_gbps": 11.80,
      "message_rate_mpps": 2.88,
      "total_bytes": 40960000,
      "elapsed_seconds": 0.347
    },
    "send": {
      "bandwidth_gbps": 10.95,
      "message_rate_mpps": 2.67,
      "total_bytes": 40960000,
      "elapsed_seconds": 0.374
    },
    "recv": {
      "bandwidth_gbps": 10.90,
      "message_rate_mpps": 2.66,
      "total_bytes": 40960000,
      "elapsed_seconds": 0.376
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

When `"partial": true`, results reflect only completed iterations before failure. The `"error"` field contains a description string.

> **Known gap** (tracked in `.specify/sync/align-tasks.md`): as of this writing, `"partial"` is never actually set to `true` by the implementation, and a mid-test failure aborts without emitting this JSON object at all. Treat the `partial`/`error` fields as the target contract, not yet the observed behavior.

## Launch Script Contract

```
scripts/launch.sh <server_host> <client_host> [rdma-test options...]
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RDMA_TEST_BIN` | rdma-test | Path to binary on remote hosts |
| `RDMA_TEST_PORT` | 7471 | Port number |
| `RDMA_TEST_STARTUP_DELAY` | 2 | Seconds to wait for server startup |

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Test completed successfully |
| 1 | Test or connection failure |
| Non-zero | Propagated from client process |
