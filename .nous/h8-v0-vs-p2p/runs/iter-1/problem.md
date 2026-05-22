# Problem Framing — Dispatcher v0 Bounce vs P2P

## Research Question

Is the bounce-buffer path (SSD→host DRAM→GPU via `cudaMemcpy`) in dispatcher v0 faster than a direct P2P path (SSD→GPU BAR1 via NVMe DMA) for a 4 MiB sequential lookup broken into 32 x 128 KiB chunks, when both use the same sequential ReadSync submission strategy?

**Key mechanism under test:** `read_from_block_device` at `components/dispatcher/v0/src/lib.rs:179-276` reads all 128 KiB chunks sequentially into a contiguous host DMA buffer, then performs one `dma_copy_to_device` call. The P2P alternative reads each chunk directly into a GPU-backed DmaBuffer obtained from `IGpuServices::prepare_memory_for_spdk()`, eliminating the host-to-device copy.

## System Interface

### Build command

```bash
RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
```

### Server launch

```bash
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/certus-server \
  --metadata-pci 0000:63:00.0 \
  --data-pci 0000:63:00.0 \
  --dispatcher-version v0 \
  --listen 0.0.0.0:50051
```

### CLI flags (code evidence)

| Flag | Semantics | Source |
|------|-----------|--------|
| `--metadata-pci` | PCI address of metadata NVMe device | `apps/certus-server/src/main.rs:29` |
| `--data-pci` | PCI address(es) of data NVMe devices | `apps/certus-server/src/main.rs:33` |
| `--dispatcher-version` | "v0" or "v1" | `apps/certus-server/src/main.rs:49` |
| `--listen` | gRPC listen address | `apps/certus-server/src/main.rs:37` |

### Python client (test harness)

```bash
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 apps/certus-server/python-client/test_client.py \
  --server localhost:50051 \
  --bench \
  --bench-object-size 4194304 \
  --bench-num-objects 10 \
  --bench-iterations 20
```

### Client flags (code evidence)

| Flag | Semantics | Source |
|------|-----------|--------|
| `--bench` | Run lookup latency benchmark | `apps/certus-server/python-client/test_client.py:498` |
| `--bench-object-size` | Object size in bytes | `apps/certus-server/python-client/test_client.py:453` |
| `--bench-num-objects` | Number of objects to benchmark | `apps/certus-server/python-client/test_client.py:457` |
| `--bench-iterations` | Lookup iterations per tier | `apps/certus-server/python-client/test_client.py:461` |

### Output format

The Python client prints benchmark results to stdout in this format:
```
  Tier            Avg (us/obj)   Min (us/obj)   Max (us/obj)   Avg (GB/s)   Peak (GB/s)
  -------...
  Memory-tier     X              X              X              X            X
  SSD-tier        X              X              X              X            X
```

Source: `apps/certus-server/python-client/test_client.py:425-428`

## Baseline Command

Server (terminal 1):
```bash
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/certus-server \
  --metadata-pci 0000:63:00.0 \
  --data-pci 0000:63:00.0 \
  --dispatcher-version v0 \
  --listen 0.0.0.0:50051
```

Client (terminal 2):
```bash
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

Not yet validated on hardware (requires SPDK + NVMe + GPU). The build is validated via:
```bash
RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
```

The Python client `bench_lookup_latency` function at `test_client.py:310-439` handles the full populate→wait→measure→cleanup cycle. For dispatcher v0 (no memory-tier), all populated objects land in staging and get written through to SSD by the background writer. The "SSD-tier" latency line measures the bounce path (sequential ReadSync + `dma_copy_to_device`).

## Experimental Conditions

### Condition A: Baseline (bounce, sequential ReadSync)

The existing dispatcher v0 code. No modifications. Server launched with `--dispatcher-version v0`.

**Measured metric**: SSD-tier lookup latency and throughput from the Python client benchmark.

### Condition B: P2P (direct NVMe→GPU, sequential ReadSync)

Modify `read_from_block_device` in `components/dispatcher/v0/src/lib.rs` to:
1. Call `gpu.prepare_memory_for_spdk(base64_payload, None)` to obtain a GPU-backed DmaBuffer
2. Read each 128 KiB chunk via ReadSync directly into offset slices of the GPU-backed buffer (using the same sequential pattern)
3. Skip the final `dma_copy_to_device` call (data is already on GPU)

This requires:
- Extending `IpcHandle` (in `components/interfaces/src/idispatcher.rs`) to carry the raw 64-byte CUDA IPC handle bytes alongside the address/size
- Modifying `certus-server/src/service.rs` lookup handler to pass the raw handle bytes through
- Adding `base64` dependency to the dispatcher crate for encoding the payload
- A new `read_from_block_device_p2p` method or modifying the existing one

**Measured metric**: Same SSD-tier lookup latency/throughput from the Python client benchmark (the client is unchanged — the P2P path is transparent to it).

### Condition C: P2P with separate per-chunk DmaBuffer allocation

Variant of Condition B where instead of one large GPU DmaBuffer, each 128 KiB chunk is read into a separately-prepared DmaBuffer slice (to test whether the overhead of a single large preparation vs multiple small preparations matters). This is only relevant if the SPDK registration has per-call overhead.

## Success Criteria

- **Direction**: If P2P (Condition B) shows consistently lower SSD-tier lookup latency across all 20 iterations, the bounce path is slower.
- **Magnitude**: Based on prior P2P experiments in this repo (handoff from h8-dispatcher-p2p), bounce-sequential was ~2206 MB/s and P2P-warm was ~3670 MB/s (1.66x ratio). We expect a similar or smaller advantage here because:
  - The certus-server adds gRPC overhead (not present in p2p-server)
  - The dispatcher has additional dispatch-map lookup overhead
  - Both paths use sequential ReadSync (not BatchSubmit)
- **Falsifiable claim**: P2P-sequential will achieve lower per-object latency than bounce-sequential for 4 MiB objects, with the advantage being >= 1.2x.

## Constraints

- All measurements MUST go through certus-server with `--dispatcher-version v0`
- Do NOT use or modify gpu-p2p-server
- Do NOT create standalone benchmark binaries
- P2P implementation goes in `components/dispatcher/v0/`
- Value size fixed at 4 MiB (32 x 128 KiB chunks)
- Hardware: device 0000:63:00.0 confirmed working; device 0000:62:00.0 has VFIO issues
- Always clear SPDK lock files before starting
- Debug build is appropriate (PCIe DMA dominates, not CPU)
- nvidia-peermem kernel module must be loaded for P2P

## Prior Knowledge

From `h8-dispatcher-p2p` iterations 1-2:
- P2P warm (BatchSubmit): 3670 MB/s / 1.09ms for 4 MiB
- Bounce (BatchSubmit): 2206 MB/s / 1.81ms for 4 MiB
- Ratio: 1.66x (varies between sessions; ratios more stable than absolutes)
- Cold P2P (GDRCopy setup overhead): 2.74x slower than bounce (first call only)
- The submission strategy (BatchSubmit vs sequential ReadSync) is hypothesized to be a significant factor independent of path
- `prepare_memory_for_spdk` returns a DmaBuffer backed by GPU BAR1 via `spdk_mem_register`
