# Problem Framing: Phase Decomposition of Bounce vs P2P Latency

## Research Question

Iteration 1 established that P2P warm is ~2x faster than bounce for 4 MiB NVMe→GPU transfers with 128 KiB chunks. This iteration asks: **Is the 2x latency gap caused by the copy phase (H2D vs D2D) or the NVMe read phase (host-DMA vs BAR1-DMA)?**

Hypothesis: The copy phase dominates — D2D copies are dramatically faster than H2D copies at 128 KiB granularity, and the NVMe read phases are approximately equal regardless of DMA target.

Implementation files:
- `handle_bounce`: `components/gpu-services/v0/src/bin/p2p_server.rs:374-433`
- `handle_p2p`: `components/gpu-services/v0/src/bin/p2p_server.rs:436-490`
- `do_chunked_read`: `components/gpu-services/v0/src/bin/p2p_server.rs:272-322`
- Client parsing: `components/gpu-services/v0/tests/gpu_client_p2p.py:44-61`

## System Interface

- **Build:** `RUSTFLAGS='-L /usr/local/lib' cargo build -p gpu-services --features p2p --bin gpu-p2p-server`
- **CLI flags relevant to experiment:**
  - `--mode <bounce|p2p|p2p-cold>` — transfer mode (`p2p_server.rs:50`)
  - `--chunk-size <bytes>` — NVMe I/O chunk size, default 131072 (`p2p_server.rs:58`)
  - `--staging-size <bytes>` — pre-allocated staging for p2p mode, default 4194304 (`p2p_server.rs:54`)
  - `--pci <addr>` — NVMe PCI address (`p2p_server.rs:46`)
  - `--socket <path>` — Unix socket path (`p2p_server.rs:42`)
- **Code evidence:** All flags defined in `Cli` struct at `p2p_server.rs:38-64`.
- **Output:** Server responds with "OK ..." line per request. Client prints benchmark stats to stderr. The `run_condition.sh` harness captures combined stdout+stderr to a file.

## Baseline Command

```bash
bash .nous/h8-transfer-path/runs/iter-2/inputs/run_condition.sh \
  bounce results/bounce-s1.txt 0000:62:00.0
```

## Baseline Validation

Not smoke-testable without hardware access. Iter-1 validated the same harness:
- Exit code: 0
- Output format: table with Throughput/Avg/Min/Max latency lines
- Baseline bounce result: 1544.0 MB/s, 2.59 ms avg latency
- Baseline P2P warm result: 3064.0 MB/s, 1.31 ms avg latency

The same binary and harness are used; this iteration only adds instrumentation code changes.

## Experimental Conditions

### Condition 1: `instrumented-bounce` (code change)

Instrument `handle_bounce` with `std::time::Instant` timing around:
1. The `do_chunked_read()` call (NVMe read phase)
2. The `cudaMemcpy H2D` loop (copy phase)

Append `read_us=<N> copy_us=<N>` to the OK response string. This allows the client to report per-phase breakdowns.

Modify the Python client to parse `read_us` and `copy_us` from response lines and report phase statistics.

Run with: `--mode bounce`

### Condition 2: `instrumented-p2p` (code change, same instrumentation)

Instrument `handle_p2p` with equivalent timing:
1. The `do_chunked_read()` call (NVMe read phase)
2. The `cudaMemcpy D2D` loop (copy phase)

Append `read_us=<N> copy_us=<N>` to the OK response string.

Run with: `--mode p2p`

### Condition 3: `copy-only-bounce` (code change — ablation)

Add a `--skip-nvme` flag to the server. When set, skip the `do_chunked_read()` call entirely and proceed directly to the copy phase. The buffers will contain whatever was in them from allocation (zeros from hugepage pool). This isolates the H2D copy time without NVMe read contribution.

Run with: `--mode bounce --skip-nvme`

### Condition 4: `copy-only-p2p` (code change — ablation)

Same `--skip-nvme` flag with P2P mode. Skips `do_chunked_read()`, proceeds directly to D2D copy phase from pre-allocated GPU staging buffers.

Run with: `--mode p2p --skip-nvme`

## Success Criteria

1. **Phase attribution:** NVMe read phase accounts for a roughly equal fraction of total time in both modes (within 20% of each other), while the copy phase shows a clear >1.5x difference between H2D and D2D — confirming the copy phase as the dominant latency differentiator.
2. **Copy isolation consistency:** The copy-only ablation (no NVMe reads) shows H2D copy time > D2D copy time by a similar ratio (~2x) as observed in the full path, confirming the copy is not confounded by NVMe reads.
3. **Instrumentation overhead:** Instrumented full-path timings (`read_us + copy_us`) should account for >90% of the client-measured total latency (the remainder being socket overhead and IPC handle open/close).

## Constraints

- Per RP-1: P2P warm with pre-warmed pool is ~2x faster overall (established).
- Per RP-2: P2P cold overhead is ~6ms from GDRCopy setup (not relevant to this iteration).
- Must not change the benchmark semantics — same 4 MiB transfer, same 128 KiB chunks.
- `--skip-nvme` ablation removes data correctness (buffers contain stale/zero data) but this is acceptable for timing measurement.
- MDTS limit is 128 KiB — do not change chunk size.
- Server uses synchronous `cudaMemcpy` — timing the copy loop captures actual GPU work completion.

## Prior Knowledge

- **RP-1** (high confidence): P2P warm achieves ~2x throughput and ~2x lower latency than bounce. The D2D copy stays within GPU memory without PCIe traversal.
- **RP-2** (high confidence): GDRCopy cold setup adds ~6ms for 32 chunks. Not tested in this iteration.
- **Iter-1 diagnostic** for h-main refutation: "The extra cudaMemcpy H2D step in bounce mode (32 sequential H2D copies of 128 KiB each) is the bottleneck." This iteration directly tests that claim.
