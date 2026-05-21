# CLI Contract: iops-benchmark

**Date**: 2026-04-15

## Usage

```
iops-benchmark [OPTIONS]
```

## Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--op` | `read\|write\|rw` | `read` | Operation type |
| `--block-size` | bytes (u64) | `4096` | IO block size in bytes |
| `--queue-depth` | u32 | `32` | Outstanding IOs per thread |
| `--threads` | u32 | `1` | Number of concurrent client threads |
| `--duration` | seconds (u64) | `10` | Test duration |
| `--ns-id` | u32 | `1` | NVMe namespace ID |
| `--pci-addr` | string | (first device) | NVMe controller PCI BDF address |
| `--pattern` | `random\|sequential` | `random` | IO access pattern |
| `--quiet` | flag | off | Suppress per-second progress |
| `--help` | flag | — | Print usage and exit |

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Benchmark completed successfully |
| 1 | Validation error (invalid parameters) |
| 2 | Fatal error (device not found, SPDK init failed, DMA alloc failed) |

## Output Format

### Config Summary (stdout, at startup)

```
=== IOPS Benchmark ===
Device:       0000:03:00.0
Namespace:    1 (953869 sectors, 512B sectors)
Operation:    read
Pattern:      random
Block size:   4096 bytes
Queue depth:  32
Threads:      1
Duration:     10 seconds
```

### Progress (stderr, per-second, unless --quiet)

```
[  1s] 145230 IOPS
[  2s] 148102 IOPS
[  3s] 147856 IOPS
...
```

### Final Report (stdout, after completion)

```
=== Results ===
Duration:     10.00 seconds
Total ops:    1,478,234
Total IOPS:   147,823
Throughput:   577.4 MB/s
Errors:       0

Latency (us):
  min:    2.1
  mean:   6.8
  p50:    5.9
  p99:    18.4
  max:    142.7
```

### Final Report in rw Mode (stdout)

```
=== Results ===
Duration:     10.00 seconds
Read ops:     742,118  (74,211 IOPS)
Write ops:    736,116  (73,611 IOPS)
Total ops:    1,478,234
Total IOPS:   147,823
Throughput:   577.4 MB/s
Errors:       0

Latency (us):
  min:    2.1
  mean:   6.8
  p50:    5.9
  p99:    18.4
  max:    142.7
```

## Validation Errors (stderr)

```
error: block-size 1000 is not a multiple of device sector size 512
error: threads must be >= 1
error: duration must be >= 1
warning: queue-depth 512 exceeds device maximum 256, clamping to 256
error: namespace 5 not found (available: 1)
error: no NVMe device found at PCI address 0000:99:00.0
error: no active namespaces on device
```
