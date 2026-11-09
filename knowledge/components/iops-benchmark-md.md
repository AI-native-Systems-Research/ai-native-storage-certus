# iops-benchmark-md

**Crate**: `iops-benchmark-md`
**Path**: `apps/iops-benchmark-md/`
**Type**: Application (not a component)

## Description

Multi-device variant of the IOPS benchmark. Supports multiple NVMe controllers simultaneously, distributing client threads across devices. Each actor thread and each client thread is pinned to a distinct CPU core. Reports per-device and aggregate statistics.

## Component Wiring

```
SPDKEnvComponent ---[ISPDKEnv]---> BlockDeviceSpdkNvmeComponent (device 1)
                 \--[ISPDKEnv]---> BlockDeviceSpdkNvmeComponent (device 2)
                                   ...
                                   Each device: [IBlockDevice] ---> N x ClientChannels
```

## Build

```bash
cargo build -p iops-benchmark-md --release
```
