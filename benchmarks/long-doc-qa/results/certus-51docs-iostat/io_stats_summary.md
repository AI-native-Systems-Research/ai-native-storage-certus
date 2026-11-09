# Certus SSD/extent-tier I/O — long_doc_qa 51×10k (Qwen2.5-7B)

Run: `CONNECTOR=certus`, 51 docs × 10000 tok, out=100, inflight=4, slab=2 MiB.
certus-server pid 13338 (`--memory-tier-size 12G --format`, 4×NVMe).
Sampled `Dispatcher.GetIoStats` every 1s during the run (337 samples,
`iostat_samples.csv`). Deltas are last−first sample.

## SSD/extent block-device I/O for the whole run
| metric | value |
|---|---|
| read ops | 279,552 |
| read bytes | 36,641,439,744 (34.12 GiB) |
| mean read size | 128.0 KiB/op |
| mean read latency | 438.3 µs/op |
| **write ops** | **0** |
| **write bytes** | **0** |

Write counters were byte-identical across all 337 samples → the SSD tier
absorbed **zero writes** during the run (store, query, and 15s settle).

## Per phase (reads only; writes zero throughout)
| phase | SSD read | active throughput |
|---|---|---|
| warmup / store round (t≈240–267s) | ~17.06 GiB | 622 MiB/s avg, peak 1654 MiB/s |
| query / fetch round  (t≈284–310s) | ~17.06 GiB | 647 MiB/s avg, peak 1686 MiB/s |

Reads are absent 0–239s (model load + priming), then burst 0.4–1.7 GiB/s
during each round.

## vLLM cache (same run)
- External (Certus) prefix cache hit: **100.0%** (1,024,960 / 1,025,194)
- GPU prefix cache hit: 0% (0 / 1,025,194)
- Round times: warmup 45.8s (TTFT 0.978s) / query 43.3s (TTFT 0.762s)

## Reading of the zero-write result
The workload is deterministic (identical filler docs, model, `PYTHONHASHSEED=0`),
run many times before this one; the server was already ~91.5% DRAM-tier full and
holding ~80 GiB of extents. These exact KV blocks were **already persisted on the
SSD tier from prior runs**, so this run served its 100% external hits by *reading
back* existing extents and wrote nothing new. 34 GiB was served from SSD (17 GiB
per round at ~650 MiB/s); the remainder of the 100%-hit KV came from the 12 GiB
DRAM tier.

## What GetIoStats does NOT give
It is a single global block-device counter — no DRAM-vs-SSD hit split, no
hit-rate. So "how much of the 100% external hit came from DRAM vs SSD" is only
inferable (served − 34 GiB-from-SSD), not directly measured. The 128 KiB read
unit is the device read granularity; a Certus KV block (~896 KiB) is ~7 reads.
