# multiturn-router: prompt-level multi-turn through the llm-d router

`apps/workload-generator` synthesizes block keys; this drives real conversations at the
prompt level, through an OpenAI-compatible router, so that a turn can reuse the KV a
previous turn generated wherever it was cached.

| file | role |
|---|---|
| `multiturn_router.py` | the driver: ShareGPT human turns, token-exact prompts (render endpoint + `return_token_ids`), N sessions, M at a time, one JSON line per turn |
| `run_arm.sh` | one arm: the fleet scraper (`synthetic-agentic/scrape_prom.py --k8s`) beside the driver, then the EPP's p2p decisions for the arm's window |
| `compare_arms.py` | two arms side by side: fetches, prefix-cache hits, router decisions, TTFT |
| `gdr_bw.sh` | RDMA write bandwidth node2 -> node5, host and GPU memory on each end (`ib_write_bw --use_cuda`) |

Each turn's prompt = previous prompt + the model's answer as token ids + the next user
message, tokenized by the router's render service, with no chat template. A per-run `--tag`
prefixes every session so two arms never share cache.

## 2026-10-26: decode -> prefill fetch, on vs off

**Setup.** node2 (2 decode, A30) + node5 (1 prefill, A30), Qwen3-8B, P/D with
`MultiConnector[NixlConnector, Offloading(certus)]`, NIXL over TCP on the pod network.
pd-certus with peer selection milestone 3 (router: precise prefix producer over KV events,
`p2p-source-producer`; `CERTUS_EVENT_MEDIUM=cpu`). 12 ShareGPT conversations x 5 turns,
3 at a time, 1 100-token answers (`ignore_eos`), through the router. **Off** = decode's KV
events off, so the router cannot see what decode generated; everything else identical.

