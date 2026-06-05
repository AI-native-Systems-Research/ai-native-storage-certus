# certus-server-fs

Filesystem-backed drop-in replacement for `certus-server`, designed for side-by-side performance benchmarking. Implements the identical gRPC Dispatcher API using local filesystem I/O instead of SPDK NVMe, enabling measurement of the performance advantage provided by the Certus stack.

## Architecture

```
Python Client → gRPC (protobuf) → certus-server-fs
                                        ↓
                                  CUDA IPC open
                                  cudaMemcpy D2H / H2D
                                        ↓
                              ┌─────────────────────┐
                              │   Memory Tier (LRU)  │  ← hot path
                              └─────────────────────┘
                                        ↓ miss
                              ┌─────────────────────┐
                              │  Filesystem Storage  │  ← cold path
                              │  O_DIRECT + O_SYNC   │
                              └─────────────────────┘
```

### Key differences from certus-server

| Layer | certus-server | certus-server-fs |
|-------|---------------|------------------|
| Storage I/O | SPDK userspace NVMe (polled) | Kernel `O_DIRECT` + `O_SYNC` file I/O |
| Memory tier | Custom DMA-capable pool | HashMap-based LRU with `Vec<u8>` entries |
| GPU transfer | Zero-copy DMA via SPDK bounce buffer | `cudaMemcpy` through CUDA-pinned staging buffer |
| Data layout | Extent-managed on raw NVMe namespace | One file per key in a directory |
| Dependencies | SPDK, extent-manager, dispatcher, etc. | Only `gpu-services` (for CUDA FFI) |

### Data flow

**Populate (write):** Open CUDA IPC handle → `cudaMemcpy(D2H)` to pinned staging buffer → `pwrite` with `O_DIRECT|O_SYNC` to file → insert into LRU memory tier.

**Lookup hot path:** Find key in LRU → copy to staging buffer → `cudaMemcpy(H2D)` to GPU.

**Lookup cold path:** `pread` with `O_DIRECT` from file → promote to LRU → `cudaMemcpy(H2D)` to GPU.

## Build

```bash
cargo build -p certus-server-fs --release
```

No SPDK or special hardware setup required — only a CUDA-capable GPU (for IPC handle operations with the benchmark client).

## Run

```bash
# Basic usage (creates data dir, formats on startup)
./target/release/certus-server-fs --format

# With explicit options
./target/release/certus-server-fs \
    --data-dir /tmp/certus-fs-data \
    --memory-tier-size 2G \
    --staging-size 16M \
    --format \
    --listen 0.0.0.0:50051
```

### CLI options

| Flag | Default | Description |
|------|---------|-------------|
| `--data-dir` | `/tmp/certus-fs-data` | Directory for per-key data files |
| `--listen` | `0.0.0.0:50051` | gRPC listen address |
| `--memory-tier-size` | `2G` | LRU cache capacity (e.g. `256M`, `1G`) |
| `--staging-size` | `16M` | CUDA-pinned host buffer for GPU↔host transfers |
| `--format` | off | Clear data directory on startup |
| `--tls-cert` / `--tls-key` | — | Enable TLS |

## Benchmarking

Run the same `certus-api-bench.py` against either server without modification:

```bash
# Terminal 1: start filesystem server
./target/release/certus-server-fs --format --memory-tier-size 2G

# Terminal 2: run benchmark
cd apps/python
python certus-api-bench.py --server localhost:50051 \
    --clients 1 --num-objects 16 --iterations 10 --batch-size 10

# For comparison, stop the fs server and start the real one:
./target/release/certus-server --drive-count 1 --format --memory-tier-size 2G

# Re-run the same benchmark command
python certus-api-bench.py --server localhost:50051 \
    --clients 1 --num-objects 16 --iterations 10 --batch-size 10
```

### Example results (1 client, 1 SSD, 4 MiB blocks)

| Metric | certus-server (SPDK) | certus-server-fs | Speedup |
|--------|---------------------|------------------|---------|
| Populate | 487 µs / 8.6 GB/s | 20,090 µs / 0.21 GB/s | 41x |
| Lookup (hot) | 466 µs / 9.0 GB/s | 1,136 µs / 3.7 GB/s | 2.4x |
| Lookup (cold) | 2,238 µs / 1.9 GB/s | 9,878 µs / 0.42 GB/s | 4.4x |

## Notes

- The filesystem server uses `O_DIRECT` and `O_SYNC` to bypass the kernel page cache and ensure data is persisted to the drive, providing a fair comparison with SPDK's direct NVMe access.
- The memory tier uses the same default capacity (2 GiB) as certus-server so hot-path comparisons reflect the transfer mechanism difference, not cache size.
- The staging buffer is allocated with `cudaHostAlloc` (pinned memory) for optimal `cudaMemcpy` throughput.
