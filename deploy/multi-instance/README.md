# Multi-Instance Certus Launcher

Launch **N independent `certus-server` instances — one per NVMe SSD** — each
bound to its own gRPC port and with its NVMe poller pinned to a dedicated
physical core in the **same NUMA zone as the drive**. A companion script fires
one `certus-api-bench.py` client at each server in parallel and aggregates the
throughput into a system-wide total.

This is the "scale-out on a single box" topology: rather than one server
driving many drives, each drive gets a dedicated server + poller core, and
(when `numactl` is present) its memory-tier DMA pool is kept node-local.

## Layout

| Script | Purpose |
|--------|---------|
| `config.sh` | Shared config + topology helpers (sourced, not run directly). |
| `launch-servers.sh` | Discover SSDs, plan NUMA-aware core/port assignment, launch one server per SSD in its own **tmux window**. |
| `run-benchmarks.sh` | Run one benchmark client per server in parallel; aggregate results. |
| `stop-servers.sh` | SIGTERM the servers, then kill the tmux session. |

## Prerequisites

1. **Build the server** (release):
   ```bash
   cargo build --release -p certus-server
   ```
2. **Bind NVMe drives to vfio-pci** (and hugepages/IOMMU per the repo README):
   ```bash
   scripts/spdk-scripts/bind_vfio.sh
   scripts/spdk-scripts/show_spdk_devices.sh      # verify
   ```
3. `tmux`, `numactl`, and `python3.12` with the bench deps
   (`pip install -r apps/python/requirements.txt`, plus `torch` + a CUDA GPU).

## Quick start

```bash
cd deploy/multi-instance

# Launch one server per discovered SSD (formatting fresh on-disk state):
./launch-servers.sh --format

# Run 4 client threads per instance and aggregate throughput:
./run-benchmarks.sh -- --clients 4 --num-objects 32 --iterations 10 --block-size 4M

# Inspect live servers (one tmux window each):
tmux attach -t certus            # Ctrl-b n / Ctrl-b p to cycle windows

# Tear down:
./stop-servers.sh
```

## How instances are assigned

For instance `i` (SSD `i` in discovery order, sorted by NUMA node then BDF):

- **gRPC port** = `BASE_PORT + i` (default base `50051`).
- **NUMA node** = the SSD's `numa_node` from sysfs.
- **`--poller-base-cpu`** = the next free physical core on that NUMA node
  (a per-node cursor starts after `POLLER_RESERVE_CORES` leading cores, so the
  OS / gRPC threads keep core 0 etc.). With one drive per server, the poller
  occupies exactly that one core.
- The server is wrapped in `numactl --cpunodebind=<node> --membind=<node>`
  (unless `--no-numactl`) so its threads and the 2 GiB memory-tier pool stay
  local to the drive's node.

Example on a 2-socket box (NUMA0 = drives `61/62/63/64`, NUMA1 = `c1/c2/c3`):

```
IDX BDF            NUMA  PORT   POLLER_CPU
0   0000:61:00.0   0     50051  1
1   0000:62:00.0   0     50052  2
2   0000:63:00.0   0     50053  3
3   0000:64:00.0   0     50054  4
4   0000:c1:00.0   1     50055  17
5   0000:c2:00.0   1     50056  18
6   0000:c3:00.0   1     50057  19
```

The plan is written to `/tmp/certus-multi-instance/instances.tsv`; per-server
logs are `srv-<i>.log` and per-client logs are `bench-<i>.log` in the same dir.

## `launch-servers.sh` options

```
-n NUM         Number of instances/SSDs (default: all discovered)
-p BASE_PORT   First gRPC port (default 50051)
-s SESSION     tmux session name (default "certus")
--format       Pass --format to each server (DESTROYS existing data)
--mem SIZE     Per-instance memory-tier size (e.g. 2G, 512M)
--no-numactl   Do not NUMA-bind the servers
BDF ...        Explicit NVMe PCI addresses (overrides auto-discovery)
```

## `run-benchmarks.sh` options

```
-s SESSION          tmux session name (only used to locate the run dir indirectly)
--no-gpu-affinity   Do not pin clients to GPUs round-robin
-- BENCH_ARGS...    Everything after -- is forwarded to certus-api-bench.py
```

By default client `i` is pinned to GPU `i % <gpu count>` via
`CUDA_VISIBLE_DEVICES`. Useful bench args: `--clients`, `--num-objects`,
`--iterations`, `--block-size`, `--batch-size`, `--skip-flush`.

## Troubleshooting

- **`EAL: Failed to open VFIO group N` / `PCI_BUS: Requested device ... cannot
  be used`** in a server log is **benign**. Each SPDK process enumerates *all*
  vfio-pci devices at startup and cannot open the ones already claimed by its
  sibling instances. Every server still successfully opens its own assigned
  drive (look for `data drive 0 initialized at <bdf>, poller pinned to CPU N`
  followed by `listening on`).

- **Clients exit non-zero with `FlushToSsd failed`**: the SPDK-NVMe backend may
  not support the benchmark's explicit DRAM-cache flush. Throughput numbers are
  still valid (the cold path reads real SSD data); `run-benchmarks.sh` reports
  the non-zero exit but still aggregates. Pass `-- ... --skip-flush` to skip the
  dedicated flush phase.

## Environment overrides

All defaults in `config.sh` can be overridden via environment variables:
`CERTUS_SERVER_BIN`, `CERTUS_BENCH_SCRIPT`, `CERTUS_PYTHON`, `CERTUS_SESSION`,
`CERTUS_BASE_PORT`, `CERTUS_RUNDIR`, `CERTUS_POLLER_RESERVE`,
`CERTUS_MEMORY_TIER_SIZE`, `CERTUS_USE_NUMACTL`, `CERTUS_READY_TIMEOUT`.
