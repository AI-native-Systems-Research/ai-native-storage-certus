# block-device-spdk-nvme (v2)

**Crate**: `block-device-spdk-nvme`
**Path**: `components/block-device-spdk-nvme/`
**Version**: 0.2.0
**Features**: `telemetry` (IO statistics), `spdk-test`

## Description

NVMe block device component using SPDK for direct userspace NVMe controller access. Each instance is associated with a single NVMe controller. The actor thread is pinned to the NUMA node of the controller. Provides both `IBlockDevice` (data path) and `IBlockDeviceAdmin` (lifecycle/configuration) interfaces.

Hot-path optimizations over v1:
- **TSC-based timeout**: Uses hardware Time Stamp Counter (`rdtscp`) for low-overhead deadline checking
- **ContextPool slab allocator**: Eliminates per-IO heap allocation for async completion contexts
- **Scratch buffers**: Pre-allocated vectors for draining completions

## Component Definition

```
BlockDeviceSpdkNvmeComponent {
    version: "0.2.0",
    provides: [IBlockDevice, IBlockDeviceAdmin],
    receptacles: {
        spdk_env: ISPDKEnv,
        logger: ILogger,
    },
}
```

## Interface Definition

```rust
define_interface! {
    pub IBlockDevice {
        fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError>;
        fn sector_size(&self, ns_id: u32) -> Result<u32, NvmeBlockError>;
        fn num_sectors(&self, ns_id: u32) -> Result<u64, NvmeBlockError>;
        fn max_queue_depth(&self) -> u32;
        fn num_io_queues(&self) -> u32;
        fn max_transfer_size(&self) -> u32;
        fn block_size(&self) -> u32;
        fn numa_node(&self) -> i32;
        fn nvme_version(&self) -> String;
        fn telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError>;
    }
}

define_interface! {
    pub IBlockDeviceAdmin {
        fn set_pci_address(&self, addr: PciAddress);
        fn set_actor_cpu(&self, cpu: usize);
        fn initialize(&self) -> Result<(), NvmeBlockError>;
        fn signal_stop(&self);
        fn shutdown(&self) -> Result<(), NvmeBlockError>;
        fn detach_controller(&self);
    }
}
```

## Verified Properties

None. No formal verification model exists for this component's interface.

## Receptacles

| Name | Interface | Required | Purpose |
|------|-----------|----------|---------|
| `spdk_env` | `ISPDKEnv` | Yes | SPDK environment must be initialized first |
| `logger` | `ILogger` | No | Optional debug/info logging |

## Key Types

- `ClientChannels { command_tx: Sender<Command>, completion_rx: Receiver<Completion> }`
- `Command` — `ReadSync`, `WriteSync`, `ReadAsync`, `WriteAsync`, `WriteZeros`, `BatchSubmit`, `AbortOp`, `NsProbe`, `NsCreate`, `NsFormat`, `NsDelete`, `ControllerReset`
- `Completion` — `ReadDone`, `WriteDone`, `WriteZerosDone`, `AbortAck`, `Timeout`, `NsProbeResult`, `NsCreated`, `NsFormatted`, `NsDeleted`, `ResetDone`, `Error`
- `NvmeBlockError` — `FeatureNotEnabled`, `NotInitialized`, `Timeout`, `Aborted`, `InvalidNamespace`, `NotSupported`, `BlockDevice`, `SpdkEnv`, `LbaOutOfRange`, `ClientDisconnected`
- `TelemetrySnapshot { total_ops, min_latency_ns, max_latency_ns, mean_latency_ns, mean_throughput_mbps, elapsed_secs }`
- `OpHandle(u64)` — unique async operation handle
- `NamespaceInfo { ns_id, num_sectors, sector_size }`
