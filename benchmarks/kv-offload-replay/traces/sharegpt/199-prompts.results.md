# Replay results — `traces/sharegpt/199-prompts`

Replay of the committed 199-prompt ShareGPT trace
(`199-prompts.mgr.jsonl` + `199-prompts.handler.jsonl`) through every target
that runs end-to-end on this box. Captured 2026-05-13 against
`vllm==0.20.2+cu129`, `torch==2.11.0+cu130`, `llmd_fs_backend` upstream
commit `9e3eb64` (vLLM 0.20.2 migration), Certus from this branch.

Same trace replayed through every backend — identical 455-record manager
call sequence and 335-record handler transfer sequence in every row.

## Manager replay (admission + eviction + bookkeeping)

| Target | num_blocks | Wall | ops/s | prepare_store p99 |
|---|---|---|---|---|
| simple-lru (pure Python) | 256 | 0.008 s | 59,008 | 0.021 ms |
| certus-connector policy-only | 1,024 | 0.006 s | 72,759 | 0.074 ms |
| certus-connector native (SPDK + NVMe) | 16,384 | 0.012 s | 37,758 | 0.133 ms |
| fs-backend (XFS on NVMe) | 16,384 | 0.150 s | 3,028 | 3.9 ms (max 111 ms) |

## Handler replay (real IO, write path only)

442 blocks × 128 KiB = **55.2 MiB** moved per run. The trace contains 199
GPU→CPU transfers and zero CPU→GPU, so this exercises the store path only.

| Target | Storage | Wall | Throughput | p50 | p99 |
|---|---|---|---|---|---|
| certus-connector native | SPDK direct DMA → NVMe | 0.125 s | **441 MB/s** | 1.20 ms | 124.8 ms |
| fs-backend | POSIX → XFS → NVMe (same drive class) | 1.180 s | **46.8 MB/s** | 1047 ms | 1058 ms |

Both backends ran on Intel SSDPF2KE032T9L 3.2 TB drives in this server.
For Certus that drive (`0000:61:00.0`) is bound to `vfio-pci`; for fs-backend
a sibling drive of the same model (`0000:c4:00.0`) was rebound to the
kernel `nvme` driver and formatted XFS at `/mnt/fs-backend-bench`.

## Interpretation

- **~9× throughput, ~870× p50 latency gap** between Certus and fs-backend
  on the same physical drive class. The gap is architectural, not media:
  fs-backend's engine reports `2 write-preferring workers` and serializes
  199 transfers through them — the uniform ~1 s p50 is dispatch tail, not
  disk. Certus skips the kernel/filesystem stack via SPDK direct DMA.
- An fs-backend tmpfs run produced 47.1 MB/s — within margin of the NVMe
  number, confirming the bottleneck is in the engine, not the storage.
- 0% hit rate on every target — expected. The trace is store-only (no
  CPU→GPU restores), so `lookup` never finds anything.

## Caveats

- Trace replay is **open-loop** — see main README "Not closed-loop". Per-call
  decisions are faithful, but the captured key stream was driven by the
  original vLLM 0.19 + offloading run.
- The fs-backend `cpu-manager` (vLLM built-in) target wasn't tested — its
  vLLM 0.20 API requires a richer `req_context` than the replay tool currently
  synthesizes.
- fs-backend was run with default config; tuning workers / staging buffer
  size could close some of the gap.
