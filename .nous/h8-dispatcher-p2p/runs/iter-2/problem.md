# Problem Framing — Iteration 2: P2P Integration in Dispatcher

## Research Question

Does integrating direct P2P (NVMe→GPU via BAR1) into the dispatcher's `promote_and_serve` path yield lower end-to-end lookup latency than the existing pipelined bounce path (SSD→DRAM→GPU), when the overhead of dispatcher infrastructure (memory-tier management, dispatch-map bookkeeping, extent manager lookup) is included?

Iteration 1 established that P2P warm is 2.47x faster than bounce in isolation (standalone `gpu-p2p-server`). This iteration tests whether that advantage survives integration into the full dispatcher data path.

Key source files:
- `components/dispatcher/v1/src/pipeline.rs:30-123` — current sequential `ReadSync` + memcpy + `dma_copy_to_device` pipeline
- `components/dispatcher/v1/src/lib.rs:190-266` — `promote_and_serve` orchestration
- `components/gpu-services/v0/src/bin/p2p_server.rs:271-323` — `do_chunked_read` BatchSubmit reference implementation
- `components/interfaces/src/igpu_services.rs:461-463` — `prepare_memory_for_spdk` interface

## System Interface

**Build command:**
```bash
RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server
```

**CLI flags (validated via `--help`):**
- `--socket <PATH>` — Unix socket path (default: `/tmp/gpu_p2p_server.sock`)
- `--pci <ADDR>` — NVMe PCI address (DDDD:BB:DD.F)
- `--mode <MODE>` — `bounce | p2p | p2p-cold` (parsed at `p2p_server.rs:28-36`)
- `--chunk-size <BYTES>` — NVMe I/O chunk size (default: 131072, `p2p_server.rs:57-58`)
- `--staging-size <BYTES>` — Pre-allocated staging buffer size (default: 4194304, `p2p_server.rs:53-54`)

**Client:**
```bash
python3 components/gpu-services/v0/tests/gpu_client_p2p.py <size_bytes> <socket_path> --iterations N
```

**Output format:** Client prints to stderr:
```
Throughput:    X MB/s
Avg latency:   X ms
Min latency:   X ms
Max latency:   X ms
```

**Code evidence for flags:**
- `--mode` enum: `p2p_server.rs:28-36` (TransferMode enum with ValueEnum derive)
- `--chunk-size`: `p2p_server.rs:57-58` (clap arg, default 131072)
- `--staging-size`: `p2p_server.rs:53-54` (clap arg, default 4194304)
- `--pci`: `p2p_server.rs:45-46` (optional, uses first device if omitted)
- Client payload format: `gpu_client_p2p.py:19-21` (64-byte IPC handle + 8-byte LE size)

## Baseline Command

```bash
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /tmp/gpu_p2p_bench.sock && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/gpu-p2p-server --socket /tmp/gpu_p2p_bench.sock --pci 0000:63:00.0 --mode bounce --chunk-size 131072 --staging-size 4194304
```

Client (in separate shell):
```bash
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_bench.sock --iterations 20
```

## Baseline Validation

Probe run (2026-05-19), 5 iterations each:
- **Bounce**: Throughput 2206 MB/s, Avg latency 1.81ms, Min 1.77ms, Max 1.85ms
- **P2P warm**: Throughput 3670 MB/s, Avg latency 1.09ms, Min 1.08ms, Max 1.10ms
- **Exit code**: 0 for both. Binary built and running successfully.

## Experimental Conditions

### Condition A: Baseline — Bounce via standalone server (existing)
The existing `gpu-p2p-server --mode bounce` path. NVMe→host DMA buffers→cudaMemcpy H2D→client GPU. Uses BatchSubmit for concurrent NVMe reads. This represents what the dispatcher's pipeline.rs *would* look like if it used BatchSubmit instead of sequential ReadSync.

**Command:**
```bash
# Server:
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /tmp/gpu_p2p_bench.sock && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/gpu-p2p-server --socket /tmp/gpu_p2p_bench.sock --pci 0000:63:00.0 --mode bounce --chunk-size 131072 --staging-size 4194304

# Client:
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_bench.sock --iterations 20
```

