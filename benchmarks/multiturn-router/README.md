# multiturn-router: prompt-level multi-turn through the llm-d router

`apps/workload-generator` synthesizes block keys; this drives real conversations at the
prompt level, through an OpenAI-compatible router, so that a turn can reuse the KV a
previous turn generated wherever it was cached.

| file | role |
|---|---|
| `multiturn_router.py` | the driver: ShareGPT human turns, token-exact prompts (render endpoint + `return_token_ids`), N sessions, M at a time, one JSON line per turn |
| `run_arm.sh` | one arm: the fleet scraper (`synthetic-agentic/scrape_prom.py --k8s`) beside the driver, then the EPP's p2p decisions for the arm's window |
| `compare_arms.py` | two arms side by side: fetches, prefix-cache hits, router decisions, TTFT |
| `prefill_cost.py` | prefill compute for N new tokens over an already-cached context, on the prefill pod directly |
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
- **The on/off gap is prefill computing the previous answer.** Fetch off, prefill
  computes ~1 100 answer tokens plus the user message each turn; fetch on, only the
  message (~40 tokens) and the partial blocks. Timed directly on the prefill pod
  (`max_tokens: 1`, context already in its GPU cache, median of 5):

  | context already cached | +1 100 tokens | +40 tokens | difference |
  |---|---|---|---|
  | 1 229 | 0.185 s | 0.044 s | 0.141 s |
  | 2 371 | 0.185 s | 0.035 s | 0.150 s |
  | 3 528 | 0.207 s | 0.050 s | 0.157 s |

  against measured on/off TTFT gaps of 0.13 / 0.09 / 0.16 / 0.17 s on turns 1-4.
- **Why fetch off climbs with the turn.** Two things grow with the prompt: computing
  1 100 tokens costs more over a longer context (+0.022 s from 1 229 to 3 528 tokens,
  above), and decode's pull of the prompt's KV grows (Qwen3-8B: ~147 KB per token, so
  ~0.18 GB at turn 1 to ~0.69 GB at turn 4, +~0.05 s at ~9 GB/s). Together ~+0.08 s,
  against +0.054 s measured. Fetch on pays the same handoff growth but its medians move
  only +0.02 s (0.18 -> 0.20); with 12 sessions per turn that difference is within the
  spread, so "flat" is not a separate effect worth claiming.
- Every block fetched was served as fetched (3 301 / 3 301), against 3 092 of 3 299 under
  TCP.
- **The misses are the new user message, which nobody holds yet.** Turn k's prompt is
  turn k-1's prompt + answer + the next user message. Prefill holds the full 16-token
  blocks of the old prompt; decode holds those of prompt + answer (`(P+A-1)//16`, since
  the last token generated has no KV). The connector asks decode for every block prefill
  lacks, up to `P'//16`, so the blocks past decode's end -- the one straddling the end of
  the answer and the user's new text -- come back as misses. Counted from `turns.jsonl`
  alone, that predicts **3 301 hits and 132 misses**, exactly the RDMA arm's numbers (and
  3 298 / 134 for TCP, against 3 299 / 133 measured: the same 3 432 keys, one block moved
  across the boundary). Correct behaviour: the source reports what it does not hold, and
  prefill computes those blocks.
- End-to-end latency is generating 1 100 tokens and is the same in all four arms within
  noise; the RDMA arms are ~1.5 s shorter because of the handoff.

## 2026-10-27: how often aggregated serving moves a session (peer selection M4?)

Peer selection milestone 4 -- a routing sidecar in front of aggregated pods, so a session
the router moves to another pod fetches its prefix from the old pod's certus -- was to be
built only if moves are frequent. Measured on llm-d's optimized-baseline router
(`prefix-cache-affinity-filter` + `token-load-scorer`), 3 aggregated pods (2 on node2,
1 on node5), certus offload on, Qwen3-8B, 12 ShareGPT sessions x 5 turns. A move is a
later turn served by a different pod than the turn before it. Results and scripts in
`results/2026-10-27-agg-session-moves/`.

| run | load | turn-0 prompt | later turns | moves | at turn |
|---|---|---|---|---|---|
| serial, short system text | 1 turn at a time, 300-token answers | 34-149 tokens | 48 | **8** | all turn 1 |
| serial, ~600-token system text | same | 629-744 tokens | 48 | **0** | - |
| concurrent, as the arms above | 3 sessions at a time, 1 100-token answers | 44-60 tokens | 48 | **3** | all turn 1 |

- **Once a session has stayed one turn, it never moved again**: no move at turn 2, 3 or 4
  in any run. Every move was at turn 1, and only when turn 0's prompt was short.
- A likely reading, not checked in the router's source: a turn-0 prompt this short fills
  few or no cache blocks the affinity filter can match, so turn 1 is placed by load
  alone; by turn 1 the prompt carries turn 0's answer and the affinity holds. With a
  ~600-token system prompt -- what real deployments send -- there were no moves at all.
- **A move at turn 1 costs little to recompute**: what the old pod holds is turn 0, under
  ~1 300 tokens here. The fetch pays where the shared prefix is long, which is exactly
  where the affinity already keeps the session in place.
- So at this scale **M4 is not worth a proxy hop on every aggregated request**. Load high
  enough for the `token-load-scorer` to override affinity was not tried (3 concurrent
  sessions on 3 A30s); that is the case to measure before revisiting.
- Attribution: serial runs read each pod's `vllm:request_success_total` around each turn
  (`session_moves.py`); the concurrent run matched the EPP's picks (`--v=4`, followed live
  since the log rotates within minutes) to the driver's send times in order
  (`moves_from_epp.py`). Five send pairs were under 50 ms apart -- two of them the three
  sessions' simultaneous turn 0 -- so a pairing there could swap two sessions' pods;
  all three moves involve sessions 0-2's turn 1, so the concurrent count is 3 +/- 2. The
  serial runs have no such ambiguity.

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
