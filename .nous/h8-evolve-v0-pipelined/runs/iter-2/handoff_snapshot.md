# Handoff — h8-evolve-v0-pipelined Iteration 2

## Goal

Evolve dispatcher v0's pipelined read path from QD=1 (sequential ReadSync) to QD=32 (BatchSubmit of ReadAsync × 32), measuring the speedup from NVMe flash parallelism. Compare with and without GPU copy overlap to isolate the dominant improvement mechanism. All benchmarks through certus-server with `--dispatcher-version v0`.

## Key Discoveries

1. **BatchSubmit implementation** (`block-device-spdk-nvme/v2/src/actor.rs:750-768`): Takes `Vec<Command>`, selects one queue pair via `controller.qpairs.select_index(batch_size)`, and dispatches all sub-commands to that qpair without blocking. This gives true NVMe queue depth.

2. **ReadAsync inside BatchSubmit** (`actor.rs:568-651`): Each ReadAsync calls `spdk_nvme_ns_cmd_read` (non-blocking SPDK NVMe submit at line 623), inserts into pending_ops, then `qp.submit()` (line 649). The NVMe controller sees all reads simultaneously. Completions arrive as individual `Completion::ReadDone { handle, result }` on the callback channel — the `OpHandle` identifies which read completed.

3. **Iter-1 pipeline was QD=1**: The iter-1 patch used ReadSync per chunk (one at a time). The 2x speedup (19,502 → 9,659 us) came from eliminating 32× DmaBuffer::new allocations, NOT from NVMe parallelism. The NVMe time was still ~9,600 us (32 × ~300 us sequential reads).

4. **QD=32 proven viable in h8-v1-pinned**: P2P+BatchSubmit achieved 777 us for 4 MiB (27.3x over sequential). That used GPU BAR1 DMA targets. For host-bounce with cudaHostAlloc, add ~200 us for H2D copy → expect ~1,000-1,200 us total.

5. **ENOMEM risk is low for cudaHostAlloc buffers**: The h8-v1-pinned ENOMEM failure was specific to v1's ring buffer approach (multiple SGL entries per command). Individual 128 KiB cudaHostAlloc allocations should be physically contiguous within the pinned region → single SGL entry per read → no SGL exhaustion.

6. **32 buffers needed**: Unlike iter-1's double-buffer approach (2 buffers for QD=1 pipeline), BatchSubmit QD=32 requires all 32 read targets to exist simultaneously. 32 × 128 KiB = 4 MiB of cudaHostAlloc — trivial memory cost.

7. **Out-of-order completions possible**: ReadAsync completions may not arrive in LBA order. The implementation must track which buffer corresponds to which segment offset. Use OpHandle-to-segment mapping or pre-assign each buffer to a fixed segment index.

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
  Validated: exits 0 in 0.17s (already compiled).

