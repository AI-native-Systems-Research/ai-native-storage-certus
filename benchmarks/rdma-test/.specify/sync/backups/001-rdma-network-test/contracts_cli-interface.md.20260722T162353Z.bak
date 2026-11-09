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
| `--test` | `-t` | Enum | all | Test type: throughput, latency, all |
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

When `"partial": true`, results reflect only completed iterations before failure. The `"error"` field contains a description string.

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
