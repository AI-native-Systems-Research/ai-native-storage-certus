# Handoff: Pipelined Bounce with Channel Reuse (Iteration 2)

## Goal

Fix the per-chunk `connect_client()` overhead that limited iter-1's pipelined bounce to only 17% improvement. Rewrite the pipeline to use a single pre-connected channel for all ReadAsync commands, then measure whether the resulting throughput approaches P2P warm levels.

## Key Discoveries

- **Per-chunk connect_client() was the dominant overhead in iter-1.** `issue_single_read()` (iter-1 patch line 95-118) calls `connect_client()` for every chunk. Each call allocates 2 SPSC channels (capacity 64 each), registers with the actor via a control message, and does an atomic increment — measured at 13-17μs. With 32 chunks: 416-544μs added to read phase (1204μs observed vs 649μs with BatchSubmit).
- **Individual ReadAsync on shared channel works.** `ClientChannels` uses `Sender<Command>` and `Receiver<Completion>`. You can send multiple `ReadAsync` commands sequentially on the same channel before receiving completions. The actor processes them independently (actor.rs:467-509), producing one `Completion::ReadDone` per op. Channel capacity is 64 (`lib.rs:67`), well above 32 chunks.
- **BatchSubmit has no special optimization over sequential sends.** `BatchSubmit` (actor.rs:636-651) just loops `dispatch_command` for each op. The only difference: it pre-selects a queue pair based on batch_size (line 638), while individual sends get per-op selection based on `pending_ops.len()` (actor.rs:488). This may or may not affect NVMe performance.
- **cudaMemcpyAsync is confirmed working with SPDK buffers.** Iter-1 proved SPDK hugepage DMA buffers satisfy CUDA's pinned-memory requirement (cudaHostRegister succeeds, copy_us=112μs for 32 async dispatches vs 826μs synchronous). No need to re-verify.
- **Single CUDA stream confirmed sufficient.** Iter-1's h-robustness showed 1-stream vs 2-stream: 1764 vs 1774 MB/s (0.6% difference). The copy engine is not the bottleneck.
- **Build validated:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server` exits 0.

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`
- **Run baseline:**
  ```bash
  bash .nous/h8-evolve-v0/runs/iter-2/inputs/run_condition.sh \
    bounce results/sequential-bounce.txt 0000:62:00.0
  ```
- **Output format:** Server response: `OK <size> bytes (<mode>, <chunks> chunks) read_us=N copy_us=N total_us=N`. Client reports: Throughput (MB/s), Avg/Min/Max latency (ms).
- **Baseline result (iter-1):** Sequential bounce: 1440 MB/s, 2.78ms. Pipelined (per-chunk connect): 1764 MB/s, 2.27ms. P2P warm: 3082 MB/s, 1.30ms.
- **Runtime env:** `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64`

## Code Map

| Location | What | When to look |
|----------|------|-------------|
| `components/gpu-services/v0/src/bin/p2p_server.rs:28-36` | `TransferMode` enum | Adding `BouncePipelinedSync` variant |
| `components/gpu-services/v0/src/bin/p2p_server.rs:272-322` | `do_chunked_read` | Reference for single-connect + BatchSubmit pattern |
| `components/gpu-services/v0/src/bin/p2p_server.rs:280-284` | `connect_client()` call in do_chunked_read | The pattern to replicate: query IBlockDevice, connect once |
| `components/gpu-services/v0/src/cuda_ffi.rs:71+` | CUDA FFI (already has cudaMemcpyAsync, streams from iter-1 patch) | Verify async FFI still present after git checkout |
| `components/block-device-spdk-nvme/v1/src/lib.rs:67` | `CLIENT_CHANNEL_CAPACITY = 64` | Verify channel won't block with 32 sends |
| `components/block-device-spdk-nvme/v1/src/lib.rs:374-413` | `connect_client()` implementation | Understanding what the 13-17μs overhead consists of |
| `components/block-device-spdk-nvme/v1/src/actor.rs:636-651` | `BatchSubmit` handler | Confirm it just loops dispatch_command |
| `components/block-device-spdk-nvme/v1/src/actor.rs:488` | Queue pair selection for ReadAsync | `select_index(pending_ops.len() + 1)` — differs from BatchSubmit's `select_index(batch_size)` |
| `components/interfaces/src/iblock_device.rs:373-377` | `ClientChannels` struct | `command_tx: Sender<Command>`, `completion_rx: Receiver<Completion>` |
| `components/gpu-services/v0/tests/gpu_client_p2p.py` | Benchmark client | Parses server response, reports throughput/latency |

## Code Targets

### h-main: Rewrite pipelined bounce to reuse single channel

**File:** `components/gpu-services/v0/src/bin/p2p_server.rs`

The iter-1 patch added `handle_bounce_pipelined` and `issue_single_read`. The fix rewrites `handle_bounce_pipelined` to:
1. Call `connect_client()` once (copy the pattern from `do_chunked_read` lines 280-284).
2. Replace `issue_single_read()` calls with `channels.command_tx.send(Command::ReadAsync{...})`.
3. Wait for completions via `channels.completion_rx.recv()`.
4. Keep double-buffer structure, cudaHostRegister, cudaMemcpyAsync, and timing instrumentation.

**Pipeline algorithm (corrected from iter-1):**
```
allocate buf_a, buf_b (DMA buffers)
cudaHostRegister both buffers
create 1 CUDA stream
connect_client() once → channels

send ReadAsync chunk[0] into buf_a via channels.command_tx
for i in 1..num_chunks:
    recv completion from channels.completion_rx
    cudaMemcpyAsync from completed buf to GPU, on stream
    swap active buffer
    send ReadAsync chunk[i] into other buf via channels.command_tx
recv last completion
cudaMemcpyAsync last chunk
cudaStreamSynchronize(stream)
```

