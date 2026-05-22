# Problem Framing: Pipelined Bounce with CUDA-Native Pinned Memory

## Research Question

Does a pipelined bounce-buffer transfer using **CUDA-native pinned memory** (`cudaHostAlloc`) — registered with SPDK for NVMe DMA — achieve true async overlap and outperform non-pipelined bounce for 4 MiB transfers of 32×128 KiB chunks?

Iter-1 showed that pipelining with SPDK hugepage buffers + `cudaHostRegister` is 11-13% **slower** than non-pipelined bounce (2.07-2.12ms vs 1.87ms). Root cause: `cudaMemcpyAsync` from `cudaHostRegister`-ed SPDK hugepages does not execute truly asynchronously — it falls back to synchronous behavior internally, plus per-transfer register/unregister overhead (~200μs) dominates.

The fix (per RP-4): use `cudaHostAlloc` to allocate pipeline buffers. This memory is CUDA-native pinned from birth — `cudaMemcpyAsync` is guaranteed async. Register these buffers with SPDK via `create_spdk_dma_buffer_from_cuda_host_alloc` (`dma.rs:253`) so NVMe can DMA into them. Allocate once at startup, reuse across all requests — eliminating per-transfer registration overhead.

**Key source files:**
- `components/gpu-services/v0/src/bin/p2p_server.rs` — server (bounce at :374, dispatch at :642)
- `components/gpu-services/v0/src/cuda_ffi.rs` — CUDA FFI (cudaHostAlloc at :94, cudaMemcpyAsync via iter-1 patch at ~:114)
- `components/gpu-services/v0/src/dma.rs:253` — `create_spdk_dma_buffer_from_cuda_host_alloc` (allocates CUDA-pinned → registers with SPDK → wraps as DmaBuffer)
- `components/interfaces/src/iblock_device.rs:205` — `ReadAsync` command

## System Interface

- **Build command:** `RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p`
- **Server binary:** `target/debug/gpu-p2p-server`
- **Client script:** `components/gpu-services/v0/tests/gpu_client_p2p.py`

### CLI Flags (server — `p2p_server.rs:38-64`)

| Flag | Default | Semantics |
|------|---------|-----------|
| `--mode` | `p2p` | Transfer mode (p2p_server.rs:51, ValueEnum) |
| `--socket` | `/tmp/gpu_p2p_server.sock` | Unix domain socket (p2p_server.rs:43) |
| `--chunk-size` | `131072` (128 KiB) | NVMe I/O chunk size (p2p_server.rs:58) |
| `--staging-size` | `4194304` (4 MiB) | Staging buffer size (p2p_server.rs:55) |
| `--once` | false | Serve one client then exit (p2p_server.rs:62) |
| `--pci` | auto-detect | NVMe PCI address (p2p_server.rs:47) |

### CLI Flags (client — `gpu_client_p2p.py:64-78`)

| Argument | Semantics |
|----------|-----------|
| `<size_bytes>` | Transfer size (positional, line 77) |
| `<socket_path>` | Unix socket path (positional, line 78) |
| `--iterations N` | Repeat N times, report stats (line 69) |

### Code Evidence

- `cudaHostAlloc` FFI: `cuda_ffi.rs:94`
- `cudaMemcpyAsync` FFI: added by iter-1 patch (inside extern block after line 111)
- `cudaStreamCreate/Destroy/Synchronize`: added by iter-1 patch
- `create_spdk_dma_buffer_from_cuda_host_alloc`: `dma.rs:253` — allocates DmaBuffer from cudaHostAlloc ptr, registers with SPDK
- `spdk_unregister_and_cuda_free_host`: `dma.rs:232` — free function handling cleanup
- `DmaBuffer::from_raw`: `interfaces/src/spdk_types.rs:293` — wraps external memory as DmaBuffer
- `ReadAsync` command: `interfaces/src/iblock_device.rs:205`
- `ReadDone` completion: `interfaces/src/iblock_device.rs:293`
- Feature gates: `p2p = ["gpu", "spdk"]` at `Cargo.toml:11`

### Output Format

Client (`gpu_client_p2p.py`) reports to stderr:
- Single iteration: `latency: <X.XX> ms, throughput: <Y.Y> MB/s`
- Benchmark mode (`--iterations N`): Avg/Min/Max latency (ms), Throughput (MB/s)

## Baseline Command

```bash
# Terminal 1 — server (bounce mode, non-pipelined)
RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p && \
  sudo target/debug/gpu-p2p-server --mode bounce --chunk-size 131072

# Terminal 2 — client (4 MiB, 10 iterations benchmark)
python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_server.sock --iterations 10
```

