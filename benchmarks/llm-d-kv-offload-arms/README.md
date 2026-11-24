# llm-d-kv-offload-arms: Certus against vLLM's own KV tiers, through llm-d

The scripts, analysis and run data behind the KV-offload comparison of 2026-10-29
to 2026-11-05. It compares Certus with vLLM's own offload tiers, with prefill and
decode split (P/D) and aggregated, through the llm-d router, under a workload
whose KV is 3.4 times larger than the memory tiers. The result is
`report/kv-offload-comparison.html`, a standalone page.

These scripts are **tied to the cluster the runs used** (node2 + node5, A30s,
Qwen3-8B). They are kept as run, apart from paths, so that what is here is what
produced the numbers. See [Cluster-specific checks](#cluster-specific-checks) for
what each hard-wired value protects before you point them at other hardware.

## Results

Time to first token (TTFT) for turns after the first, one run per configuration
(320 turns: 40 conversations x 8 turns), from `report/data2.json`:

| configuration | run | TTFT p50 | TTFT p90 | prompt KV cached |
|---|---|---|---|---|
| P/D · NIXL + Certus, with fetch | `fx2-1-arm1` | 0.64 s | 0.92 s | 95.3% |
| P/D · NIXL + Certus, no fetch | `fx2-2-arm2` | 0.99 s | 1.32 s | 79.5% |
| P/D · NIXL + vLLM CPU + filesystem | `ov-4-arm4` | 1.77 s | 2.71 s | 79.5% |
| P/D · NIXL + vLLM CPU + P2P | `ov-5-arm5` | 2.88 s | 4.21 s | 0% |
| P/D · vLLM CPU + P2P + filesystem (experimental) | `ov-5fs-arm5fs` | 1.64 s | 2.66 s | 95.3% |
| Aggregated · Certus, no fetch | `c13-3a-arm3a` | 0.27 s | 3.83 s | 77.7% |
| Aggregated · Certus, with fetch | `c13-3b-arm3b` | 0.28 s | 4.67 s | 93.2% |

The report gives bootstrap intervals for each comparison, the four-repeat
aggregated comparison, and the explanation of the experimental configuration's
gap.

## Layout

| path | what it is |
|---|---|
| `env.sh` | sourced by every script: `REPO`, `OUT_ROOT`, `NS`, `ROUTER` (see [Running](#running)) |
| `run_arm.sh` | one run: the fleet Prometheus scraper beside the long-doc driver, then the router's p2p decisions for the run's window |
| `run_sequence.sh` | the Certus configurations (arms 1, 2, 3, 3a, 3b), each from empty Certus tiers, with every check before it runs; made the `fx2-*` and `c13-*` runs |
| `run_sequence_vllm.sh` | the vLLM-tier configurations (arms 4, 5); made the `ov-*` runs. Older, with fewer checks (see [Runs done partly by hand](#runs-done-partly-by-hand)) |
| `load_watch.py` | logs each model server's KV load rate every 10 min during a run; warns on a slow loader |
| `gpu-hold.yaml` | placeholder pod that holds node5 GPU0 so prefill lands on GPU1 |
| `certus-server-numa-fc2a2b80-8G.yaml` | the Certus deployment the `fx2-*` and `c13-*` runs used: 4 NUMA-pinned instances, 8 GiB memory tier each. The image registry is left as `%%CERTUS_REGISTRY%%/%%CERTUS_REPO%%`, as in `deploy/k8s/certus-server-numa.yaml.tpl`; fill it in from `deploy/llm-d/site.env` |
| `analysis/compare_runs.py` | quick side-by-side of any run directories |
| `analysis/build_report_data.py` | computes `report/data2.json` from `results/` |
| `report/build_report.py` | writes `report/kv-offload-comparison.html` from `data2.json` |
| `experimental/vllm-p2p-fs/` | the experimental configuration: a patch to `install.sh`, three profiles, and its runners. **Not supported**, see its section |
| `results/<run>/` | the 15 runs the report reads: `turns.jsonl`, `summary.json`, `prom.log.gz`, `p2p.log`, `pods.txt`, and per-tier `final-metrics/` for the experimental runs |
| `results/ov-5fs-probe-mslogs/` | model-server logs of the probe run with P2P debug logging (gzipped), which the report's fetch-timeline numbers come from |
| `results/sequence-logs/` | the sequence logs for those runs: every check, install and warm-up as it happened |

The run data is as recorded, except that node names in `pods.txt` are shortened to `node2`/`node5`.

To regenerate the report from the committed runs:

```bash
python3 -I analysis/build_report_data.py   # writes report/data2.json
python3 -I report/build_report.py          # writes report/kv-offload-comparison.html
```

Both reproduce the committed files byte for byte.

## The configurations

| arm | configuration | install | then |
|---|---|---|---|
| 1 | P/D · NIXL + Certus, with fetch | `PD=1 WITH_CERTUS=1 RDMA=1` | decode publishes KV events, so the router names decode as prefill's fetch source |
| 2 | P/D · NIXL + Certus, no fetch | the same | `CERTUS_KV_EVENTS` removed from decode, so no fetch source is ever named |
| 3 | aggregated · Certus, prompts only | `WITH_CERTUS=1 FETCH=1` | `CERTUS_OFFLOAD_PROMPT_ONLY=true`, no KV events. Dropped from the report |
| 3a | aggregated · Certus, no fetch | the same | answers stored, no KV events |
| 3b | aggregated · Certus, with fetch | the same | answers stored, KV events on, so a session that moves fetches its history |
| 4 | P/D · NIXL + vLLM CPU + filesystem | `PD=1 RDMA=1 TIERED=1` | drives on RAID 0 + XFS (`deploy/llm-d/tiered-disks.sh fs`), Certus not running |
| 5 | P/D · NIXL + vLLM CPU + P2P | `PD=1 RDMA=1 OFFLOAD=1` | Certus not running |
| 5fs | P/D · vLLM CPU + P2P + filesystem | experimental, below | drives on RAID 0 + XFS |

The workload is the same for every arm: `benchmarks/kv-offload-replay/longdoc_router.py`
with 40 documents of 4,000 tokens, 8 questions each, 2,000-token answers
(`--ignore-eos`), 6 in flight, interleaved order. The seed is `400 + <arm>` in
`run_sequence.sh`, so each arm sees the same documents across repeats. The
vLLM-tier and experimental runs used seeds 404 and 435.

## Running

```bash
# Certus arms, each from empty tiers, as the fx2 runs were made
OUT_ROOT=~/kv-offload-arms-runs ARMS="1 2" ./run_sequence.sh
# aggregated repeats, alternating so drift falls on both
PREFIX=c13 ARMS="3a 3b" REPEATS="2 3 4" ./run_sequence.sh
# vLLM tiers: drives on RAID 0 + XFS first, Certus stopped
ARMS=4 ./run_sequence_vllm.sh
```

`env.sh` sets the defaults, and the environment overrides any of them:

| variable | default | meaning |
|---|---|---|
| `REPO` | this checkout | where `deploy/llm-d/install.sh` and `benchmarks/` come from. Every run here used tag `backup/kv-offload-arms-tree` (`ac42370a`). Tag `backup/kv-offload-arms-fetch` (`63be408a`) is the snapshot behind the earlier aggregated runs (`fx-3*`, `rp-*`), which the report no longer cites. On the day this was committed, `install.sh` and `longdoc_router.py` were unchanged between that tag and `unstable` |
| `OUT_ROOT` | `~/kv-offload-arms-runs` | run directories, warm-ups, install logs and the sequence log. `run_arm.sh` **replaces** its run directory |
| `NS` | `llm-d-quickstart` | llm-d namespace |
| `ROUTER` | `http://node2:30080` | router NodePort |
| `PREFIX` | `fx2` | `run_sequence.sh` run-name prefix |
| `CERTUS_TAG` | `fc2a2b80` | the Certus image every instance must run |
| `PREFILL_NODE` | `node5` | short name of the node prefill is pinned to; `pin_pd` looks up its `kubernetes.io/hostname` label |

Before a run, deploy Certus with `certus-server-numa-fc2a2b80-8G.yaml` for the
Certus arms, or stop it and run `tiered-disks.sh fs` on each node for the vLLM
arms. `run_sequence.sh` restarts Certus itself before every arm.

## Cluster-specific checks

These checks are what made the runs comparable. Each one caught an invalid run
at least once. They are hard-wired to this cluster; change them knowingly.

| check | where | protects against |
|---|---|---|
| every Certus instance pinned all 4 of its NUMA-local drives, `Restricting to 4 PCI device(s)` | `certus_drives_ok`, `fresh_certus` | the two-instance drive race: one instance held the other's drives, which loaded at a quarter speed for the whole run. Fixed in Certus `fc2a2b80`, still checked |
| tiers read 0 bytes after the restart | `fresh_certus` | an arm starting with an earlier arm's KV |
| image `certus:$CERTUS_TAG`, `batched piece copies on` | `fresh_certus` | a run on the wrong build |
| prefill on node5 GPU1 (`A1:00`) | `pin_pd`, `gpu-hold.yaml` | prefill on a different GPU between arms (GPU0 and GPU1 sit on different NUMA nodes) |
| `GPU KV cache size: 37,184` on every model server, 4 model servers | `record_pods` | a different GPU KV budget changes how much spills to the tiers |
| routing sidecar at llm-d-router `ddabbfd0` (pinned by digest) | `check_sidecars` | `:main` dropped the router's fetch source: 0 fetches, silently |
| `router-check.sh` passes | `run_arm` | the router failing open, which routes without prefix awareness |
| node2 `/home` > 32 GB free, no `DiskPressure` | `run_arm` | kubelet evicting pods mid-run below 23.8 GB |
| arm 1 and 3b made at least one Certus fetch; every other arm made none | end of `run_arm` | an arm that silently is not the configuration it claims |
| `load_watch.py` slow-load warnings | during the run | a pod loading at ~5 GB/s instead of ~20 for most of a run (happened once on node2, cause unknown) |

## Runs done partly by hand

- **`ov-5-arm5`** is a rerun (seed 435) after `run_sequence_vllm.sh` deployed arm 5.
  Prefill was pinned to node5 GPU1 as in the other arms (`pin_pd`), and
  `PYTHONHASHSEED` was unset after the hash-agreement check: with it pinned, as
  llm-d's P2P guide says, vLLM 0.30's router named no pull source. The profile no
  longer sets it, and the script now checks that nothing does. The run had 1 error
  in 316 turns.
- **`ov-4-arm4`** ran from `run_sequence_vllm.sh` as committed. The deploy starts
  prefill before decode so it gets its GPU.
- **`c13-*` and `fx2-*`** ran from `run_sequence.sh` with `PREFIX` c13 and fx2.
  c13 predates the image and allow-list checks and the disk check, which fx2 added.

## Runs not included

These runs are not here, and all of them are invalid or superseded:
`c13-1-arm1.newsidecar-nofetch-invalid` (sidecar `:main`, 0 fetches),
`c13-3b-arm3b.node2-slowload` (node2 loaded at ~5 GB/s, rerun),
`sm-arm1.failopen-invalid` (router failing open), `sm-arm1.fulltiers-nopull`,
the 1,000-token-answer runs, and the earlier `ov-1..3`, `r3-*`, `rp-*`, `fr-*`
runs on the image before the drive-race fix.

## Experimental: vLLM P2P + filesystem (`experimental/vllm-p2p-fs/`)

**Not a configuration vLLM or llm-d supports.** It puts vLLM's filesystem tier
beside its P2P tier, and rewrites its KV events so the router sees disk-resident
blocks the way it sees Certus's. That makes the fetch paths comparable with
arm 1 on equal routing terms. It answers the report's question "if vLLM's P2P
could fetch from disk too, would it match Certus?"

- `install.sh.patch` adds an `ARM5_FS=1|probe|swap` switch to `deploy/llm-d/install.sh`.
  `profiles/` holds the three overlays it selects, to be copied into
  `deploy/llm-d/profiles/a30/`:
  - `pd-offload-fs-rdma`: P2P, then the filesystem tier.
  - `pd-offload-fs-probe`: the same, with P2P debug logging.
  - `pd-offload-fs-swap`: the filesystem tier ahead of P2P.

  Each overlay loads `events_as_certus.py` before vLLM starts.
- Apply them to a **scratch checkout**, never this one, and set `ARM5_REPO` to it:
  ```bash
  git worktree add /tmp/arm5fs backup/kv-offload-arms-tree
  git -C /tmp/arm5fs apply "$PWD/experimental/vllm-p2p-fs/install.sh.patch"
  cp -r experimental/vllm-p2p-fs/profiles/* /tmp/arm5fs/deploy/llm-d/profiles/a30/
  ARM5_REPO=/tmp/arm5fs ARM5_FS=probe experimental/vllm-p2p-fs/deploy.sh
  experimental/vllm-p2p-fs/run_probe.sh   # or run.sh / run_swap.sh for the matching ARM5_FS
  ```
- `run.sh` made `ov-5fs-arm5fs`, `run_probe.sh` made `ov-5fs-probe`, and `run_swap.sh`
  made `ov-5fs-swap`. `p2p_timeline.py` pairs each fetch's steps across the
  prefill and decode logs.