**Why this location:** The `handle_bounce_pipelined` function from iter-1 is the direct target. The `issue_single_read` helper can be removed entirely since we no longer need per-call channel creation.

### h-ablation: Sync-copy pipeline with channel reuse

**File:** `components/gpu-services/v0/src/bin/p2p_server.rs`

Add `BouncePipelinedSync` variant to `TransferMode`. Implement `handle_bounce_pipelined_sync` — identical to h-main's version but replace `cudaMemcpyAsync` with `cudaMemcpy` (no stream, no cudaStreamSynchronize needed). The double-buffer + single-channel + per-chunk reads remain.

**Why:** Isolates the contribution of async overlap. If total_us ≈ read_us + copy_us in this mode but total_us ≈ max(read_us, copy_us) in h-main, that proves async overlap is the source of speedup.

## What I Tried That Didn't Work

- **Iter-1: Per-chunk issue_single_read with connect_client().** Created 32 separate client sessions. Each connect_client() adds ~17μs overhead (SPSC channel allocation + atomic increment + actor control message). Total overhead: 544μs, inflating read phase from 649μs to 1204μs.
- **Iter-1: 2-stream CUDA variant.** No benefit (0.6% difference). GPU copy engine is not the bottleneck.
- **Looked for an existing "send multiple ReadAsync on one channel" example in the codebase.** None exists — all usage is BatchSubmit or single-shot. But the channel API (Sender<Command>) clearly supports it, and BatchSubmit internally dispatches them individually.

## What I Excluded and Why

- **Multi-stream variants:** Iter-1 RP-4 (high confidence) proved single stream is not a bottleneck.
- **Chunk size variation:** Campaign spec fixes at 128 KiB.
- **BatchSubmit-based pipelining (send all reads at once, recv one-by-one):** This would work but removes the double-buffer advantage — you'd need 32 DMA buffers allocated simultaneously (like sequential bounce already does). The per-chunk send approach uses only 2 buffers.
- **Larger transfer sizes:** Campaign spec fixes at 4 MiB.
- **Queue pair selection optimization:** The actor uses `select_index(pending_ops.len() + 1)` for individual sends vs `select_index(batch_size)` for BatchSubmit. This might cause different NVMe queue utilization, but the effect should be small (both result in the same total pending ops).

## Evolution of Thinking

Iter-1 correctly identified that pipelining direction works but got the magnitude wrong. The diagnostic clearly pointed to per-chunk connect_client() as the root cause (read_us=1204 vs expected ~650). The fix is straightforward: reuse the channel.

The key architectural insight: `ClientChannels` is a simple channel pair (Sender + Receiver). There's nothing preventing sequential sends of individual commands. The confusion in iter-1 came from treating `ReadAsync` as requiring its own channel — but that's only because the iter-1 designer created `issue_single_read` as a helper that encapsulated both connection and submission.

The new uncertainty is whether the actor's queue pair selection (`pending_ops.len()` for individual sends) causes different NVMe scheduling behavior vs BatchSubmit's `batch_size` pre-selection. If it does, read_us might not fully match BatchSubmit's 649μs, but the overhead should be NVMe-queue-level (nanoseconds), not channel-allocation-level (microseconds).

## Current Status

- **Validated:** Build works. Channel API supports multiple sends. Actor processes individual ReadAsync identically to BatchSubmit-dispatched ones. cudaMemcpyAsync confirmed working on SPDK buffers.
- **Uncertain:** Whether actor queue pair selection difference affects per-chunk NVMe latency. Whether receiving completions one-by-one has different latency profile than BatchSubmit (which dispatches all then receives all). Whether client-side socket IPC becomes the new bottleneck at sub-ms server times.
- **Suggested next:** If h-main confirms channel reuse fixes the overhead (total_us < 900μs), iteration 3 should investigate whether the remaining gap to P2P warm (predicted ~750μs vs P2P's ~650μs) is inherent to the bounce-buffer architecture (extra PCIe hop) or addressable. If read_us still doesn't match BatchSubmit despite channel reuse, investigate actor polling/scheduling differences.

## Warnings & Constraints

- **Worktree starts from clean state.** The iter-1 patch (cuda_ffi additions, pipelined modes) must be re-applied or recreated. The executor should re-add: cudaStream_t, cudaMemcpyAsync, cudaStreamCreate/Synchronize/Destroy FFI, TransferMode variants, timing instrumentation in handle_bounce/handle_p2p, and the main() routing.
- **Don't use issue_single_read().** It's the iter-1 anti-pattern. Delete or ignore it. The correct pattern is `do_chunked_read`'s approach: query+connect once, then operate on the channel.
- **Channel ordering.** Completions arrive in submission order for a single channel/client (SPSC). The pipeline can safely assume recv() returns the oldest pending completion.
- **Double-buffer lock contention.** The pipeline alternates between buf_a and buf_b. Since NVMe read (via command_tx.send) and cudaMemcpyAsync (read from buffer) never access the same buffer simultaneously (we wait for read completion before copying), the Mutex<DmaBuffer> lock is uncontested. But you must drop the lock guard before sending the next read to avoid holding it across the send.
- **MDTS 128 KiB.** Do not increase chunk_size.
- **NVMe PCI: 0000:62:00.0.** Only one SPDK process at a time.
- **Build: `RUSTFLAGS='-L /usr/local/lib'`** for libgdrapi.
- **5s server startup.** The run_condition.sh sleeps 5s; don't reduce.
- **Response parsing:** Client checks `resp.startswith("OK")`. Timing fields appended after chunks info.
