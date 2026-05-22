# Problem Framing: Pipelined Bounce Buffer Transfer

## Research Question

Can a **pipelined** bounce-buffer transfer (SSD→CPU→GPU with overlapped NVMe DMA and cudaMemcpyAsync H2D) match or exceed direct SSD→GPU (P2P via GDRCopy BAR1) for 4 MiB transfers composed of 32×128 KiB chunks?

The existing `handle_bounce` implementation (`components/gpu-services/v0/src/bin/p2p_server.rs:374-433`) is **two-phase sequential**: all NVMe reads complete before any H2D copy begins. Prior measurements (hypothesis-8 iter-2) show NVMe read phase ~790μs and H2D copy phase ~819μs — nearly equal durations using independent hardware (NVMe DMA engine vs GPU copy engine over PCIe). Pipelining these overlapping phases should reduce total time from ~1610μs to ~max(790,819) ≈ ~820μs, matching P2P's ~824μs.

**Key source files:**
- `components/gpu-services/v0/src/bin/p2p_server.rs` — server implementation (bounce at line 374, p2p at line 436)
- `components/gpu-services/v0/src/cuda_ffi.rs` — CUDA FFI bindings (currently lacks async stream APIs)
- `components/gpu-services/v0/tests/gpu_client_p2p.py` — benchmark client with `--iterations N`
- `components/interfaces/src/iblock_device.rs:205` — `ReadAsync` command, `:236` — `BatchSubmit`

## System Interface

- **Build command:** `RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p`
- **Server binary:** `target/debug/gpu-p2p-server`
- **Client script:** `components/gpu-services/v0/tests/gpu_client_p2p.py`

### CLI Flags (server — `p2p_server.rs:38-64`)

| Flag | Default | Semantics |
|------|---------|-----------|
| `--mode` | `p2p` | Transfer mode: `bounce`, `p2p`, `p2p-cold` |
| `--socket` | `/tmp/gpu_p2p_server.sock` | Unix domain socket path |
| `--chunk-size` | `131072` (128 KiB) | NVMe I/O chunk size |
| `--staging-size` | `4194304` (4 MiB) | Pre-allocated staging buffer size |
| `--once` | false | Serve one client then exit |
| `--pci` | auto-detect | NVMe PCI address |

### CLI Flags (client — `gpu_client_p2p.py:64-78`)

| Positional/Flag | Semantics |
|----------------|-----------|
| `<size_bytes>` | Transfer size (positional) |
| `<socket_path>` | Unix socket to connect to (positional) |
| `--iterations N` | Repeat N times, report stats |

### Code Evidence

- `--mode` parsed at: `p2p_server.rs:51` (ValueEnum)
- `--chunk-size` parsed at: `p2p_server.rs:58`
- `--staging-size` parsed at: `p2p_server.rs:55`
- `--once` parsed at: `p2p_server.rs:62`
- `BatchSubmit` dispatches individual `ReadAsync` ops: `p2p_server.rs:286-295`
- Each `ReadAsync` produces one `ReadDone` completion: `interfaces/src/iblock_device.rs:293`
- `cudaMemcpy` (synchronous only): `cuda_ffi.rs:96-101`
- No `cudaMemcpyAsync`/`cudaStreamCreate` in current FFI: confirmed via grep

### Output Format

The client (`gpu_client_p2p.py`) reports to stderr:
- Single iteration: `latency: <X.XX> ms, throughput: <Y.Y> MB/s`
- Benchmark mode (`--iterations N`): avg/min/max latency (ms), sustained throughput (MB/s)

The server returns `OK <bytes> (<mode>, <chunks> chunks)` or `ERROR: <msg>` over the socket.

## Baseline Command

```bash
# Terminal 1 — server (bounce mode, existing non-pipelined)
RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p && \
  sudo target/debug/gpu-p2p-server --mode bounce --chunk-size 131072 --once

# Terminal 2 — client (4 MiB transfer, 10 iterations)
python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_server.sock --iterations 10
```

## Baseline Validation

