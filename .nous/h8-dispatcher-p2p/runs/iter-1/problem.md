# Problem Framing: Pipelined Bounce vs. P2P Direct for 4 MiB SSD→GPU Transfer

## Research Question

Is a **pipelined bounce-buffer** path (SSD → host DRAM → GPU via cudaMemcpy H2D) faster than a **direct P2P** path (SSD → GPU VRAM via GDRCopy BAR1 DMA) for serving 4 MiB of KV-cache data broken into 128 KiB MDTS-limited chunks?

The existing dispatcher (`components/dispatcher/v1/src/pipeline.rs:30-123`) uses sequential per-chunk ReadSync + memcpy + cudaMemcpy. The `gpu-p2p-server` binary (`components/gpu-services/v0/src/bin/p2p_server.rs:28-36`) implements three transfer modes — bounce, p2p (warm), and p2p-cold — making it the ideal test harness.

## System Interface

- **Build command:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server
  ```

- **Runtime environment:**
  ```bash
  export LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH
  ```

- **Server CLI flags:**
  | Flag | Semantics | Default |
  |------|-----------|---------|
  | `--socket <PATH>` | Unix domain socket path | `/tmp/gpu_p2p_server.sock` |
  | `--pci <DDDD:BB:DD.F>` | NVMe PCI address | first found |
  | `--mode <bounce\|p2p\|p2p-cold>` | Transfer path | `p2p` |
  | `--staging-size <BYTES>` | Total GPU staging pool (p2p mode) | `4194304` |
  | `--chunk-size <BYTES>` | NVMe I/O chunk size (≤ MDTS) | `131072` |
  | `--once` | Exit after one client | off |

- **Code evidence:**
  - `--mode` defined: `p2p_server.rs:29-36` (ValueEnum)
  - `--chunk-size` parsed: `p2p_server.rs:57-59`
  - `--staging-size` parsed: `p2p_server.rs:53-55`
  - `handle_bounce` entry: `p2p_server.rs:374`
  - `handle_p2p` entry: `p2p_server.rs:436`
  - `do_chunked_read` uses `BatchSubmit`: `p2p_server.rs:286-301`

- **Client CLI:**
  ```bash
  python3 components/gpu-services/v0/tests/gpu_client_p2p.py <size_bytes> <socket_path> [--iterations N]
  ```
  Output: latency (ms), throughput (MB/s) printed to stderr.

- **Native output mechanism:** Client prints metrics to stderr in structured text. Server prints log messages to stderr. No JSON output file — metrics are parsed from the client's benchmark summary.

## Baseline Command

```bash
# Terminal 1 — server (bounce mode, 4 MiB staging, 128 KiB chunks)
rm -f /tmp/gpu_p2p_bench.sock && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/gpu-p2p-server \
  --socket /tmp/gpu_p2p_bench.sock \
  --pci 0000:63:00.0 \
  --mode bounce \
  --chunk-size 131072 \
  --staging-size 4194304

# Terminal 2 — client (4 MiB, 20 iterations)
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 components/gpu-services/v0/tests/gpu_client_p2p.py \
  4194304 /tmp/gpu_p2p_bench.sock --iterations 20
```

## Baseline Validation

Ran bounce mode with `--pci 0000:63:00.0`, 4 MiB, 10 iterations:
- **Exit code:** 0
- **Throughput:** 1044.9 MB/s
- **Avg latency:** 3.83 ms
- **Min latency:** 1.98 ms
- **Max latency:** 11.38 ms

Ran P2P (warm) mode on same device, 4 MiB, 10 iterations:
- **Exit code:** 0
- **Throughput:** 3002.3 MB/s
- **Avg latency:** 1.33 ms
- **Min latency:** 1.16 ms
- **Max latency:** 1.53 ms

Both modes produce successful transfers with parseable output.

## Experimental Conditions

### Condition A: Bounce (baseline)
```bash
# Server
./target/debug/gpu-p2p-server --socket /tmp/gpu_p2p_bench.sock --pci 0000:63:00.0 --mode bounce --chunk-size 131072 --staging-size 4194304

# Client
python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_bench.sock --iterations 20
```

### Condition B: P2P warm (pre-pinned GDRCopy staging, amortized setup)
```bash
# Server
./target/debug/gpu-p2p-server --socket /tmp/gpu_p2p_bench.sock --pci 0000:63:00.0 --mode p2p --chunk-size 131072 --staging-size 4194304

# Client
python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_bench.sock --iterations 20
```

### Condition C: P2P cold (per-request GDRCopy pin/unpin)
```bash
# Server
./target/debug/gpu-p2p-server --socket /tmp/gpu_p2p_bench.sock --pci 0000:63:00.0 --mode p2p-cold --chunk-size 131072 --staging-size 4194304

# Client
python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_bench.sock --iterations 20
```

### Condition D: P2P dispatcher integration (code change — add P2P path to dispatcher lookup)
This condition modifies `components/dispatcher/v1/src/pipeline.rs` to add a `direct_ssd_to_gpu` function that reads NVMe chunks directly into a pre-pinned GPU BAR1 buffer, bypassing the DRAM memory-tier copy. The intent is to measure whether integrating P2P at the dispatcher level (with its additional bookkeeping) preserves the P2P advantage observed in the standalone server.

## Success Criteria

- **Falsification of hypothesis (expected):** P2P warm throughput exceeds bounce throughput consistently across all seeds (directional: P2P > bounce).
- **Magnitude:** P2P warm avg latency is lower than bounce avg latency by more than measurement noise (observed: ~2.9x difference in probes).
- **Cold-start penalty:** P2P cold latency is higher than P2P warm but still measurable (validates that the GDRCopy amortization is necessary).
- **Statistical significance:** Each condition runs 20 iterations; the min/max range for P2P warm must not overlap with the min/max range for bounce.

## Constraints

- **Hardware:** NVIDIA A30 GPUs (2x), Intel SSDPF2KE032T9L NVMe (vfio-pci bound).
- **Kernel modules required:** `nvidia_peermem`, `gdrdrv`, `vfio_pci` (all loaded).
- **NVMe PCI address:** Use `0000:63:00.0` (0000:62:00.0 VFIO group 14 is busy from the probe run — should recover after 10s but prefer 63:00.0 for stability).
- **Chunk size:** 128 KiB (MDTS limit for these drives).
- **Transfer size:** 4 MiB (32 chunks × 128 KiB) — representative of KV-cache layer offload.
- **Build requires:** `RUSTFLAGS='-L /usr/local/lib'` for GDRCopy linking.
- **Stale SPDK locks:** Remove `/var/tmp/spdk_pci_lock_0000:63:00.0` before each run.

## Prior Knowledge

This is iteration 1. No active principles from prior experiments.

Initial probes suggest the hypothesis (bounce > P2P) is **false** — P2P warm shows ~2.9x advantage. The experiment will confirm this with statistical rigor and explore whether P2P cold (with setup overhead) still beats bounce.
