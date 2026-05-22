# Problem Framing — Dispatcher v0: Bounce vs P2P with Persistent GPU Staging

## Research Question

Can direct NVMe→GPU P2P DMA with **pre-pinned, persistent GPU staging** outperform the bounce buffer path (NVMe→host DRAM→GPU cudaMemcpy) for 4 MiB lookups in dispatcher v0?

The previous experiment (h8-v0-vs-p2p iter-1) showed that naive P2P (calling `prepare_memory_for_spdk` per lookup) is 33% **slower** than bounce due to per-call cudaIpcOpenMemHandle + spdk_mem_register overhead (~4-5ms each). This iteration tests whether **amortizing** that setup cost — by pre-pinning a persistent GPU staging buffer at batch start and reusing it across all lookups — eliminates the overhead advantage of bounce and reveals the underlying DMA path performance.

Key mechanism under test:
- **Bounce path** (`components/dispatcher/v0/src/lib.rs:179-276`): allocates host DMA buffer, reads 32×128 KiB chunks sequentially via ReadSync, then does one `gpu.dma_copy_to_device` (4 MiB cudaMemcpy H2D).
- **P2P-pinned path** (to implement): calls `prepare_memory_for_spdk` ONCE at the start of the lookup batch to obtain a persistent GPU DMA buffer, then for each lookup, reads chunks directly into GPU BAR1 sub-views. No per-lookup setup cost.

Constraint: The P2P path must also promote data to a host DRAM buffer (same as bounce path behavior) so subsequent lookups for the same key can hit DRAM. In v0, BlockDevice lookups always read from SSD (no caching), so this means the P2P path reads to GPU AND copies data to a staging buffer for the dispatch map.

## System Interface

- **Build command:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```

- **CLI flags relevant to experiment:**
  | Flag | Semantics | Code evidence |
  |------|-----------|---------------|
  | `--dispatcher-version v0` | Selects DispatcherComponentV0 (staging-based, no memory tier) | `apps/certus-server/src/main.rs:236-257` |
  | `--metadata-pci` | PCI address of metadata NVMe device | `apps/certus-server/src/main.rs:29` |
  | `--data-pci` | PCI address of data NVMe device(s) | `apps/certus-server/src/main.rs:33` |
  | `--listen` | gRPC listen address (default 0.0.0.0:50051) | `apps/certus-server/src/main.rs:37` |

- **Python client flags:**
  | Flag | Semantics |
  |------|-----------|
  | `--bench` | Run lookup latency benchmark |
  | `--bench-only` | Skip functional tests, benchmark only |
  | `--bench-object-size` | Object size in bytes (default 65536) |
  | `--bench-num-objects` | Objects per tier to benchmark (default 100) |
  | `--bench-iterations` | Lookup iterations per tier (default 10) |

- **Code evidence for key mechanisms:**
  - ReadSync sequential pattern: `components/dispatcher/v0/src/lib.rs:218-263`
  - `dma_copy_to_device` final copy: `components/dispatcher/v0/src/lib.rs:266-273`
  - `prepare_memory_for_spdk` implementation: `components/gpu-services/v0/src/lib.rs:330-479`
  - `IpcHandle` struct: `components/interfaces/src/idispatcher.rs:113-118`
  - `io_segmenter::segment_io`: `components/dispatcher/v0/src/io_segmenter.rs:22-55`
  - MDTS = 128 KiB (131072): `components/block-device-spdk-nvme/v2/src/controller.rs:158`

- **Output format:** Stdout table with columns: Tier, Avg (us/obj), Min (us/obj), Max (us/obj), Avg (GB/s), Peak (GB/s). Parse "SSD-tier" row for the primary metric.

## Baseline Command

```bash
# Terminal 1 — server
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/certus-server \
  --metadata-pci 0000:63:00.0 \
  --data-pci 0000:63:00.0 \
  --dispatcher-version v0 \
  --listen 0.0.0.0:50051

# Terminal 2 — client benchmark
cd apps/certus-server/python-client && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 test_client.py \
  --server localhost:50051 \
  --bench \
  --bench-object-size 4194304 \
  --bench-num-objects 10 \
  --bench-iterations 20
