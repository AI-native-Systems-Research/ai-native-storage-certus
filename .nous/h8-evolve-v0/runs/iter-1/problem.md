# Problem Framing: Pipelined Bounce Buffer vs Direct P2P

## Research Question

Can a pipelined bounce buffer (SSD→CPU→GPU with overlapping NVMe reads and H2D copies) match or exceed direct P2P (SSD→GPU via GDRCopy BAR1) for 4 MiB transfers broken into 32×128 KiB chunks?

Prior iter-2 decomposition established that the current **non-pipelined** bounce spends ~790μs on NVMe reads + ~819μs on sequential H2D copies = ~1610μs total. P2P warm achieves ~824μs (710μs read + 114μs D2D copy). Since NVMe reads and H2D copies use independent hardware (NVMe DMA engine vs GPU copy engine) with no measured PCIe interference (ablation confirmed <4% difference), true pipelining should reduce bounce total to ~max(790, 819) ≈ 820μs — matching P2P warm.

**Key source files:**
- `components/gpu-services/v0/src/bin/p2p_server.rs:374-433` — `handle_bounce` (current sequential two-phase implementation)
- `components/gpu-services/v0/src/bin/p2p_server.rs:272-322` — `do_chunked_read` (BatchSubmit of all chunks at once)
- `components/gpu-services/v0/src/cuda_ffi.rs:71-111` — CUDA FFI bindings (lacks `cudaMemcpyAsync`/streams)
- `components/gpu-services/v0/tests/gpu_client_p2p.py:44-61` — `do_transfer` client timing

## System Interface

- **Build command:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`
- **CLI flags:**
  - `--mode <bounce|p2p|p2p-cold>` — transfer mode (p2p_server.rs:50-51)
  - `--chunk-size <bytes>` — NVMe I/O chunk size, default 131072 (p2p_server.rs:57-59)
  - `--staging-size <bytes>` — pre-allocated GPU staging pool for P2P mode (p2p_server.rs:53-55)
  - `--pci <DDDD:BB:DD.F>` — NVMe PCI address (p2p_server.rs:45-47)
  - `--socket <path>` — Unix domain socket path (p2p_server.rs:42-43)
  - `--once` — serve one client then exit (p2p_server.rs:61-63)
- **Code evidence:** CLI flags defined in `Cli` struct at p2p_server.rs:38-64 using clap derive macros.
- **Output mechanism:** Client script (`gpu_client_p2p.py`) measures wall-clock latency per transfer and reports throughput/latency to stderr. Server responds with `OK <size> bytes (<mode>, <chunks> chunks)` per request. Prior iter-2 added phase timing in response: `read_us=N copy_us=N`.

## Baseline Command

```bash
bash .nous/h8-transfer-path/runs/iter-1/inputs/run_condition.sh \
  bounce results/baseline-bounce.txt 0000:62:00.0
```

This launches the server in bounce mode, runs 50 iterations of 4 MiB transfers via the Python client, and saves output to the specified file.

## Baseline Validation

Prior iteration validated on hardware (iter-1, iter-2 confirmed):
- Exit code: 0
- Bounce mode: 1510-1544 MB/s throughput, 2.59-2.65 ms avg latency
- P2P warm: 3031-3064 MB/s throughput, 1.31-1.32 ms avg latency
- Phase breakdown (iter-2): bounce read_us=790, copy_us=819; P2P read_us=710, copy_us=114

Build command verified: exits 0 with one dead_code warning.

## Experimental Conditions

### Condition 1: Pipelined Bounce (h-main)

**Code change required.** Implement double-buffered pipelining in `handle_bounce`:
1. Add `cudaMemcpyAsync` and CUDA stream FFI bindings to `cuda_ffi.rs`
2. Implement a pipelined `handle_bounce_pipelined` function that:
   - Issues NVMe reads one chunk at a time (individual `ReadAsync` per chunk, not BatchSubmit)
   - As each NVMe read completes, immediately starts `cudaMemcpyAsync` H2D for that chunk on a CUDA stream
   - While H2D copy is in flight, issues the next NVMe read
   - Uses a double-buffer scheme: 2 host DMA buffers, alternating between them
   - After all chunks dispatched, `cudaStreamSynchronize` to ensure all copies complete
3. Add `--mode bounce-pipelined` to the CLI (new enum variant)
4. Add per-phase timing instrumentation (`read_us`, `copy_us`, `total_us`) in response

**Run command:**
```bash
bash .nous/h8-evolve-v0/runs/iter-1/inputs/run_condition.sh \
  bounce-pipelined results/pipelined-bounce.txt 0000:62:00.0
```

### Condition 2: Non-pipelined Bounce (baseline)

No code changes. Uses existing `handle_bounce` (sequential read-all then copy-all).
Add per-phase timing instrumentation to match h-main output format.

**Run command:**
```bash
bash .nous/h8-evolve-v0/runs/iter-1/inputs/run_condition.sh \
  bounce results/sequential-bounce.txt 0000:62:00.0
```

### Condition 3: P2P Warm (reference)

No code changes. Uses existing `handle_p2p` with pre-pinned chunk pool.
Add per-phase timing instrumentation.

**Run command:**
```bash
bash .nous/h8-evolve-v0/runs/iter-1/inputs/run_condition.sh \
  p2p results/p2p-warm.txt 0000:62:00.0
```

### Condition 4: Pipelined Bounce with 2 streams (h-robustness)

Same as Condition 1 but uses 2 CUDA streams instead of 1, allowing multiple H2D copies to be in flight simultaneously. Tests whether a single stream is a bottleneck.

**Run command:**
```bash
bash .nous/h8-evolve-v0/runs/iter-1/inputs/run_condition.sh \
  bounce-pipelined-2stream results/pipelined-2stream.txt 0000:62:00.0
```

## Success Criteria

1. **Pipelined bounce latency** consistently lower than sequential bounce across seeds (direction: decrease)
2. **Pipelined bounce latency approaches P2P warm latency** — within 30% (predicted: within ~5% based on phase decomposition math)
3. **Throughput improvement** from pipelining should be proportional to latency reduction
4. **Phase overlap observable:** In pipelined mode, `total_us < read_us + copy_us` (phases overlap), while in sequential mode `total_us ≈ read_us + copy_us`

## Constraints

- NVMe chunk size fixed at 128 KiB (MDTS limit per p2p_server.rs:57)
- Total transfer size 4 MiB (32 chunks)
- Build requires `RUSTFLAGS='-L /usr/local/lib'` for libgdrapi
- NVMe device at PCI 0000:62:00.0, exclusive SPDK access
- Server startup requires ~5s (SPDK+CUDA init)
- GDRCopy requires `nvidia_peermem` and `gdrdrv` kernel modules
- `cudaMemcpyAsync` requires pinned (page-locked) host memory to be truly asynchronous — SPDK hugepage allocations via `DmaBuffer::new` should qualify since they're backed by hugepages

## Prior Knowledge

This is iteration 1 of the `h8-evolve-v0` campaign. No active principles extracted yet.

However, substantial prior findings exist from the `h8-transfer-path` campaign (iterations 1-2):
- Iter-1: Established P2P warm 2x faster than sequential bounce (3031 vs 1510 MB/s)
- Iter-2: Decomposed latency — bounce read=790μs, copy=819μs; P2P read=710μs, copy=114μs
- Iter-2 ablation: No PCIe interference between NVMe reads and H2D copies (<4% difference)
- These findings directly motivate the pipelining hypothesis: independent hardware paths + no interference = ideal overlap candidate
