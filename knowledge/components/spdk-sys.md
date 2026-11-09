# spdk-sys

**Crate**: `spdk-sys`
**Path**: `lib/spdk-sys/`
**Version**: 0.1.0

## Description

Raw `bindgen`-generated FFI bindings to the SPDK C libraries. Exposes NVMe controller management, environment initialization, PCI device enumeration, and DMA memory allocation functions. All functions are `unsafe`. This is not a component -- it is a pure FFI layer.

The bindings are generated at build time from `wrapper.h` (which includes `spdk/env.h`, `spdk/env_dpdk.h`, `spdk/nvme.h`) using the SPDK headers installed at `deps/spdk-build/include/`.

## Key Function Groups

### Environment
- `spdk_env_opts_init`, `spdk_env_init`, `spdk_env_fini`

### PCI Device Enumeration
- `spdk_pci_enumerate`, `spdk_pci_for_each_device`
- `spdk_pci_device_get_addr`, `spdk_pci_device_get_id`, `spdk_pci_device_get_numa_id`, etc.

### NVMe Controller
- `spdk_nvme_probe`, `spdk_nvme_detach`
- `spdk_nvme_ctrlr_get_num_ns`, `spdk_nvme_ctrlr_get_ns`
- `spdk_nvme_ctrlr_alloc_io_qpair`, `spdk_nvme_ctrlr_free_io_qpair`
- `spdk_nvme_ctrlr_reset`, `spdk_nvme_ctrlr_get_data`, `spdk_nvme_ctrlr_get_id`

### NVMe Namespace Management
- `spdk_nvme_ctrlr_create_ns`, `spdk_nvme_ctrlr_attach_ns`
- `spdk_nvme_ctrlr_delete_ns`, `spdk_nvme_ctrlr_format`

### NVMe Namespace
- `spdk_nvme_ns_is_active`, `spdk_nvme_ns_get_sector_size`
- `spdk_nvme_ns_get_num_sectors`, `spdk_nvme_ns_get_size`

### NVMe I/O
- `spdk_nvme_ns_cmd_read`, `spdk_nvme_ns_cmd_write`, `spdk_nvme_ns_cmd_write_zeroes`
- `spdk_nvme_qpair_process_completions`

### DMA Memory
- `spdk_dma_zmalloc`, `spdk_dma_free`, `spdk_zmalloc`, `spdk_free`

## Key Types
- `spdk_nvme_ctrlr`, `spdk_nvme_ns`, `spdk_nvme_qpair` (opaque)
- `spdk_nvme_ctrlr_data` (opaque)
- `spdk_nvme_ns_data`, `spdk_nvme_format`, `spdk_nvme_ctrlr_list`
- `spdk_pci_addr`, `spdk_pci_id`, `spdk_nvme_transport_id`

## Interfaces Provided

None (FFI crate).

## Receptacles

None.