Note: Do NOT use `--once` for benchmark mode — the client opens a fresh socket per iteration (gpu_client_p2p.py:46-61).

## Baseline Validation

Build validated: `RUSTFLAGS="-L /usr/local/lib" cargo build -p gpu-services --features p2p` exits 0, produces `target/debug/gpu-p2p-server`.

Iter-1 measured results (same hardware, same benchmark methodology):
- **Bounce (non-pipelined):** Avg 1.87ms, 2143 MB/s (stable across 2 seeds)
- **P2P warm:** Avg 1.27-1.34ms, 2976-3147 MB/s (stable, matches prior campaigns)
- **Bounce-pipeline (iter-1, SPDK hugepages + cudaHostRegister):** Avg 2.07-2.12ms, 1890-1935 MB/s (SLOWER than non-pipelined)

Cannot run full end-to-end smoke test in this environment (requires sudo + NVMe hardware + GPU).

## Experimental Conditions

### Condition 1: h-main — Pipelined bounce with CUDA-pinned buffers (`bounce-pipeline-v2`)

Code changes to `p2p_server.rs`:
- Add a new mode `BouncePipelineV2` (or modify the existing `BouncePipeline` variant)
- Implement `handle_bounce_pipeline_v2`:
  1. At startup (not per-request), allocate 2× chunk_size via `cudaHostAlloc` + `create_spdk_dma_buffer_from_cuda_host_alloc`
  2. Create one CUDA stream at startup (reuse across requests)
  3. Pipeline loop identical to iter-1 but WITHOUT any per-request `cudaHostRegister`/`cudaHostUnregister` calls
  4. Since the buffers are CUDA-native pinned, `cudaMemcpyAsync` will execute truly asynchronously

Key difference from iter-1: buffers are allocated ONCE with `cudaHostAlloc`, not SPDK hugepages with per-request `cudaHostRegister`. No per-request setup overhead.

Run command:
```bash
sudo target/debug/gpu-p2p-server --mode bounce-pipeline-v2 --chunk-size 131072
```

### Condition 2: h-control-negative — Non-pipelined bounce (existing `--mode bounce`)

No code changes. Same as iter-1's control.

Run command:
```bash
sudo target/debug/gpu-p2p-server --mode bounce --chunk-size 131072
```

### Condition 3: h-robustness — P2P warm (existing `--mode p2p`)

No code changes. Reference for target latency.

Run command:
```bash
sudo target/debug/gpu-p2p-server --mode p2p --chunk-size 131072
```

## Success Criteria

1. **h-main (direction):** Pipelined bounce v2 avg latency LOWER than non-pipelined bounce avg latency, consistently across both seeds. This directly tests whether true async overlap is achieved.
2. **h-main (magnitude):** Pipelined bounce v2 throughput higher than non-pipelined bounce by at least 15% (corresponding to meaningful overlap of the NVMe read and H2D copy phases).
3. **h-control-negative:** Non-pipelined bounce remains stable at ~1.87ms (±10%), confirming no system-level confound.
4. **h-robustness:** P2P warm remains stable at ~1.3ms (±10%), providing the theoretical lower bound.

## Constraints

- **sudo required** for server (SPDK/VFIO)
- **RUSTFLAGS="-L /usr/local/lib"** required for build (libgdrapi.so)
- **Do NOT use `--once`** for benchmark mode (client opens new socket per iteration)
- **MDTS limit:** chunk_size must remain 128 KiB
- **RP-4:** Must use `cudaHostAlloc` (not `cudaHostRegister` on SPDK buffers) for guaranteed async behavior
- **RP-2:** Baseline bounce may vary between NVMe devices (1.87ms observed, not the older 2.65ms)
- **RP-3:** P2P warm at ~1.27-1.34ms is the stable reference point

## Prior Knowledge

- **RP-1:** `cudaMemcpyAsync` does not achieve true async with SPDK hugepage buffers registered via `cudaHostRegister`. Per-transfer registration overhead (~200μs) plus synchronous fallback negates any overlap benefit. [confirmed by iter-1 — pipeline was 11-13% slower]
- **RP-2:** Non-pipelined bounce varies by NVMe device (1.87ms vs prior 2.65ms). Use same-run control for comparison, not cross-experiment baselines. [confirmed by iter-1 control arm]
- **RP-3:** P2P warm stable at ~1.27-1.34ms across runs and devices. [confirmed by iter-1 robustness arm]
- **RP-4:** For pipelined bounce to work, must use `cudaHostAlloc` memory. This ensures CUDA's DMA engine can pin-lock the memory in its own address space from birth, enabling true asynchronous H2D copies without internal staging. [untested — this iteration's hypothesis]
