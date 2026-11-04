# Benchmark Output Files Reference

This document describes the output files produced by the hardware ceiling profiler
and the certus connector bench sweep. All files land under `bench-results/`.

## Overview

A full `sweep_modes.sh --hw-profile` run produces:

```
bench-results/
├── hw-ceiling/                              # Hardware ceiling profiler output
│   ├── hardware_profile_<tag>_<date>.yaml   # Machine-readable hardware ceilings
│   ├── hardware-ceiling-<tag>_<date>.md     # Human-readable report with tables
│   └── raw-<tag>_<date>/                    # Raw tool output (audit trail)
├── default_<tag>_<timestamp>.yaml           # Connector bench: default mode
├── default_<tag>_<timestamp>.csv            # Same data, flat CSV
├── pipelined_<tag>_<timestamp>.yaml         # Connector bench: pipelined mode
├── pipelined_<tag>_<timestamp>.csv
├── prefix-miss_<tag>_<timestamp>.yaml       # Connector bench: prefix-miss mode
├── contention_<tag>_<timestamp>.yaml        # Connector bench: contention mode
├── scheduler-step_<tag>_<timestamp>.yaml    # Connector bench: scheduler-step mode
├── ...corresponding .csv files...
└── certus-server.log                        # Server stdout/stderr during the sweep
```

---

## 1. Hardware Ceiling Profiler

**Purpose**: Measure the raw bandwidth ceilings of every hardware path in the
Certus data plane — GPU PCIe, host RAM, NVMe — so that connector bench results
can be interpreted as a fraction of the hardware limit.

**Scripts**: `run-profile.sh` → `run-in-container.sh` → `profile-hardware-ceiling.sh`
(runs inside the `certus-hwceiling:cuda` container with GPU passthrough).

### `hardware_profile_<tag>_<date>.yaml`

The primary machine-readable output. Contains every measurement plus derived
ceilings. Sections:

| Section | What it contains |
|---------|-----------------|
| `tag` | Correlation tag (matches the bench sweep `--tag`) |
| `platform` | Hostname, CPU model, NUMA node count, kernel version, OS |
| `nvme.drives[]` | Each NVMe drive: PCI BDF, model, firmware, serial, capacity, block size. When drives are on vfio-pci (no kernel block node), model/driver come from `lspci` fallback. |
| `gpus.gpu<N>` | Per-GPU: model, PCIe gen/width, BDF, BAR1 size, and measured bandwidths (see below) |
| `host_ram` | Host DRAM memcpy bandwidth: same-NUMA and cross-NUMA (GB/s) |
| `fio_nvme_kernel` | Per-drive fio sequential READ peak (GB/s), at the best block size |
| `cpu_features` | CPU vendor, AVX-512/AVX2 support, XXH3 hash throughput tier |
| `topology` | Per-GPU: which NUMA node it sits on, which NVMe drives share that node |
| `ceilings` | Derived end-to-end ceilings (see "How ceilings are derived" below) |

#### GPU bandwidth fields (per GPU)

| Field | Measurement | Tool |
|-------|-------------|------|
| `h2d_peak_numa_local_gbps` | Host→Device peak (NUMA-local), 1 stream | `cuda_bw_test` (custom CUDA) |
| `h2d_4stream_peak_numa_local_gbps` | Host→Device peak, 4 concurrent streams | `cuda_bw_test` |
| `h2d_4m_1stream_numa_local_gbps` | Host→Device at exactly 4 MiB transfer size | `cuda_bw_test` |
| `d2h_peak_numa_local_gbps` | Device→Host peak (NUMA-local) | `cuda_bw_test` |
| `h2d_peak_cross_numa_gbps` | Host→Device from the other NUMA node | `cuda_bw_test` |
| `cross_numa_h2d_penalty_gbps` | Difference: cross-NUMA − local (negative = no penalty) | derived |
| `d2d_1stream_gbps` | Device-to-device copy, 1 stream (HBM bandwidth) | `cuda_d2d_test` (custom CUDA) |
| `d2d_4stream_gbps` | Device-to-device copy, 4 streams | `cuda_d2d_test` |

All GPU benchmarks use `cudaMemcpy` / `cudaMemcpyDeviceToDevice` over a sweep
of transfer sizes (4 KiB to 64 MiB). The peak is the maximum across all sizes.
When the host has multiple NUMA nodes, the benchmark runs once pinned to each
node via `numactl`.

#### fio NVMe fields (per drive)

| Field | Measurement | Tool |
|-------|-------------|------|
| `peak_read_gbps` | Best sequential READ throughput across block sizes (4K, 64K, 128K, 1M) | `fio` (kernel driver, `--direct=1 --ioengine=libaio --iodepth=32`) |

