# Problem Framing — h8-v1-pinned Iteration 2

## Research Question

Does P2P SSD→GPU DMA latency for 4 MiB objects (32 × 128 KiB sequential reads) improve when using a same-NUMA NVMe device (NODE-level PCIe, root complex `0000:60`) compared to the cross-NUMA NVMe (SYS-level, root complex `0000:c0`) tested in iter-1? Specifically: can NODE-level P2P with persistent GPU staging match or beat the pipelined bounce baseline at the same topology?

**Mechanism under study**: Iter-1 showed P2P is 65% slower than bounce when NVMe (`c2:00.0`, NUMA 1, root complex `0000:c0`) and GPU0 (`41:00.0`, NUMA 0, root complex `0000:40`) are connected at SYS level (cross-NUMA, cross-root-complex). The hypothesis is that some or all of this penalty comes from crossing the inter-socket link (AMD Infinity Fabric between NUMA 0 and NUMA 1). By using NVMe `63:00.0` (NUMA 0, root complex `0000:60`) — which shares NUMA affinity with GPU0 — the P2P DMA path only crosses one root-complex boundary (NODE level) instead of the NUMA interconnect (SYS level).

**Code implementing the mechanism**:
- `components/dispatcher/v1/src/pipeline.rs:30-123` — existing pipelined bounce path (sequential ReadSync, ring-buffer copy to memory-tier + cudaMemcpy to GPU)
- `components/dispatcher/v1/src/lib.rs:190-266` — `promote_and_serve` orchestrates the SSD read flow
- `components/interfaces/src/idispatcher.rs:112-118` — `IpcHandle` struct (needs `cuda_ipc_handle_bytes` extension for P2P routing)
- `components/interfaces/src/igpu_services.rs:460-463` — `prepare_memory_for_spdk` creates GPU DMA buffer
- `components/interfaces/src/spdk_types.rs:293-316` — `DmaBuffer::from_raw` for sub-buffer views

**PCIe topology (verified via `lspci -tvvv` and `nvidia-smi topo -m`)**:
- GPU0 (`41:00.0`): root complex `[0000:40]`, NUMA 0
- NVMe 63 (`63:00.0`): root complex `[0000:60]`, NUMA 0 — **NODE** to GPU0
- NVMe c2 (`c2:00.0`): root complex `[0000:c0]`, NUMA 1 — **SYS** to GPU0
- `nvidia-smi topo` legend: NODE = same NUMA, different root complex; SYS = cross-NUMA + cross-root-complex

## System Interface

- **Build command:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
- **CLI flags relevant to experiment:**
  - `--dispatcher-version v1` — selects dispatcher v1 (parsed at `apps/certus-server/src/main.rs:49`)
  - `--metadata-pci DDDD:BB:DD.F` — NVMe for metadata (parsed at `main.rs:29`)
  - `--data-pci DDDD:BB:DD.F` — NVMe for data (parsed at `main.rs:33`)
  - `--listen 0.0.0.0:50051` — gRPC endpoint (parsed at `main.rs:37`)
- **Code evidence for flags:**
  - `apps/certus-server/src/main.rs:27-51` — Clap `#[derive(Parser)]` struct defines all CLI flags
  - `apps/certus-server/src/main.rs:78-80` — `initialize_component_stack` receives PCI addresses
- **Native output mechanism:** Python client stdout table: `Tier  Avg (us/obj)  Min (us/obj)  Max (us/obj)  Avg (GB/s)  Peak (GB/s)`. Parse "SSD-tier" row for the primary metric.

## Baseline Command

```bash
# Server — NODE-level NVMe (63:00.0, NUMA 0)
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/certus-server \
  --metadata-pci 0000:63:00.0 \
  --data-pci 0000:63:00.0 \
  --dispatcher-version v1 \
  --listen 0.0.0.0:50051

# Client — 1-iteration first-hit measurement
cd apps/certus-server/python-client && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 test_client.py \
  --server localhost:50051 \
  --bench-only \
  --bench-object-size 4194304 \
  --bench-num-objects 10 \
  --bench-iterations 1
```

## Baseline Validation

