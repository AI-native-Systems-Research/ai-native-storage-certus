# dispatcher-p2p

P2P-capable dispatcher for Certus with runtime path selection between GPUDirect Storage (P2P) and traditional host-bounce (DRAM) data paths.

## Architecture

### P2P Path (NVMe → GPU BAR1 → D2D)

```
NVMe SSD ──DMA──→ GPU BAR1 staging ring (64 pre-pinned slots) ──D2D copy──→ final GPU buffer
```

- **Pre-pinned staging ring**: 64 × 128KiB GPU buffers allocated via `cudaMalloc`, mapped into BAR1 via GDRCopy (`gdr_pin_buffer` + `gdr_map`), registered with SPDK (`spdk_mem_register`)
- **NVMe DMA**: Controller writes directly into GPU BAR1 memory via PCIe posted write (single hop, no host CPU involvement)
- **D2D copy**: `cudaMemcpyAsync(DeviceToDevice)` from staging slot to final destination at GPU internal bandwidth (~600 GB/s for 128KiB — effectively zero-cost)
- **Ring reuse**: Slots recycled via lazy stream synchronization; no per-call allocation

### DRAM Path (NVMe → host DRAM → H2D)

```
NVMe SSD ──DMA──→ host DRAM (pinned ring) ──memcpy──→ memory-tier slot ──cudaMemcpyAsync H2D──→ final GPU buffer
```

Traditional host-bounce pipeline. Memory-tier populated for warm-path caching.

## Build

```bash
# Build with P2P-native dispatcher
cargo build -p certus-server --release --features p2p-native

# Build standalone component (for development)
cargo build -p dispatcher-p2p
```

## Runtime Path Selection

Set `CERTUS_FORCE_PATH` environment variable:

| Value | Behavior |
|-------|----------|
| `p2p` | Always use P2P path. Fails if GDRCopy ring can't initialize. |
| `dram` | Always use DRAM host-bounce path. P2P ring is NOT allocated. |
| `auto` | (Default) Attempt P2P ring allocation at startup. Use P2P if successful, otherwise fall back to DRAM. |

```bash
# Run with P2P
CERTUS_FORCE_PATH=p2p ./target/release/certus-server --device-pci 0000:62:00.0 --format

# Run with DRAM (for comparison / workloads where warm-path caching matters)
CERTUS_FORCE_PATH=dram ./target/release/certus-server --device-pci 0000:62:00.0 --format

# Auto (default)
./target/release/certus-server --device-pci 0000:62:00.0 --format
```

## Benchmark

```bash
# Start server (pick a path)
CERTUS_FORCE_PATH=p2p ./target/release/certus-server --device-pci 0000:62:00.0 --format &

# Run benchmark
python3 apps/python/certus-api-bench.py \
  --server localhost:50051 \
  --clients 1 \
  --num-objects 16 \
  --iterations 10 \
  --block-size 4194304

# Kill server
pkill certus-server
```

## Performance (single Intel P5800X NVMe, NVIDIA A30 GPU, PCIe Gen4)

### At QD32 (2 queues × QD32 = 64 in-flight) — current default

| Path | Cold Throughput | Cold p99 | Cold Avg | Notes |
|------|----------------|----------|----------|-------|
| **P2P** | 3.93 GB/s | 1.22 ms | 1.07 ms | NVMe → GPU BAR1 → D2D |
| **DRAM** | 3.89 GB/s | 1.23 ms | 1.08 ms | NVMe → DRAM → H2D |

At deep pipelining, both paths are **nearly identical** — the H2D copy is fully overlapped with NVMe reads.

### At QD8 (baseline, 2 queues × QD8 = 16 in-flight)

| Path | Cold Throughput | Cold p99 | Notes |
|------|----------------|----------|-------|
| **P2P** | 3.90 GB/s | 1.22 ms | P2P is 63% faster |
| DRAM | 2.39 GB/s | 1.89 ms | H2D stalls pipeline at shallow QD |

P2P's advantage is largest at **shallow queue depths** where the H2D copy stalls the NVMe pipeline.

### Root-complex bottleneck

Both paths top out at ~4 GB/s on this Intel platform. The NVMe drive can push 5.9 GB/s to host DRAM with 4 queues × QD32 (128 in-flight), but root-complex P2P forwarding caps at ~4 GB/s regardless of queue depth.

**On PCIe-switch topologies** (DGX, HGX, or custom NVMe↔GPU switch), P2P would reach full drive bandwidth since traffic doesn't traverse the root complex.

## When to Use Each Path

- **P2P** (`p2p`): Best for cold-lookup-heavy workloads (large model loading, KV cache misses). Eliminates host DRAM from the data path. Best latency. Advantage grows with multi-drive and systems with PCIe switches.
- **DRAM** (`dram`): Best for mixed workloads with high cache hit rates. The memory-tier population enables 15+ GB/s warm-path lookups. Can be tuned to higher throughput via queue depth. Use when GDRCopy/nvidia-peermem are unavailable.
- **Auto** (`auto`): Production default. Gets P2P benefits when hardware supports it, gracefully degrades otherwise.

## Prerequisites

### SPDK + IOMMU + Hugepages (required for both paths)

See the [main README](../../README.md) for full setup. Summary:

```bash
# 1. Install SPDK dependencies
deps/install_deps.sh
pip install -r deps/requirements.txt
deps/build_spdk.sh

# 2. Kernel boot parameters (reboot required)
# Intel:
grubby --update-kernel=ALL --args="intel_iommu=on iommu=pt default_hugepagesz=2M hugepagesz=2M hugepages=2048"
# AMD:
grubby --update-kernel=ALL --args="amd_iommu=on iommu=pt default_hugepagesz=2M hugepagesz=2M hugepages=2048"

# 3. Bind NVMe devices to vfio-pci
sudo deps/spdk/scripts/setup.sh

# 4. Set memlock unlimited
echo '* hard memlock unlimited' | sudo tee -a /etc/security/limits.conf
echo '* soft memlock unlimited' | sudo tee -a /etc/security/limits.conf
```

### GDRCopy (required for P2P path only)

GDRCopy provides user-space GPU BAR1 memory mapping. The P2P ring uses it to create SPDK-registered DMA buffers backed by GPU memory.

```bash
# Install GDRCopy (RHEL/Fedora)
sudo dnf install gdrcopy gdrcopy-devel

# Or build from source:
git clone https://github.com/NVIDIA/gdrcopy.git
cd gdrcopy
make PREFIX=/usr/local CUDA=/usr/local/cuda all install

# Load the kernel module
sudo modprobe gdrdrv

# Verify
gdrcopy_sanity

# Persist across reboots
echo 'gdrdrv' | sudo tee /etc/modules-load.d/gdrdrv.conf
```

### nvidia-peermem (required for P2P path only)

Enables PCIe peer-to-peer DMA between NVMe controllers and GPU memory via the IOMMU.

```bash
# Usually included with NVIDIA driver >= 510
sudo modprobe nvidia-peermem

# Verify
lsmod | grep nvidia_peermem

# Persist across reboots
echo 'nvidia-peermem' | sudo tee /etc/modules-load.d/nvidia-peermem.conf
```

### Verify P2P Readiness

```bash
# All three modules must be loaded:
lsmod | grep -E "gdrdrv|nvidia_peermem|vfio_pci"

# GPU BAR1 should be accessible:
nvidia-smi -q | grep "BAR1"

# NVMe should be bound to vfio-pci:
ls /sys/bus/pci/drivers/vfio-pci/ | grep 0000:
```
