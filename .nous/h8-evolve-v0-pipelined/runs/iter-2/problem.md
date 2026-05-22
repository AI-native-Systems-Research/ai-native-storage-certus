# Problem Framing — h8-evolve-v0-pipelined Iteration 2

## Research Question

Does replacing sequential NVMe reads (QD=1) with BatchSubmit (QD=32) in dispatcher v0's pipelined read path further reduce 4 MiB SSD-tier lookup latency beyond the 2x speedup achieved in iteration 1?

Iteration 1 proved that pre-allocated `cudaHostAlloc` buffers + per-chunk `cudaMemcpyAsync` achieves 9,659 us (2x over baseline 19,502 us). The remaining ~9,600 us is dominated by 32 sequential NVMe reads at QD=1 (~300 us each). By submitting all 32 reads simultaneously via `Command::BatchSubmit`, the NVMe controller's internal flash parallelism (multiple NAND dies) should reduce total NVMe time from ~9,600 us to approximately one read latency (~300-800 us).

**Mechanism under study:** `Command::BatchSubmit { ops: Vec<Command> }` dispatches all sub-commands to the same NVMe queue pair without blocking between them (`components/block-device-spdk-nvme/v2/src/actor.rs:750-768`). Each `ReadAsync` inside the batch calls `spdk_nvme_ns_cmd_read` (non-blocking SPDK submit, `actor.rs:623`) and accumulates in the pending ops map. The NVMe controller processes all 32 reads in parallel. Completions arrive as individual `Completion::ReadDone` messages on the callback channel.

**Key source files:**
- `components/block-device-spdk-nvme/v2/src/actor.rs:750-768` — BatchSubmit dispatch loop
- `components/block-device-spdk-nvme/v2/src/actor.rs:568-651` — ReadAsync implementation
- `components/dispatcher/v0/src/lib.rs:179-276` — Current sequential read path (target for modification)
- `components/gpu-services/v0/src/cuda_ffi.rs:71-111` — CUDA FFI (needs stream/async additions from iter-1)
- `components/gpu-services/v0/src/dma.rs:253-288` — `create_spdk_dma_buffer_from_cuda_host_alloc`

## System Interface

- **Build command:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
  Code evidence: `apps/certus-server/Cargo.toml` pulls dispatcher, gpu-services, interfaces crates.

- **Server start:**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /var/tmp/spdk_pci_lock_0000:64:00.0 && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/certus-server \
    --metadata-pci 0000:63:00.0 \
    --data-pci 0000:64:00.0 \
    --dispatcher-version v0 \
    --listen 0.0.0.0:50051
  ```
  Code evidence: `apps/certus-server/src/main.rs` (clap CLI parsing).

- **Client benchmark:**
  ```bash
  cd apps/certus-server/python-client && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 test_client.py \
    --server localhost:50051 \
    --bench-only \
    --bench-object-size 4194304 \
    --bench-num-objects 1 \
    --bench-iterations 1
  ```
  Code evidence: `apps/certus-server/python-client/test_client.py:310-439` (`bench_lookup_latency` function).

- **Output format:** Stdout table. Parse "SSD-tier" row for `Avg (us/obj)` and `Avg (GB/s)` columns.

- **Relevant CLI flags:**
  - `--dispatcher-version v0`: selects dispatcher v0 (`apps/certus-server/src/main.rs`)
  - `--bench-object-size 4194304`: 4 MiB value, producing 32 × 128 KiB chunks
  - `--bench-iterations 1`: first-hit cold measurement (avoids memory-tier promotion confound per RP-6)
  - `--bench-num-objects 1`: single object to isolate per-lookup latency

## Baseline Command

```bash
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /var/tmp/spdk_pci_lock_0000:64:00.0 && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/certus-server \
  --metadata-pci 0000:63:00.0 \
  --data-pci 0000:64:00.0 \
  --dispatcher-version v0 \
  --listen 0.0.0.0:50051 &
sleep 10 && \
cd apps/certus-server/python-client && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 test_client.py \
  --server localhost:50051 \
  --bench-only \
  --bench-object-size 4194304 \
  --bench-num-objects 1 \
  --bench-iterations 1
