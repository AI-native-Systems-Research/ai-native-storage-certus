# Problem Framing: Bounce Buffer vs Direct P2P for SSD→GPU Transfer

## Research Question

Is a two-stage bounce buffer path (NVMe → host DMA buffer → cudaMemcpy H2D → GPU) faster than direct NVMe → GPU P2P DMA (via GDRCopy BAR1 mapping) for transferring 4 MiB as a stream of 32 x 128 KiB chunks?

The relevant implementations are:
- **Bounce**: `components/gpu-services/v0/src/bin/p2p_server.rs:374` (`handle_bounce`) — allocates per-chunk SPDK hugepage host DMA buffers, issues concurrent NVMe reads via `BatchSubmit`, then copies to GPU via sequential `cudaMemcpy H2D` calls.
- **P2P (warm)**: `components/gpu-services/v0/src/bin/p2p_server.rs:436` (`handle_p2p`) — uses a pre-allocated pool of GDRCopy-pinned GPU staging buffers (BAR1 mapped), issues concurrent NVMe reads directly into GPU memory via SPDK DMA, then copies D2D to the client buffer.
- **P2P (cold)**: `components/gpu-services/v0/src/bin/p2p_server.rs:493` (`handle_p2p_cold`) — same as P2P but allocates and GDRCopy-pins fresh staging buffers per request (measures setup overhead).

The causal mechanism under investigation: NVMe DMA into host memory follows the native PCIe TLP path (root complex → DRAM), which is a fast, well-optimized datapath. NVMe DMA into GPU BAR1 requires the TLPs to traverse the PCIe fabric to the GPU's BAR1 aperture, which may incur higher per-TLP latency due to GPU memory controller scheduling. The bounce path pays an extra `cudaMemcpy H2D` cost but may recover that through faster NVMe read completion. Alternatively, the D2D copy in P2P (staying within GPU memory) may be so much cheaper than H2D that it more than compensates for any NVMe DMA penalty.

## System Interface

### Build Command

```bash
RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server
```

Requires `RUSTFLAGS=-L /usr/local/lib` because `libgdrapi.so` (GDRCopy) is installed at `/usr/local/lib`. Also requires:
- SPDK pre-built at `deps/spdk-build/`
- CUDA toolkit (libcudart)
- Kernel modules: `nvidia-peermem`, `gdrdrv`, `vfio-pci`

### CLI Flags

| Flag | Default | Semantics | Code evidence |
|------|---------|-----------|---------------|
| `--mode` | `p2p` | Transfer mode: `bounce`, `p2p`, `p2p-cold` | `p2p_server.rs:50` (ValueEnum) |
| `--chunk-size` | `131072` (128 KiB) | NVMe I/O chunk size | `p2p_server.rs:58` |
| `--staging-size` | `4194304` (4 MiB) | Pre-allocated GPU staging pool size (p2p mode only) | `p2p_server.rs:54` |
| `--socket` | `/tmp/gpu_p2p_server.sock` | Unix socket path | `p2p_server.rs:42` |
| `--pci` | (first device) | NVMe PCI address (DDDD:BB:DD.F) | `p2p_server.rs:46` |
| `--once` | false | Serve one client then exit | `p2p_server.rs:62` |

### Client

Python client: `components/gpu-services/v0/tests/gpu_client_p2p.py`

```bash
python3 components/gpu-services/v0/tests/gpu_client_p2p.py <size_bytes> <socket_path> --iterations N
```

- Allocates GPU memory via `cudaMalloc`
- Exports CUDA IPC handle
- Connects to Unix socket, sends base64-encoded payload (64B handle + 8B LE size = 72B)
- Measures end-to-end latency per transfer (wall-clock from send to response)
- Reports: throughput (MB/s), avg/min/max latency (ms)

Code evidence: `gpu_client_p2p.py:49,58` (timing), `gpu_client_p2p.py:143-151` (benchmark output).

### Output Format

