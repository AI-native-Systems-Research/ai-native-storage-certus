# Certus KVCache Storage Benchmark

Replays [Mooncake FAST25](https://github.com/kvcache-ai/Mooncake) inference
traces against **disk** (posix `pread`/`pwrite`) or **Certus** (shmq
shared-memory transport) backends, using identical workload patterns and
metrics for apples-to-apples comparison.

## Quick Start

### run.sh — full lifecycle (recommended)

`run.sh` handles the entire lifecycle: build certus-server, start it, run the
Certus benchmark, then tear down the server. Server lifecycle follows the
pattern from `workbench/targets/evolve-throughput/evaluate.py`.

```bash
cd benchmarks/certus-mooncake-bench

# Full run: build server, benchmark certus (default: 4 NVMe drives auto-discovered)
./run.sh

# 2 drives instead of 4
./run.sh --drive-count 2

# Explicit PCI addresses (when NUMA placement matters)
./run.sh --device-pci 0000:61:00.0 --device-pci 0000:62:00.0

# Quick test (100 requests, skip build)
./run.sh --skip-build --max-requests 100

# Server already running externally
./run.sh --server-running

# All 3 trace scenarios
./run.sh --scenario all

# Smoke test only (disk backend, no server/GPU needed — OS disk, not NVMe)
./run.sh --disk-only --max-requests 100
```

### benchmark.py — manual (server must be running)

```bash
# Certus backend (requires running certus-server + GPU)
python benchmark.py --backend certus --scenario toolagent --max-requests 100

# Disk backend (framework smoke test — see SPDK note below)
python benchmark.py --backend disk --scenario toolagent --max-requests 100
```

## Traces

FAST25-release traces are bundled in `mooncake_traces/`:

| Trace | Requests | Description |
|-------|----------|-------------|
| `conversation_trace.jsonl` | ~12k | Multi-turn conversation |
| `synthetic_trace.jsonl` | ~5k | Synthetic workload |
| `toolagent_trace.jsonl` | ~12k | Tool-calling agent |

## Backend Comparison

| | Disk | Certus |
|---|---|---|
| **Data path** | `os.pread`/`os.pwrite` to file | GPU DMA via CUDA IPC handles |
| **Control path** | direct syscall | shmq shared-memory mailbox (~1 µs) |
| **Requirements** | None | Running `certus-server`, CUDA GPU |
| **Per-op model** | Single `pread`/`pwrite` | Single `ring.lookup`/`ring.populate` |

Both backends implement the same `Storage` ABC and are measured identically
(`time.perf_counter()` around each op), so latency numbers are directly
comparable.

### SPDK hardware note

On machines with VFIO-bound NVMe drives (the Certus production setup), the
disk backend writes to the OS disk / tmpfs (`--storage-dir`, default `/tmp`),
**not** to the NVMe drives Certus uses via SPDK userspace I/O. The disk
backend is useful as a framework smoke test, but not as a fair storage
comparison on SPDK hardware. For a fair A-vs-B, compare Certus numbers
against Mooncake's published FAST25 results or run the disk backend on a
machine with a normal filesystem on NVMe.

> **TODO:** Unbind one NVMe drive from VFIO, mount it with ext4/xfs, and
> point `--storage-dir` at the mount for a same-hardware disk-vs-certus
> comparison (e.g. `mkfs.ext4 /dev/nvme0n1`, `mount /mnt/nvme_bench`,
> `./run.sh --disk-only --storage-dir /mnt/nvme_bench`).

## Command Line Options

| Option | Default | Description |
|--------|---------|-------------|
| `--backend` | `disk` | `disk` or `certus` |
| `--trace-dir` | `mooncake_traces/` | Trace files directory |
| `--scenario` | `toolagent` | `conversation`, `synthetic`, `toolagent`, or `all` |
| `--storage-dir` | `/tmp/certus_mooncake_bench` | Storage directory (disk) |
| `--model` | `glm5` | `glm5` or `kimi-k2.6` |
| `--page-size-tokens` | `512` | Tokens per page |
| `--file-mode` | `single` | `single` or `per-file` (disk) |
| `--max-requests` | all | Max requests to process |
| `--max-pages` | `2000` | Max pages (modulo mapping if larger) |
| `--fsync-mode` | `none` | `none`, `batch`, `always`, `end` (disk) |
| `--threads` | `1` | Client worker threads |
| `--replay-scales` | `0` | Comma-separated fast-forward speeds; 0 = unpaced |
| `--progress-interval` | `100` | Print every N requests; 0 disables |
| `--shm-path` | `/dev/shm/certus-shmq` | shmq mailbox path (certus) |
| `--gpu-device` | `0` | CUDA device (certus) |

## How It Works

1. **Load trace**: JSONL file → list of `{timestamp, hash_ids, input_length, output_length}`
2. **Layout**: MLA model config converts each request's `hash_ids` into page access requirements
3. **Replay**: For each page access:
   - `exists(page_id)` → page written before?
   - Yes → `read(page_id)` (cache hit)
   - No → `write(page_id)` (cache miss / first access)
4. **Measure**: Per-op latency, request latency, QPS, bandwidth, hit rate

This is exactly how Mooncake's benchmark works. The only difference is the
storage backend behind the `read()`/`write()` calls.

## Certus Backend — Full Data Path

When you run `--backend certus`, every `read()` and `write()` call exercises
the complete Certus production stack, the same path a real vLLM inference
engine takes:

```
benchmark.py (Python)
  │
  │  storage.write(page_id)          storage.read(page_id)
  ▼                                  ▼
CertusStorage                       CertusStorage
  │                                  │
  │  ring.populate(key, region)      │  ring.lookup(key, region)
  ▼                                  ▼
┌──────────────────────────────────────────────────────────┐
│  shmq shared-memory mailbox (/dev/shm/certus-shmq)       │
│  Lock-free control plane: 68-byte message per op          │
│  ~1 µs round-trip (spin-then-futex wait for reply)        │
│  KV data bytes NEVER cross the ring                       │
└──────────────────────────────────────────────────────────┘
  │                                  │
  ▼                                  ▼
┌──────────────────────────────────────────────────────────┐
│  certus-server  (shmq-dispatcher serve loop)              │
│                                                           │
│  Translator decodes opcode → dispatches to dispatcher     │
│                                                           │
│  STORE (populate):                                        │
│    1. Dispatcher allocates slot in memory tier             │
│    2. CUDA IPC open → DMA: GPU → DRAM (memory tier)       │
│    3. Background write-through: DRAM → NVMe SSD           │
│       (SPDK userspace I/O, no kernel, no filesystem)      │
│                                                           │
│  LOAD (lookup):                                           │
│    Cache hit  → DMA: DRAM (memory tier) → GPU             │
│    Cache miss → SPDK NVMe read → DRAM → DMA → GPU        │
│       (cold path: SSD read + GPU DMA, the bottleneck)     │
│                                                           │
│  Eviction (LRU):                                          │
│    Memory tier full → demote cold pages DRAM → SSD        │
└──────────────────────────────────────────────────────────┘
  │                                  │
  ▼                                  ▼
┌──────────────────────────────────────────────────────────┐
│  SPDK NVMe driver (VFIO-bound, userspace, zero-copy)      │
│  Extent manager: fixed-size extents, crash-consistent     │
│  Direct PCIe DMA to/from NVMe controller                  │
└──────────────────────────────────────────────────────────┘
```

### What each metric measures with `--backend certus`

| Metric | What it includes |
|--------|-----------------|
| **Write latency** | shmq round-trip + dispatcher slot alloc + GPU→DRAM DMA + reply. Write-through to SSD is async (background), so write latency is DRAM-speed. |
| **Read latency (hit)** | shmq round-trip + DRAM→GPU DMA + reply. Sub-millisecond for hot pages. |
| **Read latency (miss)** | shmq round-trip + SPDK NVMe read + DRAM stage + GPU DMA + reply. This is the cold path — dominated by SSD read latency. |
| **QPS** | End-to-end requests/sec including all the above. |
| **Hit rate** | Fraction of pages already in memory tier (written before, not yet evicted). |
| **Bandwidth** | `total_bytes / total_time` — effective throughput through the full stack. |

### CertusStorage implementation

The `CertusStorage` class (`storage/certus.py`):
- Connects to `certus-server` via the shmq shared-memory mailbox
- Pre-allocates a pool of GPU buffers at init (one `cudaMalloc` + IPC handle each)
- On `write()`: `ring.populate([(key, [region])])` — server DMAs from GPU → DRAM → SSD
- On `read()`: `ring.lookup([(key, [region])])` — server DMAs from SSD/DRAM → GPU
- On `exists()`: in-memory `set` tracking (same as DiskHashTable)

The GPU buffer pool is reused round-robin across ops — no per-op CUDA allocation.
This matches how vLLM uses Certus (the KV-cache tensor is a single long-lived
GPU allocation).

## Comparing Against Mooncake

Mooncake's FAST25 benchmark (`storage_benchmark_v1`) uses the same traces, same
layout layer, and same metrics format. The difference is the storage backend:

| | Mooncake (DiskHashTable) | Certus |
|---|---|---|
| **I/O path** | Kernel `pread`/`pwrite` → page cache → NVMe | SPDK userspace → NVMe (no kernel) |
| **Data movement** | CPU buffer ↔ NVMe | GPU ↔ DRAM ↔ NVMe (zero-copy DMA) |
| **Tiering** | None (flat file) | Memory tier (DRAM) + SSD tier with LRU eviction |
| **Cache behavior** | OS page cache (opaque) | Explicit: memory tier hit/miss visible in metrics |

To compare:

1. Run Mooncake's benchmark on their hardware (or use their published numbers)
2. Run this benchmark with `--backend certus` on Certus hardware
3. Compare QPS, read/write latency (avg, p50, p95, p99), and bandwidth

The hit rate and write ratio will be identical (same trace, same layout, same
exists-then-read/write logic). The latency and throughput differences are what
Certus's SPDK+tiering architecture adds or saves vs. kernel I/O.

## run.sh Options

| Option | Default | Description |
|--------|---------|-------------|
| `--drive-count N` | `4` | Use first N discovered NVMe drives |
| `--device-pci PCI` | | NVMe PCI address (repeatable; overrides `--drive-count` for NUMA pinning) |
| `--disk-only` | | Run disk baseline only (no server) |
| `--skip-build` | | Don't rebuild certus-server |
| `--server-running` | | Don't start/stop server (assume external) |
| `--channels N` | `8` | shmq channels |
| `--memory-tier-size` | `4G` | Server memory tier size |
| `--gpu-device N` | `0` | CUDA device |

Plus all `benchmark.py` options: `--scenario`, `--max-requests`, `--max-pages`,
`--threads`, `--replay-scales`, `--progress-interval`, `--storage-dir`.

## Architecture

```
run.sh                    Server lifecycle + both backends (recommended entry point)
benchmark.py              Main runner with --backend flag
mooncake_traces/          FAST25 JSONL traces (conversation, synthetic, toolagent)
layout/
  interface.py            KVLayout ABC (unchanged from Mooncake)
  mla.py                  MLA model configs: GLM-5, Kimi-K2.6 (unchanged)
storage/
  interface.py            Storage ABC (unchanged from Mooncake)
  disk.py                 DiskHashTable — posix pread/pwrite (unchanged)
  certus.py               CertusStorage — shmq + CUDA IPC (NEW)
```
