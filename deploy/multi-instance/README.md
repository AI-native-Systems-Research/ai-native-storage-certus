# Multi-Instance Certus Launcher

Launch **N independent `certus-server` instances — one per NVMe SSD** — each
serving its own shared-memory (shmq) mailbox and with its NVMe poller pinned to
a dedicated physical core in the **same NUMA zone as the drive**. A companion
script fires one `certus-api-bench.py` client at each server in parallel and
aggregates the throughput into a system-wide total.

The sole control transport is a `/dev/shm` mailbox: there is no TCP port. Each
instance `i` gets a distinct mailbox path `${BASE_SHM_PATH}-i`
(e.g. `/dev/shm/certus-shmq-0`, `-1`, ...), so instances never contend for a
shared resource.

This is the "scale-out on a single box" topology: rather than one server
driving many drives, each drive gets a dedicated server + poller core, and
(when `numactl` is present) its memory-tier DMA pool is kept node-local.

## Layout

| Script | Purpose |
|--------|---------|
| `config.sh` | Shared config + topology helpers (sourced, not run directly). |
| `launch-servers.sh` | Discover SSDs, plan NUMA-aware core assignment + per-instance mailbox path, launch one server per SSD in its own **tmux window**. |
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

## Compile and run (step by step)

A complete walkthrough from a fresh checkout to aggregated results. All paths
are relative to the repository root unless noted.

### 1. Build SPDK (one-time, required by `certus-server`)

`certus-server` links the userspace SPDK NVMe driver, so SPDK must be built and
installed under `deps/spdk-build/` first:

```bash
deps/install_deps.sh                 # system packages (sudo; RHEL/Fedora)
pip install -r deps/requirements.txt
deps/build_spdk.sh                   # clone + build + install to deps/spdk-build/
```

You also need IOMMU + hugepages enabled and `memlock` unlimited (see the top
level `README.md`). Verify hugepages are present:

```bash
grep Huge /proc/meminfo                # HugePages_Total should be > 0
```

### 2. Compile the server (release)

```bash
cargo build --release -p certus-server
# produces target/release/certus-server
```

The launcher expects this binary; override its location with
`CERTUS_SERVER_BIN` if you build elsewhere.

### 3. Bind the NVMe SSDs to vfio-pci

```bash
scripts/spdk-scripts/bind_vfio.sh
scripts/spdk-scripts/show_spdk_devices.sh        # list bound drives + NUMA node
```

`show_spdk_devices.sh` is also what the launcher's discovery mirrors — every
NVMe controller bound to `vfio-pci` becomes one server instance.

### 4. Install the benchmark client deps

```bash
pip install -r apps/python/requirements.txt      # shmq connector: stdlib-only, no deps
pip install torch                                # match your CUDA version
```

The shmq wire client is the pure-Python `Ring` in
`certus-shmq-connector/certus_shmq_connector/ring.py` (stdlib `struct`/`mmap`/
`ctypes` only — no `grpcio`/`protobuf`), located at runtime by
`apps/python/certus_shmq_helpers.py`. The client uses CUDA (`torch.cuda` +
cudaIpc handles), so a working NVIDIA GPU and driver are still required; install
`torch` for your CUDA version as shown above.

### 5. Launch the servers

```bash
cd deploy/multi-instance

# All discovered SSDs, formatting on-disk state fresh:
./launch-servers.sh --format

# ...or just the first 2 drives, recovering existing data:
./launch-servers.sh -n 2
```

Expected output (2-drive example):

```
[multi] Planning 2 instance(s):
  IDX BDF            NUMA  SHM_PATH                    POLLER_CPU
  0   0000:61:00.0   0     /dev/shm/certus-shmq-0      1
  1   0000:62:00.0   0     /dev/shm/certus-shmq-1      2
[multi] Launching servers in tmux session 'certus' (logs in /tmp/certus-multi-instance)
[multi] Waiting for servers to come up...
[multi]   srv0 (/dev/shm/certus-shmq-0, 0000:61:00.0) ready
[multi]   srv1 (/dev/shm/certus-shmq-1, 0000:62:00.0) ready
[multi] All 2 server(s) ready.
```

Watch a server live with `tmux attach -t certus` (one window per instance), or
tail a log: `tail -f /tmp/certus-multi-instance/srv-0.log`.

### 6. Run the benchmark and read the results