Metrics are printed to **stderr** by the client in benchmark mode:
```
============================================================
  GPU P2P DMA Benchmark: 4.00 MB x 50 iterations
============================================================
  Throughput:    XXXX.X MB/s
  Avg latency:   X.XX ms
  Min latency:   X.XX ms
  Max latency:   X.XX ms
  Total data:    XXX.X MB in X.XXX s
============================================================
```

The `run_condition.sh` harness captures combined stdout+stderr to the output file via `> "$OUTPUT" 2>&1`.

## Baseline Command

```bash
bash /home/nara/certus/ai-native-storage-certus/.nous/h8-transfer-path/runs/iter-1/inputs/run_condition.sh \
  bounce \
  /home/nara/certus/ai-native-storage-certus/.nous/h8-transfer-path/runs/iter-1/results/h-main/bounce-s1.txt \
  0000:62:00.0
```

## Baseline Validation

Previously executed on hardware (NVIDIA A30 + NVMe at PCI 0000:62:00.0). Results observed:
- **Exit code**: 0
- **Output file**: `results/h-main/bounce-s1.txt`
- **Key metrics**: Throughput 1544.0 MB/s, Avg latency 2.59 ms, Min latency 1.76 ms (50 iterations of 4 MiB)

Confirmed: the command works, server starts within 5s, client completes, output is parseable.

## Experimental Conditions

### h-main arm: Bounce vs P2P Warm

Two conditions compared, each run twice (2 seeds x 50 iterations = 100 measurements per mode):

1. **bounce** — Server with `--mode bounce`. NVMe reads target host DMA buffers (SPDK hugepages), followed by 32 sequential `cudaMemcpy(H2D)` at 128 KiB each.

2. **p2p-warm** — Server with `--mode p2p`. NVMe reads target pre-pinned GPU BAR1 staging buffers, followed by 32 sequential `cudaMemcpy(D2D)` at 128 KiB each.

Both use identical NVMe read logic (`do_chunked_read` at `p2p_server.rs:272` with BatchSubmit). The difference is:
- DMA target: host hugepage memory vs GPU BAR1 aperture
- Copy type: H2D (PCIe bus traversal) vs D2D (intra-GPU)

### h-control-negative arm: P2P Cold

One condition run twice:

3. **p2p-cold** — Server with `--mode p2p-cold`. Per-request GDRCopy setup (32 x cudaMalloc + gdr_pin + gdr_map + spdk_mem_register), then NVMe reads + D2D copies, then full teardown. Isolates the P2P DMA mechanism from the warm pool optimization.

## Success Criteria

- **h-main**: If bounce achieves consistently lower average latency AND higher throughput than P2P warm across both seeds, the bounce-is-faster hypothesis is supported. If P2P warm is faster across both seeds, the hypothesis is refuted.
- **h-control-negative**: P2P cold should exhibit consistently higher latency than P2P warm, confirming that GDRCopy setup overhead is the cost being amortized (not an inherent P2P DMA limitation).

## Constraints

- NVMe chunk size fixed at 128 KiB (MDTS limit, `p2p_server.rs:57`).
- Transfer size fixed at 4 MiB (32 chunks).
- 50 iterations per measurement with 1 warmup iteration (client-side, `gpu_client_p2p.py:120`).
- SPDK exclusive device access: only one server process per NVMe device.
- GPU staging buffers must be 64 KiB aligned (GDRCopy requirement, `gdrcopy_ffi.rs:17`).
- Server needs ~5 seconds to initialize SPDK+CUDA stack.
- Build requires `RUSTFLAGS='-L /usr/local/lib'` and `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64`.

## Prior Knowledge

- **RP-1** (high confidence): For NVMe→GPU transfers with 128 KiB chunks on NVIDIA A30, direct P2P DMA with pre-warmed GDRCopy staging achieves ~2x higher throughput (~3000 MB/s vs ~1500 MB/s) and ~2x lower latency (~1.3ms vs ~2.6ms) than bounce buffer path.
- **RP-2** (high confidence): GDRCopy per-request pin/map/unpin overhead for 32 x 128 KiB buffers adds ~6ms latency per 4 MiB transfer (~5.6x penalty vs pre-allocated pool).

These principles inform expected outcomes but do not override the experimental design — the experiment independently tests the stated hypothesis.
