# Problem Framing — Iteration 3: Direct GPU Memory Registration via nvidia-peermem

## Research Question

Can NVMe reads be directed into the final GPU destination memory (IPC-opened pointer) by registering it with SPDK via `spdk_mem_register` + nvidia-peermem, bypassing both the DRAM memory-tier and the H2D copy entirely?

Previous iterations (1 & 2) attempted P2P via GDRCopy BAR1 staging — allocating GPU memory, mapping it to a BAR1 VA via GDRCopy, NVMe-reading into the BAR1 VA, then copying to the final GPU destination. This failed catastrophically (0.01 GB/s vs 2.4 GB/s baseline) because there is no efficient CUDA path to move data from BAR1-staged memory to another GPU allocation (RP-1, RP-4).

The new approach uses a fundamentally different mechanism: `spdk_mem_register` on the GPU device pointer itself (via nvidia-peermem kernel module), which makes the GPU memory directly addressable by the NVMe DMA engine through the IOMMU — no GDRCopy, no BAR1 mapping, no staging, no secondary copy.

**Key source files:**
- `components/gpu-services/src/dma.rs:189` — `create_spdk_dma_buffer_from_cuda_malloc`: registers cudaMalloc'd GPU memory with SPDK
- `components/gpu-services/src/dma.rs:114` — `create_spdk_dma_buffer_from_gpu`: registers IPC-opened GPU memory with SPDK  
- `components/gpu-services/src/lib.rs:333` — `prepare_memory_for_spdk`: full flow that opens IPC handle + registers with SPDK (validates this path works)
- `components/dispatcher/src/pipeline.rs:244` — `pipelined_ssd_to_gpu_zero_copy`: sliding-window pipeline (template for direct GPU path)
- `components/dispatcher/src/lib.rs:194` — `promote_and_serve`: cold lookup entry point

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **CLI flags:** `--drive-count 1 --format` (format extents, single drive)
- **Code evidence:**
  - `p2p` feature: `apps/certus-server/Cargo.toml:9` propagates to `gpu-services/p2p` and `dispatcher/p2p`
  - `spdk_mem_register` extern: `components/gpu-services/src/dma.rs:12`
  - nvidia-peermem check: `components/gpu-services/src/dma.rs:125` (error message confirms requirement)
- **Output:** stdout from benchmark client, look for "Lookup (cold)" section

## Baseline Command

```bash
pkill -f certus-server; sleep 2
./target/release/certus-server --drive-count 1 --format &
sleep 6
python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304
pkill -f certus-server
```

## Baseline Validation

From iter-2 baseline results (validated):
- Exit code: 0
- Cold lookup throughput: 2.39 GB/s (per-client)
- Cold lookup avg latency: 1752 us
- Cold lookup p99 latency: 1950 us
- Data integrity: PASS

## Experimental Conditions

### Condition 1: h-main — Direct NVMe→GPU via nvidia-peermem (no DRAM)

Replace the cold lookup data path to read NVMe directly into the GPU IPC destination pointer:

1. Register the GPU destination with SPDK (`spdk_mem_register` on the IPC-opened pointer)
2. Create noop-free DmaBuffer wrappers for chunks of the GPU destination (same pattern as zero-copy uses for memory-tier)
3. Use the existing sliding-window pipeline to read NVMe into those DmaBuffers (which point directly into GPU memory)
4. Unregister from SPDK after transfer completes
5. Skip memory-tier allocation entirely — register as BlockDevice-only in dispatch-map

This eliminates both the NVMe→DRAM and DRAM→GPU copies. The data path becomes a single PCIe traversal: NVMe controller DMA writes → IOMMU → GPU device memory.

**Env var control:** Set `P2P_DIRECT=1` to activate this path. Absence uses baseline.

### Condition 2: h-ablation — Direct NVMe→GPU with DRAM backfill

Same as h-main but also maintains the memory-tier (DRAM) cache:

1. Register GPU destination with SPDK
2. Read NVMe directly into GPU memory (same as h-main)
3. After each chunk completes, also copy from GPU to DRAM memory-tier slot (D2H async)
4. Register as MemoryTier entry in dispatch-map (enabling warm-path H2D on subsequent lookups)
5. Unregister GPU pointer from SPDK

This tests whether the overhead of maintaining DRAM cache (D2H copy per chunk) is acceptable when the primary path is already direct to GPU.

### Condition 3: h-control-negative — Baseline (NVMe→DRAM→GPU)

Unmodified baseline: the existing `pipelined_ssd_to_gpu_zero_copy` path.

## Success Criteria

- **h-main throughput:** Must exceed baseline (2.4 GB/s). Expected: 3.5-5.9 GB/s (elimination of H2D copy removes one PCIe crossing; upper bound is single-drive NVMe ceiling).
- **h-main p99 latency:** Must be lower than baseline (~1.95ms). Expected: 1.0-1.4ms.
- **Data integrity:** MUST PASS for all conditions.
- **Build:** Must compile without errors.

## Constraints

- RP-1: Do NOT use GDRCopy BAR1 staging — it is fundamentally broken for this use case.
- RP-2: Use sliding-window (not batch) pipeline pattern.
- RP-3: Avoid CPU reads from GPU BAR1 memory.
- RP-4: Do not rely on CUDA D2D copies from BAR1-staged memory.
- nvidia-peermem kernel module must be loaded.
- Server must be killed and restarted between conditions.
- Wait 6+ seconds after server start for SPDK init.

## Prior Knowledge

All four active principles (RP-1 through RP-4) apply. They constrain us away from the GDRCopy BAR1 path. This iteration uses a completely different mechanism (nvidia-peermem direct registration) that is not affected by any of the identified failure modes:
- No BAR1 VA is created (no GDRCopy involvement)
- No D2D copy from BAR1 memory is needed (data lands in final location)
- No CPU read from BAR1 (no backfill from BAR1)
- The pipeline pattern remains sliding-window (RP-2 satisfied)
