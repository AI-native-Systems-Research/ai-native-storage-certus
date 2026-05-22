# Problem Framing: Pipelined Bounce with Channel Reuse (Iteration 2)

## Research Question

Can pipelined bounce (NVMe reads overlapped with cudaMemcpyAsync H2D copies) match P2P warm throughput when the per-chunk `connect_client()` overhead is eliminated by reusing a single `ClientChannels` instance across all pipeline iterations?

Iteration 1 confirmed pipelining direction (17% faster than sequential) but fell far short of prediction because `issue_single_read()` calls `connect_client()` per chunk, adding ~544μs overhead to the read phase (1204μs pipelined vs 649μs BatchSubmit). The fix: send individual `ReadAsync` commands on a single pre-connected channel, matching BatchSubmit's single-connection cost while preserving per-completion pipeline progression.

Relevant source files:
- `components/gpu-services/v0/src/bin/p2p_server.rs:272-322` — `do_chunked_read` (BatchSubmit with single `connect_client()`)
- `components/gpu-services/v0/src/bin/p2p_server.rs:374-433` — `handle_bounce` (sequential baseline)
- `components/block-device-spdk-nvme/v1/src/lib.rs:374-413` — `connect_client()` implementation (SPSC channel allocation + actor registration)
- `components/block-device-spdk-nvme/v1/src/lib.rs:67` — `CLIENT_CHANNEL_CAPACITY = 64` (sufficient for 32 chunks)
- `components/block-device-spdk-nvme/v1/src/actor.rs:467-509` — `ReadAsync` dispatch (one completion per op, independent of submission method)

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`
- **CLI flags:**
  - `--mode <MODE>`: Transfer mode (`bounce`, `p2p`, `p2p-cold`, `bounce-pipelined`, `bounce-pipelined-2stream`)
    - Defined: `p2p_server.rs:28-36` (TransferMode enum)
  - `--chunk-size <BYTES>`: NVMe I/O chunk size (default 131072)
    - Defined: `p2p_server.rs:57-59`
  - `--staging-size <BYTES>`: Total transfer size for pool allocation (default 4194304)
    - Defined: `p2p_server.rs:53-55`
  - `--pci <ADDR>`: NVMe PCI address
    - Defined: `p2p_server.rs:46-48`
  - `--socket <PATH>`: Unix socket path
    - Defined: `p2p_server.rs:42-43`
  - `--once`: Serve one client then exit
    - Defined: `p2p_server.rs:62-63`
- **Output format:** Server response per request: `OK <size> bytes (<mode>, <chunks> chunks) read_us=N copy_us=N total_us=N`. Client (`gpu_client_p2p.py`) reports to stderr: Throughput (MB/s), Avg/Min/Max latency (ms).
- **Runtime env:** `LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64`
- **Code evidence:**
  - `connect_client()` creates per-client SPSC channels: `lib.rs:390-398`
  - Channel capacity 64: `lib.rs:67`
  - `ReadAsync` produces individual completions: `actor.rs:467-509`
  - `BatchSubmit` loops `dispatch_command` per op: `actor.rs:636-651`

## Baseline Command

```bash
bash .nous/h8-evolve-v0/runs/iter-2/inputs/run_condition.sh \
  bounce results/sequential-bounce.txt 0000:62:00.0
```

## Baseline Validation

Iteration 1 results (validated):
- Sequential bounce: 1440 MB/s, 2.78ms avg latency (50 iterations). Phase: read_us=649, copy_us=826, total_us=1475.
- Pipelined bounce (per-chunk connect): 1764 MB/s, 2.27ms avg. Phase: read_us=1204, copy_us=112, total_us=1627.
- P2P warm: 3082 MB/s, 1.30ms avg latency.

The run_condition.sh script, server binary, and client script are validated working from iter-1.

## Experimental Conditions

### Condition 1: h-main — Pipelined bounce with channel reuse

**Code change:** Modify `handle_bounce_pipelined` to call `connect_client()` once before the pipeline loop, then send individual `ReadAsync` commands on the same channel for each chunk. Receive completions one-by-one from the shared `completion_rx` to drive pipeline progression.

**Expected effect:** Eliminates ~544μs of per-chunk connection overhead from the read phase. Predicted read_us should drop from 1204μs to ~660-700μs (similar to BatchSubmit's 649μs plus minimal per-send overhead). With copy_us=112μs (confirmed async), predicted total_us = max(read_us, copy_us) + pipeline_drain ≈ 700-750μs.

**Command:** Same run_condition.sh with `bounce-pipelined` mode.

### Condition 2: h-control-negative — Sequential bounce (unchanged)

**No code change.** Run existing bounce mode.

**Expected:** read_us=649, copy_us=826, total_us=1475 (same as iter-1). Confirms baseline hasn't drifted.

**Command:** `bounce` mode.

### Condition 3: h-ablation — Pipelined bounce with channel reuse but synchronous cudaMemcpy

**Code change:** Same as h-main (single connect_client, per-chunk ReadAsync on shared channel) but use `cudaMemcpy` (synchronous) instead of `cudaMemcpyAsync`. This isolates the contribution of async copies: if channel reuse alone is sufficient, sync copies should still show improvement from eliminating connect_client overhead.

**Expected:** total_us ≈ read_us + copy_us (no overlap). Should be ~660 + 826 = ~1486μs — similar to sequential bounce. This proves that the speedup from h-main comes from async overlap, not just from avoiding connect_client() latency.

**Command:** New mode `bounce-pipelined-sync` or parameter flag.

### Condition 4: h-robustness — P2P warm reference

**No code change.** Run existing p2p mode.

**Expected:** 3082 MB/s, 1.30ms. Validates hardware/system state is consistent.

**Command:** `p2p` mode.

## Success Criteria

1. **h-main:** Pipelined bounce with channel reuse achieves total_us < 900μs server-side (vs iter-1's 1627μs), corresponding to >50% improvement from iter-1 pipelined and placing it within 2x of P2P warm's end-to-end latency.
2. **h-main:** read_us drops to within 20% of BatchSubmit read_us (649μs) — i.e., read_us < 780μs.
3. **h-main:** Throughput exceeds 2500 MB/s (vs iter-1's 1764 MB/s).
4. **h-ablation:** total_us with sync copies is ≥ total_us with async copies, confirming overlap is the mechanism.
5. **h-control-negative:** Sequential bounce latency within 10% of iter-1 result (2.78ms).

## Constraints

- Chunk size fixed at 128 KiB (MDTS limit).
- Transfer size fixed at 4 MiB (32 chunks).
- NVMe PCI: 0000:62:00.0 (exclusive access required).
- Build requires `RUSTFLAGS='-L /usr/local/lib'`.
- Server startup delay: 5s.
- SPSC channel capacity: 64 slots (sufficient for 32 in-flight commands).
- Cannot modify the IBlockDevice interface or actor implementation.

## Prior Knowledge

- **RP-1 (high confidence):** SPDK hugepage DMA buffers satisfy CUDA's pinned-memory requirement. cudaHostRegister succeeds on them, and cudaMemcpyAsync achieves true non-blocking operation (112μs dispatch for 32×128KiB).
- **RP-2 (high confidence):** Each `connect_client()` call costs ~13-17μs. Per-chunk pipelining with connect_client() per chunk costs ~544μs vs single-connection approaches.
- **RP-3 (medium confidence):** Pipelined bounce achieves 17% improvement over sequential with the per-chunk connect overhead. Removing that overhead should yield significantly more.
- **RP-4 (high confidence):** Single CUDA stream is not a bottleneck at 128KiB chunk size.
