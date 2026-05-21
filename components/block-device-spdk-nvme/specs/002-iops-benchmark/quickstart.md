# Quickstart: IOPS Benchmark

## Prerequisites

1. Linux host with hugepages configured and NVMe device bound to VFIO/UIO
2. SPDK built at `deps/spdk-build/` (run `deps/build_spdk.sh`)
3. Rust stable toolchain (1.75+)

## Build

```bash
cargo build -p iops-benchmark --release
```

## Run (default parameters)

```bash
# 4KB random reads, QD=32, 1 thread, 10 seconds, first available device
sudo ./target/release/iops-benchmark
```

## Run (custom parameters)

```bash
# 64KB sequential writes, QD=64, 4 threads, 30 seconds, specific device
sudo ./target/release/iops-benchmark \
  --op write \
  --block-size 65536 \
  --queue-depth 64 \
  --threads 4 \
  --duration 30 \
  --pci-addr 0000:03:00.0 \
  --pattern sequential

# Mixed read/write, quiet mode (no progress)
sudo ./target/release/iops-benchmark --op rw --quiet
```

## Interpret Results

- **IOPS**: IO operations per second (higher is better)
- **Throughput**: IOPS * block_size, in MB/s
- **Latency p50**: Median latency — half of all IOs complete faster than this
- **Latency p99**: Tail latency — 99% of IOs complete faster than this
- **Errors**: Should be 0 on healthy hardware; non-zero indicates device issues

## Compare with fio

```bash
# Equivalent fio command for 4KB random reads, QD=32, 1 thread, 10 seconds
sudo fio --name=test --ioengine=spdk --filename="trtype=PCIe traddr=0000.03.00.0 ns=1" \
  --rw=randread --bs=4k --iodepth=32 --numjobs=1 --runtime=10 --time_based
```
