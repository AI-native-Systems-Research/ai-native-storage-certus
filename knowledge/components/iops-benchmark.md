# iops-benchmark

**Crate**: `iops-benchmark`
**Path**: `apps/iops-benchmark/`
**Type**: Application (not a component)

## Description

Multi-threaded NVMe IOPS/throughput/latency benchmark. Each worker thread gets its own `ClientChannels` from `IBlockDevice::connect_client()`. Worker and actor threads are NUMA-pinned. Measures and reports IOPS, MB/s, and latency percentiles (min, mean, p50, p99, max) per thread and aggregate.

## CLI Arguments

- `--pci-addr <BDF>` -- target NVMe controller
- `--driver <v1|v2>` -- block device driver version
- `--op <read|write|rw>` -- operation type
- `--io-mode <sync|async>` -- IO submission mode (default: async)
- `--queue-depth <N>` -- async queue depth (default: 32)
- `--block-size <bytes>` -- I/O block size (default: 4096)
- `--threads <N>` -- number of worker threads (default: 1)
- `--duration <secs>` -- benchmark duration (default: 10)
- `--pattern <random|sequential>` -- LBA access pattern (default: random)
- `--ns-id <N>` -- target NVMe namespace (default: 1)
- `--quiet` -- suppress per-second progress output

## Component Wiring

```
SPDKEnvComponent ---[ISPDKEnv]---> BlockDeviceSpdkNvmeComponent
                                        |
                                   [IBlockDevice] ---> N x ClientChannels (one per worker)
                                   [IBlockDeviceAdmin] ---> initialize
```

## Build

```bash
cargo build -p iops-benchmark --release
```
