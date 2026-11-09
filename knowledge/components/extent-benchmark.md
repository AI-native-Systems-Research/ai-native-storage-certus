# extent-benchmark

**Crate**: `extent-benchmark`
**Path**: `apps/extent-benchmark/`
**Type**: Application (not a component)

## Description

End-to-end extent manager benchmark. Wires up the full storage stack: SPDK environment, data and metadata block devices, and the extent manager. Formats the extent manager, initializes it, then benchmarks `reserve_extent` + `publish` throughput under configurable concurrency using a `Barrier` to synchronize worker threads.

## Component Wiring

```
SPDKEnvComponent ---[ISPDKEnv]---> BlockDeviceSpdkNvmeComponentV1 (data)
                 \--[ISPDKEnv]---> BlockDeviceSpdkNvmeComponentV1 (metadata)
                                        |                    |
                                   [IBlockDevice]       [IBlockDevice]
                                        \                  /
                                    ExtentManagerV2
                                   [IExtentManager] ---> reserve_extent + publish
```

## Build

```bash
cargo build -p extent-benchmark --release
```
