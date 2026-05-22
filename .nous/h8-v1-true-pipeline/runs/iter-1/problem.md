# Problem Framing — Dispatcher v1 True Pipelining vs P2P

## Research Question

Can dispatcher v1's `pipelined_ssd_to_gpu` function be modified to truly overlap NVMe reads with GPU DMA copies (using CUDA async streams), and does this overlapped pipeline outperform direct P2P SSD→GPU DMA for 4 MiB lookups (32x128 KiB chunks) through certus-server?

Key source files:
- `components/dispatcher/v1/src/pipeline.rs:60-119` — current sequential "pipeline" (ReadSync + sync cudaMemcpy per chunk, no overlap)
- `components/gpu-services/v0/src/cuda_ffi.rs` — CUDA FFI bindings (lacks cudaMemcpyAsync/cudaStream)
- `components/dispatcher/v1/src/lib.rs:190-266` — `promote_and_serve` calls `pipelined_ssd_to_gpu`
- `apps/certus-server/src/main.rs:191-209` — memory-tier pool cudaHostRegister (enables async DMA)

## System Interface

- **Build command:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server`
- **Run server:** `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH sudo target/debug/certus-server --metadata-pci 0000:62:00.0 --data-pci 0000:63:00.0 --dispatcher-version v1 --listen 0.0.0.0:50051`
- **Run benchmark:** `python3 apps/certus-server/python-client/test_client.py --server localhost:50051 --bench-only --bench-object-size 4194304 --bench-num-objects 10 --bench-iterations 1`

### CLI Flags (certus-server):
| Flag | Type | Semantics | Code evidence |
|------|------|-----------|---------------|
| `--metadata-pci` | String | PCI address for metadata NVMe (format: DDDD:BB:DD.F) | `apps/certus-server/src/main.rs:30` |
| `--data-pci` | Vec<String> | PCI address(es) for data NVMe (repeatable) | `apps/certus-server/src/main.rs:34` |
| `--dispatcher-version` | String | "v0" or "v1" (default "v1") | `apps/certus-server/src/main.rs:49` |
| `--listen` | String | gRPC listen address (default "0.0.0.0:50051") | `apps/certus-server/src/main.rs:37` |

### CLI Flags (test_client.py):
| Flag | Type | Semantics | Code evidence |
|------|------|-----------|---------------|
| `--bench-only` | bool | Skip functional tests, run only benchmark | `apps/certus-server/python-client/test_client.py:453` |
| `--bench-object-size` | int | Object size in bytes (default 65536) | `apps/certus-server/python-client/test_client.py:457` |
| `--bench-num-objects` | int | Objects per tier to benchmark (default 100) | `apps/certus-server/python-client/test_client.py:461` |
| `--bench-iterations` | int | Lookup iterations per tier (default 10) | `apps/certus-server/python-client/test_client.py:465` |

### Output format:
Benchmark prints to stdout a table with columns: Tier, Avg (us/obj), Min (us/obj), Max (us/obj), Avg (GB/s), Peak (GB/s). The SSD-tier row is the relevant metric (cold lookups from SSD, hitting the pipeline path).

## Baseline Command

```bash
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH sudo target/debug/certus-server --metadata-pci 0000:62:00.0 --data-pci 0000:63:00.0 --dispatcher-version v1 --listen 0.0.0.0:50051 &
sleep 5
cd apps/certus-server/python-client
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH python3 test_client.py --server localhost:50051 --bench-only --bench-object-size 4194304 --bench-num-objects 10 --bench-iterations 1
```

## Baseline Validation

From h8-v1-pinned iter-2 condition-A (identical hardware, same code path, NVMe 63:00.0 NODE-level):
- Exit code: 0
- SSD-tier latency: **7,029 us/obj** (4 MiB, 10 objects, 1 iteration, first-hit cold)
- Memory-tier latency: 585 us/obj
- SSD/Memory-tier ratio: 12.0x

The 7,029 us for 32 chunks implies ~220 us/chunk average (includes ReadSync wait + memcpy to memory-tier + sync cudaMemcpy H2D).

## Experimental Conditions

### Condition A: Baseline (existing sequential v1 pipeline)
No code changes. Runs with the existing `pipelined_ssd_to_gpu` which does:
1. ReadSync → wait → memcpy to mem-tier → cudaMemcpy to GPU, per chunk sequentially

### Condition B: True overlapped pipeline (h-main treatment)
Modify `pipeline.rs` to implement double-buffered async overlap:
1. Add `cudaStream_t`, `cudaStreamCreate`, `cudaStreamDestroy`, `cudaStreamSynchronize`, `cudaMemcpyAsync` to `cuda_ffi.rs`
2. Rewrite `pipelined_ssd_to_gpu` to use `ReadAsync` + `cudaMemcpyAsync`:
   - Pre-issue ReadAsync for chunk 0 into ring buffer 0
   - For each subsequent chunk: wait for previous ReadDone, launch cudaMemcpyAsync H2D for completed chunk, memcpy to mem-tier, issue ReadAsync for next chunk into alternate buffer
   - After last chunk: wait for final ReadDone, launch final cudaMemcpyAsync, cudaStreamSynchronize
3. This overlaps NVMe read of chunk N+1 with GPU copy of chunk N

### Condition C: P2P direct SSD→GPU DMA (comparison)
Apply the P2P patch from h8-v1-pinned (adds `cuda_ipc_handle_bytes` to IpcHandle, `p2p_ssd_to_gpu_persistent` to pipeline.rs, `get_or_create_gpu_dma` to lib.rs). This bypasses memory-tier entirely and DMAs directly from SSD to GPU BAR1 memory.

## Success Criteria

- **Primary:** True pipeline (condition B) achieves lower SSD-tier latency than baseline (condition A) — demonstrating async overlap works.
- **Stretch:** True pipeline latency approaches P2P latency (condition C), ideally within 2x of P2P.
- **Mechanism validation:** If NVMe read (~100-200 us/chunk) and GPU copy (~50-100 us/chunk) truly overlap, theoretical max speedup is ~1.5-2x over sequential.

## Constraints

- All benchmarks through certus-server (no standalone binaries)
- Implementation changes only in `components/dispatcher/v1/` and `components/gpu-services/v0/src/cuda_ffi.rs`
- Do NOT use or modify gpu-p2p-server
- Use `--bench-iterations 1` for fair first-hit comparison
- Restart server between conditions (fresh process, fresh populate cycle)
- NVMe device: 0000:63:00.0 (NODE-level topology, same NUMA as GPU0)

## Prior Knowledge

From h8-pipelined experiments:
- `cudaHostRegister` on SPDK hugepage buffers does NOT enable true async cudaMemcpyAsync (falls back to sync). CUDA needs natively-pinned memory.
- However, memory-tier pool is registered with `cudaHostRegister` at certus-server startup (main.rs:191-209). The memory-tier pool is mmap'd, not SPDK hugepages — need to verify whether cudaMemcpyAsync works from this registered memory.
- If it doesn't work async, the fallback approach is: allocate pipeline ring buffers with `cudaHostAlloc`, SPDK-register them, read NVMe into those, then async-copy from those to GPU (skip memory-tier on the GPU path).

From h8-v1-pinned iter-2:
- P2P achieves 3,451 us/obj vs bounce 7,029 us/obj (~2x speedup, NODE-level topology)
- NVMe 63:00.0 is on NUMA 0, same as GPU0 — optimal PCIe topology