Build validated: `RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p` exits 0 (1.00s, dev profile). Binary produced at `target/debug/gpu-p2p-server`.

Prior experiment results (from hypothesis-8 iter-1/iter-2, same hardware):
- Bounce: avg latency 2.65 ms, throughput 1510 MB/s (4 MiB, 128 KiB chunks)
- P2P warm: avg latency 1.32 ms, throughput 3031 MB/s
- Decomposed: NVMe read ~790μs, H2D copy ~819μs (bounce mode)

Server launch requires `sudo` (SPDK/VFIO needs root-level IOMMU access). Cannot run a full end-to-end smoke test in this environment (no NVMe hardware in CI).

## Experimental Conditions

### Condition 1: Baseline (existing bounce — non-pipelined)

Server: `--mode bounce --chunk-size 131072 --staging-size 4194304 --once`
Client: `4194304 /tmp/gpu_p2p_server.sock --iterations 10`

This is the existing two-phase sequential implementation (all reads, then all copies).

### Condition 2: Pipelined Bounce (h-main)

**Code change required.** Add a new transfer mode `bounce-pipeline` to the server that:

1. **Add CUDA async stream FFI** to `cuda_ffi.rs`: declare `cudaMemcpyAsync`, `cudaStreamCreate`, `cudaStreamSynchronize`, `cudaStreamDestroy` (link against libcudart which already exports them).

2. **Add `BouncePipeline` variant** to `TransferMode` enum and CLI.

3. **Implement `handle_bounce_pipeline`** in `p2p_server.rs`:
   - Allocate 2 host DMA buffers (double-buffer, each chunk_size)
   - Create a CUDA stream for async H2D copies
   - Submit NVMe `ReadAsync` for chunk[0] into buffer A
   - Loop: on `ReadDone` for buffer A, launch `cudaMemcpyAsync H2D` from A, submit next `ReadAsync` into buffer B, swap A↔B
   - After last chunk: `cudaStreamSynchronize` to ensure final copy completes

Server: `--mode bounce-pipeline --chunk-size 131072 --staging-size 4194304 --once`
Client: `4194304 /tmp/gpu_p2p_server.sock --iterations 10`

### Condition 3: P2P warm (control — direct SSD→GPU)

Server: `--mode p2p --chunk-size 131072 --staging-size 4194304 --once`
Client: `4194304 /tmp/gpu_p2p_server.sock --iterations 10`

No code changes. Uses pre-pinned GDRCopy BAR1 staging buffers.

## Success Criteria

1. **Primary:** Pipelined bounce latency is within 30% of P2P warm latency (i.e., pipelined ≤ 1.3 × P2P)
2. **Secondary:** Pipelined bounce is measurably faster than non-pipelined bounce (≥20% latency reduction)
3. **Directional:** Pipelined bounce throughput should approach the minimum of (NVMe read bandwidth, PCIe H2D bandwidth) — estimated ~3000 MB/s given prior measurements showing NVMe read at ~5 GB/s and H2D at ~4.9 GB/s

## Constraints

- Transfer size: 4 MiB (matches KV-cache page granularity)
- Chunk size: 128 KiB (NVMe MDTS constraint, `p2p_server.rs:58`)
- Hardware requirements: NVIDIA GPU, NVMe on SPDK, nvidia-peermem + gdrdrv modules
- `sudo` required for server (SPDK/VFIO)
- Each condition runs with `--once` (single client) for controlled measurement

## Prior Knowledge

This is the first iteration of the h8-pipelined campaign (new campaign ID). However, substantial prior results exist from the original hypothesis-8 campaign:

- **Bounce (non-pipelined) = 2.65ms, P2P warm = 1.32ms** (iter-1)
- **NVMe read ~790μs, H2D copy ~819μs in bounce mode** (iter-2 decomposition)
- **D2D copy ~114μs** (iter-2) — GPU-to-GPU is 7x faster than H2D
- The near-equal read/copy times (790 vs 819μs) make this an ideal pipelining candidate
- No prior pipelined implementation exists in the codebase (confirmed by grep)