### Condition B: P2P warm via standalone server (existing)
The existing `gpu-p2p-server --mode p2p` path. NVMe→pre-pinned GPU staging (GDRCopy BAR1)→D2D copy→client GPU. Uses BatchSubmit for concurrent NVMe reads into BAR1-mapped buffers.

**Command:**
```bash
# Server:
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 /tmp/gpu_p2p_bench.sock && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/gpu-p2p-server --socket /tmp/gpu_p2p_bench.sock --pci 0000:63:00.0 --mode p2p --chunk-size 131072 --staging-size 4194304

# Client:
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 components/gpu-services/v0/tests/gpu_client_p2p.py 4194304 /tmp/gpu_p2p_bench.sock --iterations 20
```

### Condition C: Dispatcher-integrated P2P — `promote_and_serve_p2p` (code change)
Add a new function `p2p_ssd_to_gpu` in `pipeline.rs` that:
1. Calls `gpu.prepare_memory_for_spdk(ipc_base64, None)` to get a BAR1-backed DMA buffer covering the full GPU destination
2. Issues `BatchSubmit` with `ReadAsync` commands targeting chunk-sized offsets within that DMA buffer
3. Collects all completions
4. Skips memory-tier population (serve-only, no DRAM caching on this path)

Then modify `promote_and_serve` in `lib.rs` to call `p2p_ssd_to_gpu` when a `transfer_mode: P2P` flag is set in the config.

The server binary gains a new `--mode dispatcher-p2p` mode that creates the dispatcher component, populates a key to SSD, then performs lookup (triggering promote_and_serve_p2p) and measures the latency. Alternatively (simpler): add `--mode p2p-sequential` to the existing p2p_server that uses sequential ReadSync into BAR1 buffers (matching dispatcher's serial approach but with P2P destination), to isolate the path-vs-submission-strategy effects.

### Condition D: Sequential P2P — isolate submission strategy (code change)
Add `--mode p2p-seq` to `gpu-p2p-server` that uses sequential `ReadSync` (like the dispatcher's pipeline.rs) but targets BAR1-backed buffers instead of host DRAM. This isolates the effect of the transfer path (P2P vs bounce) from the effect of the submission strategy (BatchSubmit vs sequential ReadSync).

**Intent:** In `handle_p2p` (or new `handle_p2p_seq`), instead of calling `do_chunked_read` (BatchSubmit), issue sequential ReadSync per chunk into the pre-pinned GPU staging buffers, then D2D copy to client. This matches the dispatcher's serial pipeline pattern but uses P2P destination.

## Success Criteria

1. **Condition B (P2P warm) consistently outperforms Condition A (bounce)** across all seeds, confirming iter-1's finding persists. Expected: P2P warm 1.5-2.5x lower latency.
2. **Condition D (P2P sequential) outperforms Condition A (bounce sequential equivalent)**: If P2P path alone (without BatchSubmit) is still faster, this confirms the path advantage is the dominant factor, not the submission strategy.
3. **Condition D is slower than Condition B**: The gap between P2P-sequential and P2P-BatchSubmit quantifies the submission strategy contribution.

## Constraints

- Device `0000:63:00.0` only (0000:62:00.0 has VFIO group busy issues — from iter-1).
- Always clear `/var/tmp/spdk_pci_lock_0000:63:00.0` before starting server.
- Always `rm -f` socket path before starting server.
- Server startup takes 3-5 seconds; client must wait.
- Only one SPDK process per device at a time.
- 4 MiB transfer size, 128 KiB chunk size (matching iter-1 for comparability).
- Debug build (DMA-bound, not CPU-bound — established in iter-1).

## Prior Knowledge

- **RP-1** (high confidence): P2P warm achieves ~2.5x lower latency than bounce for 4 MiB at 128 KiB chunks in standalone server.
- **RP-2** (high confidence): GDRCopy per-request pin/unpin is 2.74x slower than bounce — pre-pinning is required.
- **RP-3** (medium confidence): Halving chunk size narrows advantage from 2.47x to 2.04x; both paths degrade at smaller chunks.

This iteration advances from standalone measurement to understanding which factor (path vs submission strategy) contributes how much, as a prerequisite for dispatcher integration.
