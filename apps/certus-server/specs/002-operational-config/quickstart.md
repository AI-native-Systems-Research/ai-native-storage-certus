# Quickstart: Operational Configuration

Configure certus-server for different deployment scenarios using CLI flags.

## Prerequisites

1. SPDK environment configured (hugepages, IOMMU, device unbound from kernel driver)
2. CUDA toolkit installed (for GPU DMA registration)
3. NVMe devices available

## Basic Usage

```bash
# Auto-select 2 NVMe drives, default 2 GiB memory tier
certus-server --drive-count 2

# Explicit PCI addresses
certus-server --device-pci 0000:02:00.0 --device-pci 0000:03:00.0
```

## Options

| Option | Description |
|--------|-------------|
| `--drive-count N` | Auto-select first N NVMe drives (NUMA-0 preferred) |
| `--device-pci ADDR` | Explicit PCI address (repeatable) |
| `--memory-tier-size SIZE` | DRAM pool size (e.g., `512M`, `2G`) |
| `--format` | Format extent managers (destroys data) |
| `--poller-base-cpu N` | Pin poller threads starting at core N |
| `--max-eviction-attempts N` | Max eviction retries (default 2048) |

## Examples

### First-time setup (format drives)

```bash
certus-server --drive-count 4 --format --memory-tier-size 4G
```

### Production restart (recover existing data)

```bash
certus-server --drive-count 4 --memory-tier-size 4G --poller-base-cpu 2
```

### Development (single drive, small pool)

```bash
certus-server --drive-count 1 --memory-tier-size 256M --format
```

### With TLS and telemetry

```bash
certus-server --drive-count 4 \
  --tls-cert /etc/certus/cert.pem --tls-key /etc/certus/key.pem \
  --otel-endpoint http://localhost:4317
```

## Tips

- Use `--format` only on first deployment or when you want to destroy all cached data.
- Set `--poller-base-cpu` to cores in the same NUMA zone as your NVMe drives for best performance.
- The `--drive-count` flag discovers drives after SPDK init — ensure devices are unbound from the kernel driver first.