Everything after `--` is forwarded to `certus-api-bench.py`:

```bash
./run-benchmarks.sh -- --clients 4 --num-objects 32 --iterations 10 --block-size 4M
```

One client runs per server (GPU round-robin), and the per-phase aggregate
throughput is summed across instances:

```
IDX   MAILBOX                     POPULATE      HOT          COLD
0     /dev/shm/certus-shmq-0      0.33 GB/s    10.42 GB/s    2.14 GB/s
1     /dev/shm/certus-shmq-1      0.30 GB/s     3.14 GB/s    1.87 GB/s
---   --------------------------  -----------  -----------   -----------
SUM   2 ok                        0.63 GB/s    13.56 GB/s    4.01 GB/s
```

`POPULATE` is write throughput, `HOT` is memory-tier (DRAM) read throughput,
`COLD` is SSD-tier read throughput. The `ALL` row is the system-wide total. Full
per-client output is kept in `/tmp/certus-multi-instance/bench-<i>.log`.

> If clients exit non-zero with `FlushToSsd failed`, add `--skip-flush` to the
> forwarded args — see [Troubleshooting](#troubleshooting).

### 7. Tear down

```bash
./stop-servers.sh                # graceful SIGTERM, then kill the tmux session
./stop-servers.sh --purge-logs   # also delete /tmp/certus-multi-instance
```

## How instances are assigned

For instance `i` (SSD `i` in discovery order, sorted by NUMA node then BDF):

- **shmq mailbox** = `${BASE_SHM_PATH}-i` (default base `/dev/shm/certus-shmq`,
  so `/dev/shm/certus-shmq-0`, `-1`, ...). Passed to the server as
  `--shm-path <path> --channels <N>` (default 32 channels). Any stale mailbox at
  that path is removed before the instance launches.
- **NUMA node** = the SSD's `numa_node` from sysfs.
- **`--poller-base-cpu`** = the next free physical core on that NUMA node
  (a per-node cursor starts after `POLLER_RESERVE_CORES` leading cores, so the
  OS / server threads keep core 0 etc.). With one drive per server, the poller
  occupies exactly that one core.
- The server is wrapped in `numactl --cpunodebind=<node> --membind=<node>`
  (unless `--no-numactl`) so its threads and the 2 GiB memory-tier pool stay
  local to the drive's node.

Example on a 2-socket box (NUMA0 = drives `61/62/63/64`, NUMA1 = `c1/c2/c3`):

```
IDX BDF            NUMA  SHM_PATH                    POLLER_CPU
0   0000:61:00.0   0     /dev/shm/certus-shmq-0      1
1   0000:62:00.0   0     /dev/shm/certus-shmq-1      2
2   0000:63:00.0   0     /dev/shm/certus-shmq-2      3
3   0000:64:00.0   0     /dev/shm/certus-shmq-3      4
4   0000:c1:00.0   1     /dev/shm/certus-shmq-4      17
5   0000:c2:00.0   1     /dev/shm/certus-shmq-5      18
6   0000:c3:00.0   1     /dev/shm/certus-shmq-6      19
```

The plan is written to `/tmp/certus-multi-instance/instances.tsv`; per-server
logs are `srv-<i>.log` and per-client logs are `bench-<i>-<r>.log` (instance `i`,
client replica `r`) in the same dir.

## `launch-servers.sh` options

```
-n NUM         Number of instances/SSDs (default: all discovered)
-p PATH        Base shmq mailbox path; instance i serves PATH-i
               (default /dev/shm/certus-shmq)
-s SESSION     tmux session name (default "certus")
--format       Pass --format to each server (DESTROYS existing data)
--mem SIZE     Per-instance memory-tier size (e.g. 2G, 512M)
--no-numactl   Do not NUMA-bind the servers
BDF ...        Explicit NVMe PCI addresses (overrides auto-discovery)
```

## `run-benchmarks.sh` options

```
-s SESSION                      tmux session name
-c N, --clients-per-server N    Launch N client *processes* per server (default 1)
--gpu-spread                    Round-robin clients across ALL GPUs (ignore NUMA)
--gpu N                         Pin ALL clients to GPU N
--no-gpu-affinity               Do not set CUDA_VISIBLE_DEVICES (clients use GPU 0)
-- BENCH_ARGS...                Everything after -- is forwarded to certus-api-bench.py
```

**Clients per server.** `-c N` runs `N` separate benchmark *processes* against
each server instance, so total clients launched = `instances x N`. Each
instance's throughput is summed across its `N` processes (the `NCLI` column),
and the `ALL` row sums every instance. This is distinct from the bench script's
own `--clients`, which sets the number of *threads within* a single process —
they compose, e.g. `-c 2 -- --clients 4` = 8 concurrent threads per server.

**GPU selection** (via `CUDA_VISIBLE_DEVICES`):

| Mode | Behaviour |
|------|-----------|
| *(default)* | **NUMA-local**: each client uses a GPU in the same NUMA zone as its server instance, round-robin among that node's GPUs. Falls back to global round-robin if a node has no local GPU (and collapses to "use the only GPU" on single-GPU hosts). |
| `--gpu-spread` | Round-robin across **all** GPUs, ignoring NUMA (`launch_index % gpu_count`). |
| `--gpu N` | Pin **every** client to GPU `N`. |
| `--no-gpu-affinity` | Set nothing; every client falls back to GPU 0. |

GPU→NUMA mapping is read from sysfs (`/sys/bus/pci/devices/<gpu_bdf>/numa_node`)
via `nvidia-smi`. On the reference box GPU 0 is on NUMA 0 and GPU 1 on NUMA 1,
so node-0 SSDs (`61–64`) drive GPU 0 and node-1 SSDs (`c1–c4`) drive GPU 1 —
keeping the shmq client, server, poller, and GPU all on one socket.

Useful forwarded bench args: `--clients`, `--num-objects`, `--iterations`,
`--block-size`, `--batch-size`, `--skip-flush`.

Example — 2 client processes per server, all on GPU 0:

```bash
./run-benchmarks.sh -c 2 --gpu 0 -- --clients 4 --num-objects 32 --block-size 4M
```

```
IDX   MAILBOX                     NCLI   POPULATE      HOT          COLD
0     /dev/shm/certus-shmq-0      2      0.66 GB/s    4.81 GB/s    1.80 GB/s
1     /dev/shm/certus-shmq-1      2      0.61 GB/s    3.34 GB/s    1.84 GB/s
---   --------------------------  ----   ----------   ----------   ----------
SUM   2 ok                        4      1.27 GB/s    8.15 GB/s    3.64 GB/s
```

## Troubleshooting

- **`EAL: Failed to open VFIO group N` / `PCI_BUS: Requested device ... cannot
  be used`** in a server log is **benign**. Each SPDK process enumerates *all*
  vfio-pci devices at startup and cannot open the ones already claimed by its
  sibling instances. Every server still successfully opens its own assigned
  drive (look for `data drive 0 initialized at <bdf>, poller pinned to CPU N`
  followed by the `mailbox <path> channels=...` line — the server publishes the
  mailbox file *last*, once the poller and worker pool are up).

- **One instance reports `FAILED` (mailbox missing) for every phase**: its
  server never published its mailbox file, so `${BASE_SHM_PATH}-i` does not
  exist in `/dev/shm` and the benchmark client had nothing to open. This means
  the server failed to initialize (out of hugepages, could not open its drive,
  panicked) — check `srv-<i>.log`. Each instance uses a **distinct** mailbox
  path, so there is no cross-instance collision to work around; `launch-servers.sh`
  removes any stale mailbox before launch and treats the mailbox file appearing
  on disk as the readiness signal.

- **Clients exit non-zero with `FlushToSsd failed`**: the SPDK-NVMe backend may
  not support the benchmark's explicit DRAM-cache flush. Throughput numbers are
  still valid (the cold path reads real SSD data); `run-benchmarks.sh` reports
  the non-zero exit but still aggregates. Pass `-- ... --skip-flush` to skip the
  dedicated flush phase.

## Environment overrides

All defaults in `config.sh` can be overridden via environment variables:
`CERTUS_SERVER_BIN`, `CERTUS_BENCH_SCRIPT`, `CERTUS_PYTHON`, `CERTUS_SESSION`,
`CERTUS_BASE_SHM_PATH`, `CERTUS_CHANNELS`, `CERTUS_RUNDIR`,
`CERTUS_POLLER_RESERVE`, `CERTUS_MEMORY_TIER_SIZE`, `CERTUS_USE_NUMACTL`,
`CERTUS_READY_TIMEOUT`.