| | fetch on | fetch off |
|---|---|---|
| turns (errors) | 60 (0) | 60 (0) |
| router `pulled` decisions | **48** (every later turn) | 0 (57 `self`) |
| FETCH requests / blocks fetched (prefill's certus) | 48 / **3 299** | 0 / 0 |
| ... served as fetched / misses | 3 092 / 133 | - |
| prefill external prefix-cache hit rate | **88 %** (49 472 / 56 074 tokens) | 0 % |
| prefill GPU prefix-cache hits | 86 400 / 142 474 | 86 400 / 142 474 |
| TTFT, later turns: median / mean | **1.93 s / 2.03 s** | 2.16 s / 2.40 s |
| TTFT median by turn 0-4 | 0.25 / 2.01 / 0.49 / 1.44 / 1.30 | 0.26 / 2.45 / 1.37 / 1.74 / 1.74 |
| end-to-end latency, median | 27.1 s | 27.6 s |
| wall time | 549 s | 552 s |

**Reading it.**
- Every turn after the first fetched the previous answer from the decode pod's certus:
  48 pulls, 3 299 blocks, 88 % of the tokens prefill would otherwise have computed. Without
  decode's events the router names no source and prefill recomputes them.
- TTFT improves 11 % at the median and 16 % at the mean, but the TTFT measured here includes
  decode's NIXL pull of the whole prompt over TCP (~107 MB/s), the same in both arms, so it
  understates what prefill saves. End-to-end latency is dominated by generating 1 100
  tokens and barely moves.
- GPU prefix-cache hits are identical in both arms: prefill's own GPU cache holds the
  prompts it computed; certus supplies exactly the part it never saw, the answers.

## 2026-10-26: the same, with NIXL GPU to GPU over RDMA

Identical setup and workload, but `pd-certus-rdma`: NIXL with `kv_buffer_device: cuda`
over UCX RC verbs on the `roce` secondary network (ipvlan, GPUDirect). The prefill->decode
handoff is RDMA; certus's decode->prefill fetch is unchanged (its own RDMA endpoint).
Results in `results/2026-10-26-rdma-fetch-{on,off}/`.

| | TCP, fetch on | TCP, fetch off | RDMA, fetch on | RDMA, fetch off |
|---|---|---|---|---|
| turns (errors) | 60 (0) | 60 (0) | 60 (0) | 60 (0) |
| router `pulled` / blocks fetched | 48 / 3 299 | 0 / 0 | 48 / **3 301** | 0 / 0 |
| ... served as fetched / misses | 3 092 / 133 | - | **3 301** / 132 | - |
| prefill external prefix-cache hit rate | 88 % | 0 % | **94 %** (52 816 / 55 974) | 0 % |
| TTFT, later turns: median | 1.93 s | 2.16 s | **0.20 s** | 0.33 s |
| TTFT, later turns: mean | 2.03 s | 2.40 s | **0.40 s** | 0.53 s |
| TTFT median by turn 0-4 | 0.25 / 2.01 / 0.49 / 1.44 / 1.30 | 0.26 / 2.45 / 1.37 / 1.74 / 1.74 | 0.14 / 0.18 / 0.22 / 0.20 / 0.20 | 0.12 / 0.31 / 0.31 / 0.36 / 0.37 |
| end-to-end latency, median | 27.1 s | 27.6 s | 25.9 s | 25.9 s |
| wall time | 549 s | 552 s | 523 s | 523 s |

"Later turns" are turns 1-4, the ones that can reuse a previous answer (`compare_arms.py`).

**Reading it.**
- **The handoff was most of TTFT.** Moving NIXL from TCP (~107 MB/s) to GPUDirect RDMA
  cuts TTFT about 6-10x in both arms; what is left is prefill's compute plus decode's pull
  of the prompt's KV (up to ~0.7 GB at turn 4, at ~9 GB/s).
- **With the handoff out of the way, the fetch shows.** Fetch on vs off: median TTFT
  0.20 s vs 0.33 s (-38 %), mean 0.40 s vs 0.53 s (-25 %). Under TCP the same fetch was
  -11 % / -16 %, diluted by the transfer both arms paid.
- **Fetch on, TTFT stays flat across turns** (~0.20 s); fetch off, it climbs (0.31 ->
  0.37 s). Either arm's prompt, and so the handoff, grows each turn; why only the off arm
  climbs is not separated here (prefill recomputes ~1 100 tokens of answer per turn in
  both cases).
- Every block fetched was served as fetched (3 301 / 3 301), against 3 092 of 3 299 under
  TCP. The 132 misses (~2.8 per fetch) are not explained yet.
- End-to-end latency is generating 1 100 tokens and is the same in all four arms within
  noise; the RDMA arms are ~1.5 s shorter because of the handoff.

## 2026-10-26: GPUDirect RDMA bandwidth, node2 <-> node5

`gdr_bw.sh`, 1 MiB messages, 4 QPs, mlx5_0 (200 Gb/s RoCE v2), `nvidia-peermem` loaded.
node2 GPU0 is on the NIC's NUMA node; node5's GPU is on the other socket from its NIC
(`nvidia-smi topo`: SYS).

Client node2, server node5. A WRITE moves data node2 -> node5; a READ is issued by node2
and moves data node5 -> node2, so node5's NIC reads *from* node5's memory.

| node2 memory / node5 memory | RDMA WRITE (node2 -> node5) | RDMA READ (node5 -> node2) |
|---|---|---|
| host / host | 172.4 Gb/s | 174.4 Gb/s |
| GPU0 / host | 171.4 Gb/s | 171.8 Gb/s |
| host / GPU | 169.7 Gb/s | **93.0 Gb/s** |
| GPU0 / GPU | 170.9 Gb/s | **90.8 Gb/s** |

- GPUDirect works at line rate in every direction except one: **reading out of node5's
  GPU** (node5's NIC pulling from a GPU on the other socket) halves, to ~91 Gb/s. Writing
  *into* node5's GPU from across the socket is full rate.
- NIXL's decode side pulls the prompt's KV from the prefill pod with RDMA READ. Prefill is
  on node5, so a GPU-to-GPU handoff reads out of node5's GPU: **~91 Gb/s (~11 GB/s)**,
  against ~107 MB/s over TCP today -- still about 100x better. Host staging on node5
  (`kv_buffer_device: cpu`) would read at full rate but costs a GPU->host copy first.
- Both are far above what the pods get today; the blocker is the pods' network, not the
  NIC or GPUDirect.