fio runs against the raw block device (`/dev/nvmeXnY`) with O_DIRECT, bypassing
the page cache. Each block size runs for `--fio-runtime` seconds (default 5).
The drive must be on the kernel nvme driver (not vfio-pci) for this step.

#### How ceilings are derived

| Ceiling | Formula | What it represents |
|---------|---------|-------------------|
| `warm_gpu0_peak_gbps` | GPU 0 NUMA-local H2D peak | Memory-tier (DRAM) → GPU: the warm-path ceiling when data is already in the tier |
| `dispatcher_cold_Ndrive_gbps` | `min(NVMe aggregate read, H2D peak)` | NVMe → DRAM → GPU: the cold-path ceiling. At low drive counts this is NVMe-bound; at high counts it becomes H2D-bound. |
| `p2p_cold_Ndrive_gpu0_gbps` | BAR1 direct write bandwidth | NVMe → GPU BAR1 direct (bypassing host RAM). Requires `nvme-bar1-bench` (not yet available). |

### `hardware-ceiling-<tag>_<date>.md`

Human-readable markdown report containing the same data as the YAML, plus:

- Raw `lscpu`, `numactl --hardware`, `nvidia-smi -L` output
- PCIe theoretical bandwidth table
- Full `cuda_bw_test` output tables (bandwidth at every transfer size)
- `inspect-nvme.py` JSON for each drive (firmware, write cache, latency profile)
- Topology routing matrix (which drive ↔ GPU pairs share a NUMA node)
- Summary tables with derived ceilings

Use the YAML for scripted analysis. Use the markdown to read and share results.

### `raw-<tag>_<date>/`

Captured stdout/stderr from every tool invocation. Useful for debugging when a
number looks wrong. Key files:

| File | Content |
|------|---------|
| `platform.txt` | `lscpu`, `numactl`, `nvidia-smi -L`, `lspci -tv` |
| `gpu-static.csv` | `nvidia-smi --query-gpu` output (index, name, BDF, PCIe gen, width) |
| `gpu-numa.csv` | GPU index → NUMA node mapping |
| `cuda_bw_test_numa<N>.txt` | Full H2D/D2H bandwidth table, pinned to NUMA node N |
| `cuda_d2d_test.txt` | Full D2D bandwidth table |
| `memcpy_bench.txt` | Host RAM memcpy benchmark output |
| `fio_<dev>_read_<bs>.json` | fio JSON output for each drive × block size |
| `fio_results.csv` | Consolidated fio results: `device,rw,bs,gbps,iops,lat_us` |
| `inspect-<dev>.json` | `inspect-nvme.py` output (identify, write cache, latency) |
| `nvidia-smi-q.txt` | Full `nvidia-smi -q` (BAR1 sizes, ECC state, clocks) |
| `drive-numa.csv` | NVMe BDF → NUMA node mapping |

---

## 2. Connector Bench Sweep

**Purpose**: Measure the end-to-end throughput and per-phase latency of the Certus
shmq connector — the same code path vLLM uses to offload/load KV cache blocks
to/from NVMe via shared memory.

**Script**: `bench_connector_lifecycle.py`, called by `sweep_modes.sh` once per mode.

**Prerequisite**: `certus-server-yaml` must be running with the target NVMe drives
bound to vfio-pci. `sweep_modes.sh` handles this automatically.

### Per-mode YAML files: `<mode>_<tag>_<timestamp>.yaml`

Each file contains two top-level keys:

#### `params:`

Run configuration — everything needed to reproduce the result.

| Field | Meaning |
|-------|---------|
| `tag` | User-supplied label (e.g. "2drive") |
| `mode` | Benchmark mode (see below) |
| `target_ssd` | PCI BDFs of the NVMe drives certus-server is using |
| `lookup_order` | `lookup-first` (vLLM ≥ 0.21) or `touch-first` (vLLM ≤ 0.20) |
| `shm_path` | Shared-memory mailbox path (e.g. `/dev/shm/certus-shmq`) |
| `gpu` | GPU device index used (default 0) |
| `tp` | Tensor parallelism degree (number of GPU workers) |
| `bs` | Blocks per batch (KV blocks submitted in one round) |
| `num_blocks` | Total unique block IDs in the working set |
| `block_bytes` | Bytes per KV block (default 2 MiB = one vLLM block) |
| `workers` | Worker threads in the connector's thread pool |
| `working_set` | Total blocks in the eviction-pressure working set |
| `timestamp` | ISO 8601 time the run started |