```

## Baseline Validation

From h8-v0-vs-p2p iter-1 (same hardware, same codebase):
- Exit code: 0
- SSD-tier avg latency: 13763.8 us/obj (0.30 GB/s) for bounce path with 4 MiB objects
- Memory-tier avg latency: 19315.7 us/obj (0.22 GB/s)
- All 10/10 functional tests pass

The baseline command is validated and working.

## Experimental Conditions

### Condition A: Bounce baseline (no code changes)

The existing `read_from_block_device` path at `lib.rs:179-276`. No modifications needed. This is the control.

### Condition B: P2P with persistent staging

Code changes to dispatcher v0 and certus-server:

1. **Add `cuda_ipc_handle_bytes` field to `IpcHandle`** (`components/interfaces/src/idispatcher.rs:113-118`)
   - Add `pub cuda_ipc_handle_bytes: Option<Vec<u8>>` to carry raw 64-byte CUDA IPC handle for P2P path.
   - Update all construction sites (service.rs populate, test code) to include `cuda_ipc_handle_bytes: None`.

2. **Modify certus-server lookup handler** (`apps/certus-server/src/service.rs:176-253`)
   - Instead of opening CUDA IPC handle per entry and caching, pass raw bytes through to dispatcher.
   - Call `prepare_memory_for_spdk` ONCE at the start of the batch (using the first entry's handle bytes) to pre-pin the GPU buffer.
   - Pass `cuda_ipc_handle_bytes: Some(bytes)` in IpcHandle for each entry.
   - After all lookups complete, drop the pinned GPU DmaBuffer (cleanup happens via Drop).

3. **Add `read_from_block_device_p2p_pinned` method** (`components/dispatcher/v0/src/lib.rs`, after line 276)
   - Accept a pre-pinned GPU DmaBuffer reference (passed from the service layer or cached in a per-batch context).
   - Run the same `segment_io` + sequential ReadSync loop, but target GPU DMA sub-views.
   - After all chunks land on GPU, also copy the assembled data to a host staging buffer and update dispatch map (memory-tier promotion for subsequent lookups).

4. **Modify dispatcher lookup to accept pre-pinned buffer** (new approach vs previous experiment)
   - Instead of calling `prepare_memory_for_spdk` inside the dispatcher per lookup, the service.rs layer pre-pins at batch start and passes the buffer.
   - Key difference from h8-v0-vs-p2p: setup cost is amortized over N lookups in the batch.

5. **Add `base64` dependency** (`components/dispatcher/v0/Cargo.toml`) — for encoding the IPC payload.

### Condition C: P2P persistent staging with promotion to staging buffer

Same as Condition B but adds explicit host DRAM copy after the P2P read completes. The data is memcpy'd from a host bounce buffer (filled alongside the GPU read) into the dispatch map's staging buffer. This ensures subsequent lookups hit the staging tier (DRAM) rather than re-reading from SSD.

Note: Conditions B and C may be combined into a single implementation where the P2P read also fills a host buffer. The distinction is analytical: does the promotion copy negate the P2P advantage?

## Success Criteria

- **Primary**: P2P-pinned SSD-tier latency is consistently lower than bounce SSD-tier latency across all 20 measurement iterations, with the P2P-pinned path being faster (lower us/obj).
- **Direction**: P2P-pinned avg latency < bounce avg latency (testing the hypothesis that pre-pinning eliminates the setup overhead that caused P2P to lose in the previous experiment).
- **Magnitude context**: Prior standalone p2p-server data showed P2P-warm is ~1.66x faster than bounce. With sequential submission, the expected advantage is smaller (maybe 1.1-1.3x) because NVMe reads dominate and can't overlap.

## Constraints

- All benchmarks must run through certus-server (no standalone binaries).
- Do NOT use or modify gpu-p2p-server.
- P2P implementation goes in components/dispatcher/v0/.
- Same NVMe device (0000:63:00.0) for both conditions — avoid 0000:62:00.0 (VFIO issues).
- nvidia_peermem and gdrdrv kernel modules must be loaded.
- SPDK singleton: only one process per NVMe device; kill server between conditions.
- Lock file cleanup: `rm -f /var/tmp/spdk_pci_lock_*` before starting.
- Debug build is acceptable (PCIe DMA latency dominates).

## Prior Knowledge

From h8-v0-vs-p2p iteration 1:
- Bounce: 13763.8 us/obj (0.30 GB/s) for 4 MiB SSD-tier
- P2P (per-lookup setup): 18371.7 us/obj (0.23 GB/s) — 33% slower due to per-call prepare_memory_for_spdk overhead
- The ~4-5ms overhead per lookup = cudaIpcOpenMemHandle + spdk_mem_register + registered_regions map tracking
- Staging-tier latency is identical between paths (confirming the overhead is specific to the SSD-tier P2P path)
- Sub-buffer DMA views (DmaBuffer::from_raw at offset with noop_free) work correctly with SPDK DMA

From h8-transfer-path experiments (standalone p2p-server):
- P2P-warm (pre-pinned) is 1.66x faster than bounce for bulk transfer
- The key variable is whether setup cost can be amortized
