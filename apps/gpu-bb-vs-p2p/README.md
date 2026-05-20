# gpu-bb-vs-p2p

Benchmark comparing two NVMe-to-GPU data transfer strategies:

1. **Bounce-buffer (BB)**: NVMe reads into CUDA-pinned host memory, then async DMA copy (H2D) to GPU.
2. **GPUDirect P2P**: NVMe reads directly into GPU BAR1-mapped memory via GDRCopy, bypassing host RAM entirely.

Both paths use a ring-buffer pipeline with dual CUDA streams to maximize overlap between NVMe I/O and GPU DMA.

## How It Works

The benchmark reads a configurable stream of data from an NVMe device in fixed-size chunks. Rather than issuing all reads then copying sequentially, it uses a pipelined ring buffer:

- A pool of staging buffers (configurable depth, default 32) is pre-allocated.
- NVMe async reads fill ring slots while completed slots are simultaneously DMA-copied to the GPU destination.
- Two alternating CUDA streams allow the current copy to overlap with the previous slot's synchronization, eliminating pipeline stalls.

### Bounce-Buffer Path

Ring buffers are allocated with `cudaHostAlloc` (page-locked) and registered with SPDK via `spdk_mem_register`, making them valid targets for both NVMe DMA and CUDA async H2D copies.

### P2P Path

Ring buffers are GPU device allocations mapped through GDRCopy (BAR1 mapping). The NVMe controller writes directly to GPU memory with no host-side copy. A final D2D copy moves data from the staging ring to the output buffer.

## Prerequisites

- Linux with NVIDIA GPU and CUDA toolkit installed
- NVMe device bound to VFIO (userspace driver via SPDK)
- SPDK built and installed (`deps/build_spdk.sh`)
- GDRCopy installed (`libgdrapi.so`, `gdrdrv` kernel module loaded)
- `nvidia-peermem` kernel module loaded (for P2P path)
- Hugepages allocated (e.g., `echo 2048 > /sys/kernel/mm/hugepages/hugepages-2048kB/nr_hugepages`)
- `memlock` ulimit set to unlimited

## Build

```bash
cargo build -p gpu-bb-vs-p2p --release
```

## Run

```bash
LD_LIBRARY_PATH=/usr/local/lib ./target/release/gpu-bb-vs-p2p [OPTIONS]
```

If no `--pci` address is given, the first NVMe device discovered by SPDK is used.

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--pci` | auto-detect | NVMe PCI address (DDDD:BB:DD.F format) |
| `--chunk-size` | 131072 (128 KiB) | Transfer chunk size in bytes (must not exceed NVMe MDTS) |
| `--stream-size` | 5242880 (5 MiB) | Total stream size per iteration |
| `--ring-size` | 32 | Pipeline depth (number of staging buffers) |
| `--warmup` | 3 | Warmup iterations before measurement |
| `--iterations` | 10 | Number of measured iterations |

### Example

```bash
# 100 MiB stream with 128 KiB chunks, 32-deep pipeline
LD_LIBRARY_PATH=/usr/local/lib ./target/release/gpu-bb-vs-p2p \
    --stream-size 104857600 \
    --chunk-size 131072 \
    --ring-size 32
```

## Output

```
Results (NVMe → GPU, 102400 KiB stream, 128 KiB chunks, ring=32):
  bounce-buf   | mean   30987.9 us | min   30484.0 us | p50   31049.4 us | p99   31131.8 us | max   31131.8 us | 3227.1 MB/s
  p2p-direct   | mean   29614.1 us | min   29573.9 us | p50   29616.3 us | p99   29659.9 us | max   29659.9 us | 3376.8 MB/s

  P2P is 1.05x faster than bounce-buffer
```

Reports per-path statistics (mean, min, p50, p99, max latency and throughput in MB/s) plus the relative speedup of P2P over bounce-buffer.