- **Run server:**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /var/tmp/spdk_pci_lock_0000:64:00.0 && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/certus-server \
    --metadata-pci 0000:63:00.0 \
    --data-pci 0000:64:00.0 \
    --dispatcher-version v0 \
    --listen 0.0.0.0:50051
  ```

- **Run client benchmark:**
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

- **Output format:** Stdout table. Parse "SSD-tier" row: `Avg (us/obj)`, `Avg (GB/s)`.
- **Baseline results:**
  - Sequential v0 (unpatched): 19,501.8 us/obj (0.22 GB/s) [iter-1]
  - Pipelined v0 (iter-1 patch, QD=1): 9,659.3 us/obj (0.43 GB/s) [iter-1]

## Code Map

| Location | What | When to look |
|----------|------|-------------|
| `components/dispatcher/v0/src/lib.rs:179-276` | `read_from_block_device` — sequential read path | Primary modification target |
| `components/dispatcher/v0/src/lib.rs:694-747` | `IDispatcher::lookup` — routes to read path | Verify routing unchanged |
| `components/dispatcher/v0/src/io_segmenter.rs:22-55` | `segment_io` — generates chunk LBAs/offsets | Used by both arms |
| `components/block-device-spdk-nvme/v2/src/actor.rs:750-768` | BatchSubmit dispatch | Verify batch behavior |
| `components/block-device-spdk-nvme/v2/src/actor.rs:568-651` | ReadAsync — non-blocking NVMe submit | Understand completion flow |
| `components/gpu-services/v0/src/cuda_ffi.rs:71-111` | CUDA FFI declarations | Add stream/async symbols |
| `components/gpu-services/v0/src/dma.rs:253-288` | `create_spdk_dma_buffer_from_cuda_host_alloc` | Creates dual CUDA+SPDK buffer |
| `components/gpu-services/v0/src/lib.rs:489-537` | `dma_copy_to_device` sync impl | Reference for cudaMemcpy call |
| `components/interfaces/src/iblock_device.rs:204-214` | `Command::ReadAsync` struct | Fields: ns_id, lba, buf, timeout_ms |
| `components/interfaces/src/iblock_device.rs:235-239` | `Command::BatchSubmit` struct | Fields: ops: Vec<Command> |
| `components/interfaces/src/iblock_device.rs:291-297` | `Completion::ReadDone` | Fields: handle (OpHandle), result |
| `components/interfaces/src/spdk_types.rs:238-263` | `DmaBuffer::new` — spdk_dma_zmalloc alloc | Why per-chunk alloc is slow |
| `components/interfaces/src/spdk_types.rs:293-316` | `DmaBuffer::from_raw` | For noop_free wrapper buffers |
| `.nous/h8-evolve-v0-pipelined/runs/iter-1/patches/h-main.patch` | Iter-1 pipelined patch (reference) | Serves as baseline for this iteration |

## Code Targets

### h-main: BatchSubmit pipelined (QD=32 + async GPU copies)

**File:** `components/gpu-services/v0/src/cuda_ffi.rs` (after line 111, inside extern "C")
- Add: `cudaStream_t` type, `cudaStreamCreate`, `cudaStreamDestroy`, `cudaStreamSynchronize`, `cudaMemcpyAsync`
- **Why here:** Single-point CUDA FFI module. Same additions as iter-1 (validated).

**File:** `components/dispatcher/v0/Cargo.toml` (line 12)
- Change: `gpu-services = { workspace = true, features = ["spdk"] }` → `gpu-services = { workspace = true, features = ["gpu", "spdk"] }`
- **Why:** Enables access to cuda_ffi and dma modules.

**File:** `components/dispatcher/v0/src/lib.rs` (replace `read_from_block_device`, lines 179-276)
- Add `BatchPipelineState` struct: 32 DmaBuffers (cudaHostAlloc), 1 CUDA stream, lazily initialized.
- New `read_from_block_device` algorithm:
  1. `ensure_batch_pipeline_state()` — lazy-init 32 × 128 KiB cudaHostAlloc buffers + CUDA stream
  2. `connect_client()` once
  3. Build `Vec<Command>` with 32 `ReadAsync { ns_id: 1, lba: seg.lba, buf: Arc::new(Mutex::new(wrapper)), timeout_ms: 5000 }`
  4. Send single `Command::BatchSubmit { ops }`
  5. Loop `completion_rx.recv()` × 32: for each `ReadDone { handle, result }`, look up which segment index this handle corresponds to, then `cudaMemcpyAsync(gpu_base + seg.buffer_offset, bufs[idx].as_ptr(), seg.length, H2D, stream)`
  6. `cudaStreamSynchronize(stream)`
- **Key detail — handle tracking:** Before sending BatchSubmit, record `handle_counter → segment_index` mapping. The actor assigns sequential handles starting from its current `next_handle` value. Since we're the only client on this connection, handles 0..31 map to segments 0..31 in submission order. OR simpler: assign buffer[i] to segment[i], and since ReadAsync completions include the handle, track handle→buffer_index.
  
  Actually simpler: since we control which buffer goes to which ReadAsync, and the DmaBuffer pointer is unique per buffer, we can map `buf pointer → segment index`. But the simplest approach: submit segments in order, get handles in order (actor assigns monotonically), track `first_handle` before send, then `handle - first_handle = segment_index`.
  
  **Simplest safe approach:** Since we own the connection exclusively and the actor's `next_handle` increments sequentially for each op in the batch (actor.rs:575: `let handle = *next_handle; *next_handle += 1`), the first ReadAsync gets handle N, second gets N+1, etc. We don't know N, but we know the ORDER. So: just collect all 32 completions, and for each, determine which buffer it read into. Since `buf: Arc<Mutex<DmaBuffer>>` is cloned into the command, and we keep our own clone, we can just copy from buffer[i] when we see the i-th completion... but we don't know which completion corresponds to which buffer.

  **Best approach:** Pre-assign buffer[i] to segment[i]. After all 32 completions received (verifying all succeeded), copy all 32 buffers to GPU in order:
  ```
  for i in 0..32:
    cudaMemcpyAsync(gpu + seg[i].offset, buf[i].ptr, seg[i].len, H2D, stream)
  cudaStreamSynchronize(stream)
  ```
  This doesn't overlap GPU copies with NVMe reads but avoids the handle-tracking complexity. The GPU copies (32 × 128 KiB async) take ~200 us total — negligible vs NVMe savings.

  **For maximum overlap:** Issue cudaMemcpyAsync immediately on each ReadDone. Need handle→buffer mapping. Since the actor assigns handles sequentially within BatchSubmit (line 575), and we submitted segments 0..31 in that order, handle values for our batch are consecutive. Store the first handle from the first ReadDone completion, then `idx = handle.0 - first_handle`.

- **Why this location:** `read_from_block_device` is the only code path for SSD-tier lookups in v0.

### h-ablation: BatchSubmit + synchronous copy (QD=32, no overlap)

**Same files as h-main** with one difference in `lib.rs`:
- After collecting all 32 ReadDone completions, instead of per-chunk cudaMemcpyAsync, copy all 32 buffers into one contiguous staging buffer (ptr::copy_nonoverlapping), then single `cudaMemcpy` of 4 MiB to GPU.
- No CUDA stream needed (but include for build compat).
- **Why:** Isolates whether the speedup comes from QD=32 NVMe parallelism (hypothesis: yes) or from async GPU copy overlap (hypothesis: negligible contribution).

## What I Tried That Didn't Work

- **P2P direct DMA (iter-1):** 3% slower than sequential. BAR1 write bandwidth bottleneck at NODE level. Dead end — do not revisit.
- **Double-buffer QD=1 pipeline (iter-1):** Achieved 2x but NVMe QD=1 remains the bottleneck for the remaining 9,659 us.
- **Bounce+Batch in h8-v1-pinned:** ENOMEM at QD=4. That was specific to ring buffer SGL fragmentation — does NOT apply to individually-allocated cudaHostAlloc buffers.

## What I Excluded and Why

- **P2P path:** Definitively ruled out in iter-1 (RP-10). Not worth re-testing.
- **Components/dispatcher/v1/**: Campaign constraint forbids referencing.
- **Triple/quad buffering with ReadSync:** Iter-1 proved QD=1 doesn't exploit NVMe parallelism. Adding more buffers without changing to BatchSubmit doesn't help.
- **Larger chunk sizes:** MDTS constraint (128 KiB max transfer). Campaign spec fixes this.
- **Multi-stream CUDA:** h8-pipelined proved 1 stream ≈ 2 streams (0.6% difference).
- **Variable object sizes in this iteration:** Focus on 4 MiB (32 chunks) to match iter-1 and isolate the QD effect.
- **Control-negative (4 KiB):** BatchSubmit with 1 read = just ReadAsync — no meaningful difference from ReadSync. Iter-1 already measured single-chunk improvements (16% from allocation avoidance). No new information.

## Evolution of Thinking

1. Iter-1 showed 2x speedup but revealed the remaining bottleneck is 32 sequential NVMe reads (~300 us each = 9,600 us total). The GPU copy overlap was secondary — the DmaBuffer::new elimination was dominant.

2. The natural next step is QD=32 via BatchSubmit. The h8-v1-pinned experiment proved this mechanism achieves 777 us for 4 MiB via P2P. For host-bounce, add ~200 us H2D copy → expect ~1,000-1,200 us.

3. The ablation arm tests whether async GPU copy overlap matters at QD=32. When NVMe reads all complete in ~800 us (parallel), and GPU copy takes ~200 us, the maximum overlap saving is ~200 us. If total latency is ~1,000 us, that's a ~20% difference — worth measuring but likely secondary.

4. Key implementation insight: BatchSubmit + ReadAsync sends all commands in one message. The actor dispatches them all to one qpair and returns completions individually. The caller must handle potentially out-of-order completions (though in practice, sequential LBAs from one controller likely complete in-order).

5. Buffer management shift: From 2 ping-pong buffers (iter-1) to 32 dedicated buffers (iter-2). Each buffer is permanently assigned to one segment position. Total allocation 4 MiB — negligible. Lazy-init on first lookup, persist for component lifetime.

## Current Status

- **Validated:** Build works (0.17s). Iter-1 patch structure confirmed correct. BatchSubmit API confirmed (actor.rs:750). ReadAsync API confirmed (ns_id, lba, buf, timeout_ms). cudaHostAlloc + spdk_mem_register path proven (iter-1). PCI devices 63:00.0 and 64:00.0 both present.
- **Uncertain:** (1) Whether 32 concurrent cudaHostAlloc buffers registered with SPDK cause SGL issues at QD=32 (low risk — different from v1 ring buffer problem). (2) Whether ReadAsync completions arrive strictly in submission order (affects copy-on-completion implementation). (3) Exact per-read latency at QD=32 through certus-server path (gRPC overhead, actor dispatch time).
- **Suggested next (iter-3):** If BatchSubmit achieves ~1,000-1,500 us: remaining overhead is connect_client + gRPC + completion collection. Explore persistent connection pooling or pre-connected channels. If BatchSubmit fails (ENOMEM): fall back to partial batching (e.g., 4 batches of 8, QD=8). If ablation shows sync copy is equivalent: simplify the code path (remove async complexity).

## Warnings & Constraints

- **SPDK singleton:** Only one process per NVMe device. Kill certus-server and clean lock files before restart.
- **Lock file cleanup:** Always `rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /var/tmp/spdk_pci_lock_0000:64:00.0` before starting.
- **RUSTFLAGS required:** `-L /usr/local/lib` for libgdrapi.so linkage.
- **LD_LIBRARY_PATH:** Must include `/usr/local/lib:/usr/local/cuda/lib64` at runtime.
- **ReadAsync timeout:** Set to 5000 ms (5s) — generous timeout to avoid spurious failures. The actual reads should complete in <1 ms at QD=32.
- **Debug build OK:** PCIe DMA latency dominates. CPU-bound code (~ns) is negligible vs DMA (~μs).
- **Buffer lifetime:** The 32 cudaHostAlloc DmaBuffers must outlive all ReadAsync operations. Store in component field (Mutex<Option<BatchPipelineState>>), lazily initialized, dropped only on shutdown.
- **Handle monotonicity:** The actor assigns handles sequentially via `*next_handle += 1` (actor.rs:575). Within a single BatchSubmit from a single client, handles are guaranteed consecutive. Use this for completion→segment mapping.
- **Per-chunk DmaBuffer wrapper uses noop_free:** The Arc<Mutex<DmaBuffer>> wrappers for ReadAsync must NOT free the underlying cudaHostAlloc memory. Use `DmaBuffer::from_raw(ptr, len, noop_free, -1)` — same pattern as iter-1.
- **Server wait time:** `sleep 10` after server start to allow SPDK init + device attach before client connects.
- **Python client pool assumption:** `bench_lookup_latency` writes objects to staging, background writer moves to SSD (~3s wait built into client), then measures SSD-tier lookup. This path is unchanged.
