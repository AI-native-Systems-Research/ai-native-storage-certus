# Certus SSD I/O — long_doc_qa 51×10k (Qwen2.5-7B), COLD tier

certus-server restarted with `--format` (pid 81101) → extent store re-formatted,
DRAM tier empty. Baseline counters ~0 (48 write ops = superblock/bitmap format).
Sampled `Dispatcher.GetIoStats` @1Hz (300 samples). Contrast with the warm run
(results/certus-51docs-iostat/, tier pre-populated from prior runs).

## SSD/extent block-device I/O for the whole run
| dir | ops | bytes | mean latency |
|---|---|---|---|
| write | 228,771 | 27.84 GiB | 32.9 µs/op |
| read  | 223,125 | 27.24 GiB | 2068.3 µs/op |

## Per phase (round time + active-window throughput)
| phase | round time | TTFT | write | read | throughput | notes |
|---|---|---|---|---|---|---|
| warmup / cold store | 103.2 s | 2.124 s | 27.81 GiB (228,430 ops) | 4.81 GiB | **write 254 MiB/s avg, peak 1656 MiB/s (~1.6 GiB/s)** | KV computed fresh + stored; spills past 12 GiB DRAM onto SSD. 4.8 GiB read = shared-prefix reuse across the 51 filler docs within the round. Avg is low because stores are bursty per-doc over the 103 s round; peak is the real burst. |
| query / fetch | 43.9 s | 0.560 s | ~0.02 GiB | 22.43 GiB (183,750 ops) | **read 636 MiB/s avg, peak 1539 MiB/s (~1.5 GiB/s)** | evicted blocks re-fetched from SSD; DRAM tier serves the rest with no device I/O. |

## vLLM cache
- External (Certus) prefix hit: **49.7%** (510,000 / 1,025,194) — warmup is a genuine
  cold MISS (store), query round HITs (510k tok ≈ 51×10k). Warm run was 100% (both
  rounds hit pre-existing data).
- GPU prefix hit: 0%.
- Round times: warmup **103.2s** (TTFT 2.124s) — 2.25× the warm warmup (45.8s), the
  cost of real prefill + cold store. Query 43.9s (TTFT 0.560s) ≈ warm query (43.3s).

## Warm vs cold — the headline
| | warm tier | cold tier |
|---|---|---|
| SSD write | 0 GiB | 27.84 GiB |
| SSD read | 34.12 GiB | 27.24 GiB |
| external hit | 100.0% | 49.7% |
| warmup round | 45.8 s | 103.2 s |

## Latency asymmetry (architectural)
Writes ack in **33 µs**, reads take **2.07 ms** — ~60×. Writes are absorbed by the
dispatcher's async staging-to-SSD worker (fast ack, off the critical path); reads
are synchronous on the KV-fetch/generation path. cuFile is in compatible mode, so
reads are not true GPUDirect. The read-latency sum (≈380 s) over 43.9 s query wall
implies ~8-9× effective concurrency (4 drives × 2 queues).
