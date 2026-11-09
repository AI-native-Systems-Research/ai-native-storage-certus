# nvme-ns-manager

Interactive NVMe namespace management tool built on the `block-device-spdk-nvme` component. It uses SPDK for direct userspace NVMe controller access, allowing you to list, create, format, and delete namespaces without a kernel driver.

## Features

- List active namespaces with sector count, sector size, and human-readable capacity
- Create namespaces with a specified sector count or using all remaining capacity
- Format namespaces with a specific LBA format (e.g., lbaf=2 for 4KiB sectors)
- Delete namespaces
- Supports both v1 and v2 block device driver implementations
- PCI BDF address selection (defaults to the first NVMe device)

## Prerequisites

SPDK must be built and installed before compiling. From the repository root:

```bash
deps/install_deps.sh            # System packages (sudo, RHEL/Fedora)
pip install -r deps/requirements.txt
deps/build_spdk.sh              # Build SPDK into deps/spdk-build/
```

The system must also have hugepages configured and IOMMU enabled. See the top-level README.md for kernel boot parameter details.

## Build

```bash
cargo build -p nvme-ns-manager --release
```

## Usage

```bash
# Use the first available NVMe device (v2 driver)
target/release/nvme-ns-manager

# Specify a PCI address
target/release/nvme-ns-manager --pci-addr 0000:d9:00.0

# Use the v1 driver
target/release/nvme-ns-manager --driver v1
```

The tool presents an interactive menu:

```
  1) List namespaces
  2) Create namespace
  3) Create namespace (use all remaining capacity)
  4) Format namespace
  5) Delete namespace
  6) Quit
```

### Workflow: create a 4KiB-sector namespace

Namespaces are always created with the default LBA format (lbaf=0, typically 512B sectors). To use 4KiB sectors, create the namespace first then format it with the desired LBA format index:

1. Select **3** (create namespace, all remaining capacity)
2. Select **4** (format namespace), enter the namespace ID, then enter the LBAF index (e.g., `2` for 4KiB on many controllers)
3. Select **1** (list) to verify the new sector size

The supported LBAF indices are device-specific. Common mappings:

| LBAF | Typical sector size |
|------|-------------------|
| 0    | 512 B             |
| 1    | 4096 B            |
| 2    | 4096 B            |

Use `nvme id-ns /dev/nvmeX` (with the kernel driver) to inspect the full LBA format table for your device.
