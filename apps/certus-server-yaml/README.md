# certus-server-yaml

YAML-composed Certus shmq server. The component graph (block devices, extent
managers, memory tier, GPU services) is declared in a YAML profile and
assembled at compile time via `build.rs` code generation. Different profiles
select different storage backends without changing Rust source code.

## Profiles

| Profile | Feature flags | Description |
|---------|--------------|-------------|
| `full` (default) | `spdk` | Production: SPDK userspace NVMe + GPU DMA |
| `full-session-lists` | `spdk` | Same as `full`, but uses the session-lineage eviction policy instead of LRU |
| `full-fs-block` | `filesys` | Filesystem-backed block devices with O_DIRECT on `/ssd/`, SPDK for DMA allocation |
| `minimal` | (none) | Logger only, no hardware dependencies |

Profiles live in `profiles/*.yaml` and control which components are
instantiated, how they are wired, and which init hooks run.

## Building

Select a profile with the `CERTUS_PROFILE` environment variable and the
matching Cargo feature(s):

```bash
# Full SPDK NVMe (default)
cargo build -p certus-server-yaml --release

# Filesystem-backed block devices (O_DIRECT, SPDK hugepages for DMA)
CERTUS_PROFILE=full-fs-block cargo build -p certus-server-yaml \
    --no-default-features --features filesys --release

# Session-lineage eviction policy (drop-in for LRU); convenience wrapper:
scripts/build-certus-session-lists.sh
# equivalent to:
CERTUS_PROFILE=full-session-lists cargo build -p certus-server-yaml --release
```

### Prerequisites

- **full**: SPDK built at `deps/spdk-build/`, hugepages configured, NVMe
  devices unbound from kernel driver.
- **full-fs-block**: SPDK environment (for hugepage DMA buffers). Backing
  files created on a filesystem that supports `O_DIRECT` (xfs, ext4 — not
  tmpfs).
- **Both**: NVIDIA GPU with CUDA for GPU DMA transfers.

## Running

```bash
# 4 SPDK NVMe drives, format on startup, 2 GiB DRAM cache
./target/release/certus-server-yaml \
    --drive-count 4 --format --memory-tier-size 2G

# Explicit PCI addresses
./target/release/certus-server-yaml \
    --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format

# Filesystem-backed (same CLI, different binary)
CERTUS_PROFILE=full-fs-block cargo build -p certus-server-yaml \
    --no-default-features --features filesys --release
./target/release/certus-server-yaml \
    --drive-count 4 --format --memory-tier-size 2G
```

## CLI Options

| Flag | Description |
|------|-------------|
| `--drive-count N` | Use first N discovered NVMe drives (or N filesystem drives) |
| `--device-pci ADDR` | Explicit PCI address (repeatable, mutually exclusive with `--drive-count`) |
| `--memory-tier-size SIZE` | DRAM cache pool size (e.g. `256M`, `2G`). Default: 2G |
| `--format` | Format extent managers on startup (destroys existing data) |
| `--shm-path PATH` | Shared-memory queue (shmq) mailbox path. Default: `/dev/shm/certus-shmq` |
| `--channels N` | Number of shmq client channels. Default: 32 |
| `--poller-base-cpu N` | Pin NVMe poller threads starting at CPU N |

## Architecture

```
profiles/full.yaml ──┐
                     ├── build.rs ──> composition.rs (generated)
profiles/full-fs-block.yaml ─┘             │
                                           ▼
                                    build_stack(config)
                                           │
                    ┌──────────────────────┼──────────────────────┐
                    ▼                      ▼                      ▼
              spdk_env             block_device_factory      extent_manager_factory
           (DMA hugepages)        (SPDK NVMe or filesys)    (crash-consistent allocator)
                    │                      │                      │
                    └──────────────────────┼──────────────────────┘
                                           ▼
                                      Dispatcher
                                   (IDispatcher shmq)
```

The `kind: factory` mechanism in the YAML allows the dispatcher to create
N block device / extent manager pairs at runtime (one per drive) without
knowing the concrete implementation at compile time within the dispatcher
crate itself.

## Benchmarking

The server and benchmark client share the same host IPC namespace and
`/dev/shm`; the shmq mailbox path is the endpoint (there is no host:port to
configure).

```bash
# Launch the server with a shmq mailbox and 32 channels
./target/release/certus-server-yaml \
    --drive-count 4 --format --memory-tier-size 2G \
    --shm-path /dev/shm/certus-shmq --channels 32

# Point the benchmark at the same mailbox
python3 apps/python/certus-api-bench.py \
    --shm-path /dev/shm/certus-shmq \
    --clients 1 --num-objects 32 --iterations 10 --block-size 4M
```