From iter-1 results (with `c2:00.0`, SYS-level NVMe):
- Bounce SSD-tier: **6335.9 us/obj**, 0.66 GB/s (1 iteration, first-hit)
- P2P SSD-tier: **10498.3 us/obj**, 0.40 GB/s (65% slower than bounce)
- Memory-tier: 609.4 us/obj, 6.88 GB/s

From previously-run iter-2 (with `63:00.0`, NODE-level NVMe):
- Bounce SSD-tier: **21246.6 us/obj** (different object count: 5 not 10, and different test parameters)

The executor will re-run the baseline to get a fresh measurement with consistent parameters (10 objects, 1 iteration) using `63:00.0`.

## Experimental Conditions

### Condition A: Bounce baseline with NODE-level NVMe (`63:00.0`)
No code changes. Run stock dispatcher v1 with `--data-pci 0000:63:00.0`. Measures the pipelined bounce path performance at NODE-level PCIe topology.

### Condition B: P2P persistent staging with NODE-level NVMe (`63:00.0`)
Code changes to implement P2P direct SSD→GPU DMA:
- Add `cuda_ipc_handle_bytes: Option<Vec<u8>>` to IpcHandle struct
- Add GPU DMA cache to dispatcher v1 component (`HashMap<[u8;64], Arc<Mutex<DmaBuffer>>>`)
- Implement `p2p_ssd_to_gpu_persistent()` using sequential ReadSync into GPU DMA sub-buffers
- Route through P2P when `cuda_ipc_handle_bytes` is Some
- Pass `cuda_ipc_handle_bytes` from service.rs lookup handler
Server runs with `--data-pci 0000:63:00.0`.

### Condition C: Bounce baseline with SYS-level NVMe (`c2:00.0`)
No code changes. Run stock dispatcher v1 with `--data-pci 0000:c2:00.0` and `--metadata-pci 0000:c1:00.0`. Measures bounce path at SYS-level (reproduces iter-1 baseline configuration for comparison).

### Condition D: P2P persistent staging with SYS-level NVMe (`c2:00.0`)
Same code changes as Condition B. Server runs with `--data-pci 0000:c2:00.0` and `--metadata-pci 0000:c1:00.0`. Reproduces iter-1 P2P measurement for comparison.

## Success Criteria

1. **Primary (topology effect)**: The P2P penalty should be smaller at NODE level (B vs A) than at SYS level (D vs C). Specifically: `(B_lat/A_lat) < (D_lat/C_lat)`. If NODE P2P is 65% slower (same as SYS), topology is not the dominant factor.
2. **Secondary (competitiveness)**: If NODE-level P2P (B) achieves latency within 20% of NODE-level bounce (A), the hypothesis that PCIe topology is the barrier is validated and P2P becomes viable for same-NUMA configurations.
3. **Reproducibility**: Conditions C and D should approximately reproduce iter-1 measurements (C ≈ 6336 us, D ≈ 10498 us within ±30%).

## Constraints

- All benchmarks run through certus-server with `--dispatcher-version v1`
- Do NOT modify gpu-p2p-server
- Do NOT create standalone benchmark binaries
- P2P path uses IGpuServices::prepare_memory_for_spdk() with persistent caching
- Value size fixed at 4 MiB, chunk size 128 KiB (NVMe MDTS)
- 1-iteration measurement to avoid memory-tier confound (RP-6)
- NVMe devices: `63:00.0` (NUMA 0, NODE) and `c2:00.0` (NUMA 1, SYS)
- Debug build acceptable (PCIe DMA dominates CPU overhead)

## Prior Knowledge

- **RP-5**: P2P with pre-pinned GPU memory is slower than bounce when NVMe and GPU are on different root complexes (65% penalty at SYS level). [confirmed iter-1]
- **RP-6**: Multi-iteration benchmarks with different promotion policies produce invalid comparisons. [confirmed iter-1]
- **RP-7**: Caching prepare_memory_for_spdk eliminates per-lookup P2P overhead. [confirmed iter-1]
- **RP-8**: Before implementing P2P, verify PCIe topology — NVMe and GPU must share root complex/switch. Cross-root-complex adds 6-16ms for 4 MiB at 128 KiB chunks. [established iter-1]
- **PCIe topology** (verified this iteration): No NVMe on this system shares a root complex with GPU0. Best available: `63:00.0` at NODE level (same NUMA 0, root complex `0000:60` vs GPU0 at `0000:40`).
