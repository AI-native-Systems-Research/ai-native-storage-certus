# Quickstart: RDMA Network Test Tool

## Prerequisites

### System Packages (RHEL/Fedora)

```bash
sudo dnf install rdma-core-devel librdmacm-devel libibverbs-devel
```

### Verify RDMA Device

```bash
ibv_devices        # List RDMA devices
ibv_devinfo        # Show device details
ibstat             # Port status
```

If no devices appear, load SoftRoCE for testing without hardware:

```bash
sudo modprobe rdma_rxe
sudo rdma link add rxe0 type rxe netdev eth0
```

## Build

```bash
cd tools/rdma-test
cargo build --release
```

## Usage

### Basic Throughput + Latency Test

On server node:
```bash
./target/release/rdma-test server
```

On client node:
```bash
./target/release/rdma-test client --address <server-ip>
```

### Throughput Only (Large Messages)

The `-t`/`--test` flag accepts `write`, `read`, `send`, `recv`, `latency`, or `all` (default). `write` and `read` are one-sided RDMA operations; `send` and `recv` are two-sided bandwidth tests using `ibv_post_send`/`ibv_post_recv`.

```bash
# Server
rdma-test server -t write

# Client
rdma-test client -a 10.0.0.1 -t write -s 65536 -n 50000
```

Other bandwidth variants work the same way, e.g. `-t read`, `-t send`, `-t recv`.

### Latency Only (Small Messages)

```bash
# Server
rdma-test server -t latency

# Client
rdma-test client -a 10.0.0.1 -t latency -s 64 -n 100000
```

### JSON Output (for CI)

```bash
rdma-test client -a 10.0.0.1 --output json | jq .results.write.bandwidth_gbps
```

### Remote Launch via SSH

```bash
./scripts/launch.sh server-host client-host --size 4096 --iterations 10000
```

## Troubleshooting

| Symptom | Solution |
|---------|----------|
| "libibverbs not found" | Install `rdma-core-devel` |
| "No RDMA devices" | Load driver (`modprobe mlx5_ib`) or configure SoftRoCE |
| Connection timeout | Check port 7471 is open, verify fabric connectivity with `ibping` |
| Low throughput | Try larger message sizes (`-s 65536`), check MTU with `ibv_devinfo` |