```

## Baseline Validation

From iteration 1 execution (same hardware, same command): Sequential v0 baseline produced **19,501.8 us/obj** (0.22 GB/s) for 4 MiB SSD-tier lookup. Build exits 0 in 0.17s. Server starts successfully with SPDK env init.

The iter-1 pipelined treatment (our new baseline for comparison) produced **9,659.3 us/obj** (0.43 GB/s).

## Experimental Conditions

### Condition 1: Baseline — Pipelined v0 (iter-1 implementation)
Double-buffered cudaHostAlloc + per-chunk ReadSync (QD=1) + cudaMemcpyAsync. This is the iter-1 treatment, now serving as baseline for the QD improvement.

Uses the same patch as iter-1's h-main (`runs/iter-1/patches/h-main.patch`).

### Condition 2: Treatment — BatchSubmit pipelined (QD=32)
Replace the sequential ReadSync loop with:
1. Pre-allocate 32 × 128 KiB `cudaHostAlloc` buffers (registered with SPDK via `create_spdk_dma_buffer_from_cuda_host_alloc`)
2. Submit all 32 reads as a single `Command::BatchSubmit { ops: vec![ReadAsync × 32] }`
3. Collect completions as they arrive; for each completed read, issue `cudaMemcpyAsync` for that chunk
4. After all 32 completions received, `cudaStreamSynchronize`

Key difference from iter-1: NVMe sees QD=32 instead of QD=1. All reads are in-flight simultaneously.

### Condition 3: Ablation — BatchSubmit + synchronous copy (QD=32, no async overlap)
Same as Condition 2 but with a single synchronous `cudaMemcpy` of the full 4 MiB after all 32 reads complete (no per-chunk async copies). Tests whether the NVMe QD=32 improvement alone is sufficient, or whether overlapping GPU copies with tail NVMe completions adds measurable value.

## Success Criteria

- **h-main (BatchSubmit pipelined):** SSD-tier latency consistently lower than iter-1 pipelined baseline (9,659 us). Expected magnitude: 4-8x reduction (target 1,200-2,500 us range based on h8-v1-pinned QD=32 result of 777 us for P2P).
- **h-ablation (BatchSubmit + sync copy):** Latency within 15% of h-main. If so, the async overlap provides negligible additional benefit at QD=32 (NVMe parallelism dominates).

## Constraints

- All benchmarks through certus-server with `--dispatcher-version v0` (campaign constraint)
- No modifications to gpu-p2p-server (campaign constraint)
- No standalone benchmark binaries (campaign constraint)
- Implementation changes in `components/dispatcher/v0/` only
- Do NOT reference components/dispatcher/v1/
- Use `--bench-iterations 1` for fair first-hit comparison (RP-6)
- Restart server between conditions (fresh process, fresh populate cycle)
- SPDK singleton: only one process per NVMe device
- `cudaHostAlloc` required for true async copy execution (RP-4)
- 32 × 128 KiB = 4 MiB total buffer allocation — well within memory limits
- Prior finding: bounce+batch ENOMEM at QD=4 was specific to v1 ring buffers (multiple SGL entries). `cudaHostAlloc` 128 KiB individual buffers should be physically contiguous → single SGL entry → no ENOMEM risk.

## Prior Knowledge

- **RP-9** [high confidence]: Per-chunk SPDK DmaBuffer::new is the dominant latency component for 4 MiB lookups in v0. Pre-allocated cudaHostAlloc buffers yield ~2x speedup.
- **RP-4** [high confidence]: cudaMemcpyAsync requires cudaHostAlloc memory for true async. cudaHostRegister on SPDK hugepages falls back to sync.
- **RP-11** [medium confidence]: cudaMemcpyAsync provides ~16% benefit over cudaMemcpy even for single transfers (CPU sync overhead avoidance).
- **RP-10** [high confidence]: P2P BAR1 DMA not faster than host-bounce at NODE level. (P2P excluded from this iteration.)
- **h8-v1-pinned RP-8**: P2P+BatchSubmit QD=32 achieves 777 us for 4 MiB — proves NVMe QD=32 can deliver sub-millisecond reads. (Different code path but validates the mechanism.)