Mode-specific params are added depending on the mode (e.g. `pipeline_depth`,
`hit_ratio`, `requests_per_step`).

#### `results:`

A list of workloads. Each workload has:

| Field | Meaning |
|-------|---------|
| `label` | Human-readable workload name |
| `total_blocks` | Blocks transferred during the measurement window |
| `total_bytes` | `total_blocks × block_bytes` |
| `wall_seconds` | Wall-clock duration of the measurement (≥ `--min-duration`) |
| `throughput_gbs` | `total_bytes / wall_seconds` in GB/s |
| `iops` | `total_blocks / wall_seconds` |
| `phases` | Per-phase latency breakdown (see below) |

#### Phase latency (`phases:`)

Each phase reports `p50_us`, `p99_us`, `mean_us`, and `n` (sample count).
The phases correspond to actual connector API calls in the vLLM offloading path:

| Phase | What it measures |
|-------|-----------------|
| `touch` | `manager.touch()` — register block keys with the server |
| `lookup` | `manager.lookup()` — check which blocks are already stored |
| `prepare_store` | `manager.prepare_store()` — build regions, hash content |
| `submit_store` | `worker.submit_store()` — enqueue store to the shmq ring |
| `get_finished` | `worker.get_finished()` — poll for completion events |
| `complete_store` | `manager.complete_store()` — finalize stored blocks |
| `prepare_load` | `manager.prepare_load()` — build load regions |
| `submit_load` | `worker.submit_load()` — enqueue load to the ring |
| `complete_load` | `manager.complete_load()` — finalize loaded blocks |
| `prepare` | (pipelined) combined prepare step |
| `submit` | (pipelined) combined submit step |
| `pipeline_drain` | (pipelined) wait for in-flight pipeline slots |
| `complete` | (pipelined) combined complete step |
| `touch_lookup` | (scheduler-step) batched touch + lookup per step |
| `take_events` | (contention) event polling under concurrent load |

Not every phase appears in every mode — only the phases that mode exercises.

### Benchmark modes

Each mode exercises a different aspect of the connector:

| Mode | Workloads produced | What it tests |
|------|-------------------|---------------|
| **default** | Store, Warm Load, Cold Load, Mixed Store+Load Under Eviction | The four standard lifecycle paths: write blocks to NVMe, read them back from memory tier (warm), read them back from NVMe (cold), and mixed read/write under memory pressure with eviction. |
| **pipelined** | Pipelined Store, Pipelined Load | Overlapped submit/complete with a configurable pipeline depth (`pipeline_depth`). Measures how much throughput improves when multiple batches are in flight simultaneously. |
| **prefix-miss** | Prefix-Miss Load | Simulates partial cache hits: `hit_ratio` fraction of keys are pre-stored (warm), the rest miss and trigger cold loads. Models real serving where some prompt prefixes are cached. |
| **contention** | Store Thread, Load Thread, Load Alone | Concurrent store + load threads competing for the shmq ring, vs. load running alone. Measures contention overhead. |
| **scheduler-step** | Scheduler Step | Reproduces vLLM's scheduler loop: each step touches/lookups `requests_per_step` requests from `num_sessions` conversations, stores new blocks, loads cold blocks. The most realistic single-GPU serving simulation. |

### Per-mode CSV files: `<mode>_<tag>_<timestamp>.csv`

Same data as the YAML, flattened to one row per workload. Phase latencies become
columns like `touch_p50_us`, `prepare_store_p99_us`, etc. Params are repeated on
every row. Convenient for loading into pandas, R, or a spreadsheet.

---

## 3. `certus-server.log`

Stdout and stderr from the managed `certus-server-yaml` process during the sweep.
Contains SPDK initialization, hugepage allocation, drive binding confirmation, and
any errors. Check this first when a sweep fails or produces unexpected results.

---

## Interpreting results

**Warm load throughput** should approach the H2D PCIe ceiling from the hardware
profile (e.g. ~57 GB/s on H200 NVL). The gap is connector overhead (hashing,
region construction, shmq ring protocol).

**Cold load throughput** is bounded by `dispatcher_cold` from the hardware profile
(e.g. ~13 GB/s per drive). With 2 drives, the ceiling is ~26 GB/s. The gap is
connector + server overhead (SPDK I/O, memory tier management, eviction).

**Store throughput** is typically lower than load because it includes content
hashing (BLAKE2b) and write-path overhead in the server.

**Phase latencies** identify bottlenecks: if `get_finished` p99 is high, the
server is slow to complete I/O; if `prepare_store` is high, hashing or region
construction is the bottleneck.
