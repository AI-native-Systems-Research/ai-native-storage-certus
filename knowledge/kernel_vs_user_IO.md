# Kernel vs Userspace I/O: SPDK, io_uring, and O_DIRECT

Performance comparison for an 8 NVMe SSD array under inference-style workloads.

## Throughput (4KiB random read, QD=128 per device)

| Metric | SPDK | io_uring | O_DIRECT (libaio) |
|--------|------|----------|-------------------|
| Per-device IOPS | ~1.5–1.7M | ~1.2–1.4M | ~0.8–1.0M |
| 8-device aggregate | ~12–14M IOPS | ~9–11M IOPS | ~6–8M IOPS |
| Bandwidth (128KiB seq read) | ~55–56 GB/s | ~48–52 GB/s | ~38–44 GB/s |

## Latency (4KiB random read, QD=1)

| Metric | SPDK | io_uring | O_DIRECT (libaio) |
|--------|------|----------|-------------------|
| Average | ~6–8 us | ~10–14 us | ~12–18 us |
| P99 | ~10–12 us | ~18–25 us | ~25–40 us |
| P99.9 | ~12–15 us | ~30–60 us | ~50–120 us |

## Tail Latency Under Mixed Load (70R/30W, QD=64)

| Percentile | SPDK | io_uring | O_DIRECT (libaio) |
|------------|------|----------|-------------------|
| P50 | ~8 us | ~14 us | ~20 us |
| P99 | ~14 us | ~35 us | ~60 us |
| P99.99 | ~20 us | ~80 us | ~200–500 us |

## Throughput (4MiB sequential read, QD=32 per device)

| Metric | SPDK | io_uring | O_DIRECT (libaio) |
|--------|------|----------|-------------------|
| Per-device BW | ~6.8–7.0 GB/s | ~6.5–6.8 GB/s | ~6.0–6.5 GB/s |
| 8-device aggregate BW | ~54–56 GB/s | ~50–54 GB/s | ~46–50 GB/s |
| Per-device IOPS | ~1.7–1.75K | ~1.6–1.7K | ~1.5–1.6K |

## Latency (4MiB sequential read, QD=1)

| Metric | SPDK | io_uring | O_DIRECT (libaio) |
|--------|------|----------|-------------------|
| Average | ~570–600 us | ~590–640 us | ~620–680 us |
| P99 | ~620–650 us | ~680–750 us | ~750–900 us |
| P99.9 | ~650–700 us | ~750–900 us | ~900–1400 us |

### Notes on 4MiB I/O behavior

At 4MiB, the NVMe controller splits each command into multiple internal flash reads (~128–256KiB granularity). Media transfer time dominates (~500+ us), so the kernel overhead gap narrows significantly compared to 4KiB:

- **Throughput gap narrows to ~8–15%** (vs ~30–50% at 4KiB) because media time dwarfs software overhead.
- **Latency gap narrows in absolute terms** but SPDK still wins at the tail — the kernel block layer adds jitter from bio splitting, bounce buffers on misaligned requests, and IRQ batching at large transfer sizes.
- **CPU efficiency**: SPDK completes 4MiB I/Os with fewer CPU cycles per byte (no bio chain allocation, no kernel copy for scatter-gather setup). At 8 devices saturating PCIe bandwidth, this frees 1–2 cores vs O_DIRECT.
- **PCIe becomes the bottleneck**: 8 Gen4 x4 devices share PCIe lanes. At 4MiB sequential reads, all three approaches can saturate the PCIe root complex (~64 GB/s for Gen4 x16 CPU link), at which point the I/O framework matters less than topology.

## Overhead Characteristics

| | SPDK | io_uring | O_DIRECT (libaio) |
|--|------|----------|-------------------|
| Syscalls per I/O | 0 | 0–1 (batched) | 2 (submit + reap) |
| Context switches | 0 | 0–1 | 1–2 per I/O |
| Interrupt handling | Polled (none) | Polled or IRQ | IRQ-driven |
| Kernel block layer | Bypassed | Traversed | Traversed |

## Scaling (8 devices)

| | SPDK | io_uring | O_DIRECT (libaio) |
|--|------|----------|-------------------|
| Linear scaling | To ~12–16 devices | To ~6–8 devices | To ~4–6 devices |
| Bottleneck | CPU cores for pollers | Kernel lock contention | Block layer + IRQ affinity |
| CPU cores needed | 4–8 dedicated | 2–4 (SQPOLL) or shared | Shared, more total CPU |

## Key Differentiators

**SPDK**: Kernel bypass, no syscalls, polled completions, direct doorbell writes. Trades dedicated cores for deterministic latency. Best P99.99 tail.

**io_uring**: Batched submissions, optional SQ polling (`IORING_SETUP_SQPOLL`) closes 30–40% of the gap to SPDK. Works with any block device. Good balance of performance and ecosystem compatibility.

**O_DIRECT (libaio)**: Full kernel block layer traversal (bio allocation, scheduler, interrupt-driven completions). Broadest compatibility but kernel overhead dominates once device latency drops below ~15 us (Gen4/Gen5 NVMe).

## Why Tail Latency Diverges

- O_DIRECT P99.99 spikes from: IRQ coalescing timeouts, block layer merging, scheduler reordering, journal/flush interactions under writes.
- io_uring improves via batching and optional polling but still traverses the kernel block layer.
- SPDK eliminates all kernel-side variance sources; tail latency is bounded by NVMe controller behavior only.

## Recommendation for Inference Workloads

SPDK provides ~3–5x better tail latency than O_DIRECT and ~20–30% higher throughput. The predictable latency matters more than raw IOPS for inference serving, where a single straggler read can delay an entire batch.
