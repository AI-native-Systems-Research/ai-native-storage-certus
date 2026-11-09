# Extent Manager Benchmark

Measures latency and throughput of the ExtentManagerComponentV0's core operations:
**create_extent**, **lookup_extent**, and **remove_extent**.

Wires the full SPDK component stack (Logger, SPDKEnv, BlockDeviceSpdkNvme, ExtentManager)
and supports multi-threaded operation with per-phase latency percentiles and aggregate
throughput reporting.

## Prerequisites

1. Linux x86_64 system with an NVMe SSD
2. SPDK prerequisites configured:
   - Hugepages allocated (`echo 1024 > /proc/sys/vm/nr_hugepages`)
   - VFIO or UIO driver bound to the NVMe device (`setup.sh` from SPDK)
3. Rust stable toolchain (MSRV 1.75)
4. SPDK native libraries built at `deps/spdk-build/`

## Build

```bash
cargo build -p extent-benchmark --release
```

## Usage

### Single-threaded benchmark (default: 10K ops)

```bash
sudo ./target/release/extent-benchmark --device 0000:03:00.0
```

### Multi-threaded benchmark

```bash
sudo ./target/release/extent-benchmark --device 0000:03:00.0 --threads 4 --count 50000
```

### Custom size class and slab size

```bash
sudo ./target/release/extent-benchmark \
  --device 0000:03:00.0 \
  --threads 8 \
  --count 100000 \
  --size-class 262144 \
  --slab-size 2147483648
```

## CLI Reference

| Argument | Type | Default | Description |
|----------|------|---------|-------------|
| `--device` | String | **required** | NVMe device PCI address (e.g., `0000:03:00.0`) |
| `--ns-id` | u32 | 1 | NVMe namespace ID |
| `--threads` | usize | 1 | Number of worker threads |
| `--count` | u64 | 10000 | Operations per benchmark phase |
| `--size-class` | u32 | 131072 | Extent size class in bytes (128 KiB - 5 MiB, 4KiB-aligned) |
| `--slab-size` | u32 | 1073741824 | Slab size for extent manager (>= 8 KiB, 4KiB-aligned) |
| `--total-size` | u64 | auto-detect | Total managed space in bytes |

## Output Format

```
=== Extent Manager Benchmark ===
Device: 0000:03:00.0 (ns_id=1)
Threads: 4 | Count: 10000 | Size class: 131072
Slab size: 1073741824 | Total size: 107374182400

--- Create Phase ---
  Total ops:   10000
  Elapsed:     1.234s
  Throughput:  8103 ops/sec
  Latency (10000 samples):
    min:       45 us
    mean:     120 us
    p50:      112 us
    p99:      890 us
    max:     2340 us

--- Lookup Phase ---
  ...

--- Remove Phase ---
  ...

=== Summary ===
Total extents created:  10000
Total extents removed:  10000
```

In multi-threaded mode, per-thread latency breakdowns appear under each phase.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Benchmark completed successfully |
| 1 | Configuration error (invalid arguments, device not found) |
| 2 | SPDK initialization failure |
