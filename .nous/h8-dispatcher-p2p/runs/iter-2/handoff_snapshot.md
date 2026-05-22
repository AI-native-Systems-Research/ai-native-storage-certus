# Handoff — h8-dispatcher-p2p, Iteration 2

## Goal

Decompose the P2P advantage into two independent factors — **path effect** (NVMe→GPU BAR1 vs NVMe→host DRAM→GPU) and **submission strategy effect** (BatchSubmit concurrent vs sequential ReadSync) — by adding sequential variants of both P2P and bounce to the `gpu-p2p-server` binary. This determines whether the dispatcher can benefit from P2P without also needing to switch from sequential ReadSync to BatchSubmit.

## Key Discoveries

1. **Current probe results (2026-05-19)**: Bounce 2206 MB/s / 1.81ms, P2P warm 3670 MB/s / 1.09ms. Ratio is 1.66x (lower than iter-1's 2.47x — likely NVMe controller warmth or scheduling variance). Re-running with 20 iterations per seed should produce stable numbers.
2. **The dispatcher's pipeline.rs (line 60-119) uses sequential ReadSync** — one chunk at a time, wait for completion, copy, next. This is strictly worse than BatchSubmit. The experiment must isolate whether the path alone (P2P with same serial submission) helps.
3. **`prepare_memory_for_spdk` (interfaces:461) is available to the dispatcher** via the `IGpuServices` receptacle. It takes a base64-encoded IPC handle and returns a DMA buffer backed by GPU BAR1. This is the API the dispatcher would use for P2P integration.
4. **The p2p_server already has pre-pinned chunk pool infrastructure** (`ChunkPool` struct, `create_chunk_pool` function at p2p_server.rs:230-268). Adding sequential variants reuses this pool — only the read submission loop changes.
5. **No code changes needed for Conditions A and B** (BatchSubmit bounce and P2P warm). Only Conditions C (P2P-seq) and D (bounce-seq) require new `TransferMode` variants and handler functions.
6. **Device 0000:63:00.0 confirmed working** (iter-2 probes succeeded). Device 0000:62:00.0 still has VFIO group 14 busy issues.
7. **Both A30 GPUs and NVMe devices remain on NUMA 0** (no topology change since iter-1).

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server
  ```
- **Run baseline (bounce, BatchSubmit):**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /tmp/gpu_p2p_bench.sock && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/gpu-p2p-server --socket /tmp/gpu_p2p_bench.sock --pci 0000:63:00.0 --mode bounce --chunk-size 131072 --staging-size 4194304
  ```
- **Run client:**
  ```bash
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_bench.sock --iterations 20
  ```
- **Output format:** Client prints to stderr: `Throughput: X MB/s`, `Avg latency: X ms`, `Min latency: X ms`, `Max latency: X ms`. Parse these.
- **Baseline result:** Bounce 2206 MB/s / 1.81ms avg, P2P warm 3670 MB/s / 1.09ms avg (5 iterations each, probe run).

## Code Map

| File:Line | What's there | When to look |
|-----------|-------------|--------------|
| `components/gpu-services/v0/src/bin/p2p_server.rs:28-36` | TransferMode enum (bounce/p2p/p2p-cold) | Adding new P2pSeq and BounceSeq variants |
| `components/gpu-services/v0/src/bin/p2p_server.rs:374-433` | `handle_bounce`: BatchSubmit NVMe→host→cudaMemcpy H2D | Reference for handle_bounce_seq |
| `components/gpu-services/v0/src/bin/p2p_server.rs:436-490` | `handle_p2p`: BatchSubmit NVMe→GPU staging→D2D copy | Reference for handle_p2p_seq |
| `components/gpu-services/v0/src/bin/p2p_server.rs:271-323` | `do_chunked_read`: BatchSubmit of async reads | Understand concurrent submission |
| `components/gpu-services/v0/src/bin/p2p_server.rs:230-268` | `create_chunk_pool`: pre-allocate GPU staging buffers | Reused by P2P-seq handler |
| `components/dispatcher/v1/src/pipeline.rs:60-119` | Sequential ReadSync loop (the pattern to replicate) | Model for sequential handlers |
| `components/dispatcher/v1/src/lib.rs:190-266` | `promote_and_serve`: full orchestration | Context for future dispatcher integration |
| `components/interfaces/src/igpu_services.rs:461-463` | `prepare_memory_for_spdk` signature | If implementing dispatcher-level P2P later |
| `components/gpu-services/v0/tests/gpu_client_p2p.py:44-61` | `do_transfer`: socket send/recv timing | If latency numbers seem wrong |

## Code Targets

### New TransferMode variants (p2p_server.rs:28-36)
Add `P2pSeq` and `BounceSeq` to the `TransferMode` enum with `#[value(name = "p2p-seq")]` and `#[value(name = "bounce-seq")]` aliases.

### handle_p2p_seq (new function, after handle_p2p at ~line 490)
- Takes same args as `handle_p2p`: `stream`, `ctx`, `pool`
- Instead of calling `do_chunked_read` (BatchSubmit), issues sequential ReadSync per chunk:
  ```
  for each chunk in pool.buffers[..num_chunks]:
      channels.command_tx.send(Command::ReadSync { ns_id, lba, buf: chunk.dma_buf })
      channels.completion_rx.recv() → verify ReadDone
  ```
- After all reads complete, D2D copy per chunk (same as handle_p2p lines 467-485)
- **Why this location**: Directly follows handle_p2p, shares the same ChunkPool infrastructure

### handle_bounce_seq (new function, after handle_bounce at ~line 433)
- Takes same args as `handle_bounce`: `stream`, `ctx`
- Allocates host DMA buffers (same as handle_bounce lines 377-395)
- Instead of calling `do_chunked_read`, issues sequential ReadSync per chunk
- After all reads complete, sequential cudaMemcpy H2D per chunk (same as handle_bounce lines 406-427)
- **Why this location**: Directly follows handle_bounce, mirrors its allocation pattern

### Main dispatch match (wherever handle_bounce/handle_p2p are dispatched)
Add arms for the new modes routing to the new handlers. The `ChunkPool` is already allocated at startup for P2P modes — extend the pool allocation condition to include P2pSeq.

## What I Tried That Didn't Work

- **Device 0000:62:00.0:** Still busy (VFIO group 14). Confirmed by iter-1. Use 0000:63:00.0.
- **Running server with `--once`:** Exits after warmup transfer, fails multi-iteration benchmark. Don't use `--once`.
- **Stale SPDK locks:** `/var/tmp/spdk_pci_lock_*` from prior sessions block init. Always clear before starting.
- **Expecting stable absolute numbers across sessions:** Bounce went from 1469 MB/s (iter-1) to 2206 MB/s (iter-2 probe). The ratio is more stable than absolutes. Always measure all conditions in the same session.

## What I Excluded and Why

- **Full dispatcher integration (calling promote_and_serve_p2p):** Requires adding DispatcherConfig field, modifying lib.rs dispatch logic, and creating a test harness that populates→evicts→lookups through the full dispatcher. This is a large code change better suited for iteration 3 after we know P2P-seq is worthwhile. If P2P-seq is NOT faster than bounce-seq, then dispatcher integration isn't worth pursuing.
- **P2P cold sequential:** Already established in iter-1 that cold P2P is 2.74x slower than bounce. Adding sequential variant of an already-slow path adds no information.
- **Larger transfer sizes:** Research question specifies 4 MiB. Keeping consistent with iter-1 for valid comparison.
- **64 KiB chunk robustness arm:** Already tested in iter-1 (RP-3). This iteration's focus is the path/submission decomposition.

## Evolution of Thinking

1. **Initial plan:** Implement P2P directly in dispatcher's `promote_and_serve` and benchmark full dispatcher lookup. This is what iter-1's handoff suggested.
2. **Revised approach:** Before investing in that large code change, first determine whether P2P with sequential submission (matching the dispatcher's pattern) is actually faster. If the submission strategy is the dominant factor and P2P-seq isn't better than bounce-seq, then the dispatcher needs BatchSubmit (a bigger change) rather than just P2P.
3. **Decomposition insight:** The iter-1 results conflate two improvements: (a) P2P path (1 hop vs 2) and (b) BatchSubmit (concurrent vs sequential). The dispatcher currently has neither. We need to know which one matters more before choosing what to implement first.
4. **Probe observation:** Bounce throughput varies between sessions (1469→2206 MB/s). Ratios are more reliable. Always measure all conditions in same session.

## Current Status

- **Validated:** Build command works, p2p_server starts and accepts connections, both modes produce correct results on device 0000:63:00.0, client reports clean metrics.
- **Uncertain:** Whether P2P-sequential will beat bounce-sequential. The per-chunk BAR1 write latency might be higher than host DRAM DMA (BAR1 writes are uncacheable, may have higher per-TLP overhead). This is the key question.
- **Suggested next:**
  - If P2P-seq >> bounce-seq: implement P2P in dispatcher's pipeline.rs (keep sequential submission), achieving immediate latency improvement.
  - If P2P-seq ≈ bounce-seq but P2P-batch >> bounce-batch: the submission strategy dominates. Implement BatchSubmit in dispatcher first (applicable to both paths), then optionally add P2P.
  - If both factors contribute significantly: implement both (BatchSubmit + P2P) in dispatcher for iteration 3.

## Warnings & Constraints

- **SPDK singleton:** Only one SPDK process per device. Kill previous server before starting new one.
- **Socket cleanup:** Always `rm -f` socket path before starting. Stale socket causes bind failure.
- **Server startup time:** 3-5 seconds (SPDK init + CUDA init + GDRCopy pool for P2P modes). Client must wait.
- **P2P modes require ChunkPool:** 32 GPU staging buffers × 128 KiB = 4 MiB GPU VRAM for P2P and P2P-seq modes.
- **Measure all conditions in same session:** Absolute numbers vary between sessions. Run conditions back-to-back for valid ratios.
- **Client output is stderr:** Parse benchmark results from stderr, not stdout.
- **Sequential ReadSync uses connect_client() per call in dispatcher:** The p2p_server should also call `connect_client()` once at handler entry (as it does now) and reuse the channels for all sequential reads. Don't reconnect per chunk.
- **Debug build is appropriate:** Dominant cost is PCIe DMA, not CPU. Release build mainly speeds command prep which is negligible at 32 chunks.
