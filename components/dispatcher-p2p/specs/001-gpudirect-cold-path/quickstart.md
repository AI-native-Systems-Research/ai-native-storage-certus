# Quickstart: Validating GPUDirect Storage Cold Path

## Prerequisites

- Linux host with NVIDIA GPU (compute capability 7.0+)
- Kernel modules loaded: `gdrdrv` (GDRCopy), `nvidia-peermem`
- NVMe drives bound to vfio-pci for SPDK
- SPDK built at `deps/spdk-build/`
- Hugepages and IOMMU configured (see main README)
- Python 3 with gRPC dependencies for certus-api-bench_v2.py

## Build

```bash
cargo build -p dispatcher-p2p
cargo test -p dispatcher-p2p
cargo clippy -p dispatcher-p2p -- -D warnings
cargo doc -p dispatcher-p2p --no-deps
```

## Validation Scenario 1: P2P Cold Path Correctness

**Goal**: Verify cold lookups return correct data via the P2P path.

```bash
# Start the server with the P2P profile
cd apps/certus-server-yaml
cargo run --release -- --profile profiles/full-p2p.yaml

# In another terminal: populate entries, evict to SSD, then read back
python apps/python/certus-api-bench_v2.py \
  --mode cold-only \
  --entries 100 \
  --clients 1 \
  --verify-data
```

**Expected outcome**: All lookups succeed with data integrity verification passing. Server logs show "P2P ring initialized" at startup.

## Validation Scenario 2: DRAM Fallback

**Goal**: Verify graceful fallback when P2P is unavailable.

```bash
# Unload GDRCopy module (or run on a system without it)
sudo rmmod gdrdrv

# Start the server — should fall back to DRAM path
cargo run --release -- --profile profiles/full-p2p.yaml

# Run same benchmark
python apps/python/certus-api-bench_v2.py \
  --mode cold-only \
  --entries 100 \
  --clients 1 \
  --verify-data
```

**Expected outcome**: Server logs show "P2P ring initialization failed: ... falling back to DRAM path". All lookups succeed correctly.

## Validation Scenario 3: Multi-Client Concurrency

**Goal**: Verify 4+ concurrent clients get correct data without corruption.

```bash
python apps/python/certus-api-bench_v2.py \
  --mode cold-only \
  --entries 1000 \
  --clients 4 \
  --verify-data
```

**Expected outcome**: All 4 clients receive correct data. No corruption, no deadlock.

## Validation Scenario 4: Performance Measurement

**Goal**: Measure P2P vs DRAM throughput for comparison.

```bash
# With P2P (gdrdrv loaded):
python apps/python/certus-api-bench_v2.py \
  --mode mixed \
  --entries 10000 \
  --clients 4 \
  --report throughput

# Without P2P (gdrdrv unloaded or on non-P2P hardware):
# Same command — reports DRAM-path throughput for comparison
```

**Expected outcome**: Both runs produce throughput numbers. Comparison is the responsibility of the operator.

## Validation Scenario 5: Resource Cleanup

**Goal**: Verify no GPU memory leaks on shutdown.

```bash
# Run server, execute lookups, then send shutdown signal
# Monitor nvidia-smi before and after for GPU memory usage
nvidia-smi --query-gpu=memory.used --format=csv,noheader
# (start server, run benchmark, stop server)
nvidia-smi --query-gpu=memory.used --format=csv,noheader
```

**Expected outcome**: GPU memory usage returns to pre-server baseline after shutdown.

## Criterion Benchmarks

```bash
cargo bench -p dispatcher-p2p --bench cold_path_benchmark
```

Requires hardware. Reports per-commit throughput numbers for pipeline comparison.
