# connector-sweep

Runs the three KV-offload connector benches with one shared set of workload parameters, then renders a side-by-side HTML report:

| Arm | Bench | Backend |
|---|---|---|
| certus | [`certus-connector-bench`](../certus-connector-bench/README.md) (host) | `certus-server` over `/dev/shm/certus-shmq`, SPDK NVMe |
| cputier | [`cputier-connector-bench`](../cputier-connector-bench/README.md) (vLLM ≥ 0.30 image) | vLLM `TieringOffloadingSpec`: pinned CPU tier + `fs` disk tier |
| lmcache | [`lmcache-connector-bench`](../lmcache-connector-bench/README.md) (vLLM + LMCache image) | LMCache MP server: pinned-RAM L1 + `fs_native` L2 |

Every workload flag is given once and forwarded identically to each bench, so the arms can't drift. `--dram` sets all three memory tiers: certus-server's `--memory-tier-size`, cputier's `--cpu-bytes` and LMCache's L1. The arms run one after another because they share the GPU.

The `/profile-connector-benches` Claude Code skill drives these same scripts. It also asks for the disk tier, SSD count and tier size, and runs the preflight checks for you.

## Files

| File | Purpose |
|---|---|
| `run_compare.sh` | Starts certus-server, runs each arm, stops the server, then calls `make_report.py` |
| `make_report.py` | Joins the per-arm CSVs on result label. Writes the HTML: throughput and IOPS per arm, per-phase latency, and a parameter-parity table that flags any difference in red |

## Use

From the repo root:

```bash
benchmarks/connector-sweep/run_compare.sh \
    --out bench-results/connector-compare.html \
    --fs-tier /cache/cputier-bench-fs-tier \
    --l2-dir /cache/lmcache-bench-l2 \
    --device-pci 0000:86:00.0 \
    --dram 4G --working-set 4096
```

That is the run behind the 2026-10-30 comparison: one A30, 4 GiB memory tier on every arm, an 8 GiB mixed-eviction working set, and both file-based disk tiers on the `/cache` NVMe.

Artifacts go to `<out-without-.html>.d/`: `params.json` (records every arm's exact command line), `certus.csv`, `cputier.csv`, `lmcache.csv`, the three bench logs and `server.log`.

```bash
run_compare.sh --out <path> ... --skip-lmcache      # drop an arm (--skip-certus / --skip-cputier)
run_compare.sh --out <path> ... --report-only       # re-render the HTML from existing artifacts
run_compare.sh --help                               # every option and its default
```

## Before running

- **GPU free.** Stop a running `deploy/lmcache` stack first (`deploy/lmcache/teardown.sh`); its vLLM holds most of the GPU.
- **certus:** `target/release/certus-server` must be built from the current tree (`cargo build -p certus-server --release`). No other `certus-server` may be running, and the `--device-pci` SSDs must be bound to `vfio-pci`. Free 1 GiB hugepages cap the memory tier.
- **cputier:** `docker.io/vllm/vllm-openai:v0.30.0` must be in the default podman store.
- **lmcache:** `localhost/vllm-lmcache-bench:vllm0.30.0` must exist (`deploy/lmcache/build.sh`), and ports 5575 and 8095 must be free. The arm is skipped automatically for `--mode contention` and for `--tp` above 1.
- **Sizing:** `--working-set × --block-bytes` should exceed `--dram`, or the mixed-eviction phase never fills the tier. Keep `2 × working-set × block-bytes` within free GPU memory, because the certus bench allocates two buffers that size.
- **Disk tiers:** put `--fs-tier` and `--l2-dir` on the same local NVMe filesystem (not overlay or tmpfs, which reject `O_DIRECT`) for a fair cold-path comparison.

## Reading the results

- **Store figures differ in what they include.** cputier and lmcache store throughput leaves out the per-pass write-through to disk; each log prints the sustained figure that includes it. certus writes through to SSD inside the server.
- **Check the drop counts before calling a winner.** cputier and lmcache drop stores when their memory tier is full, which makes their mixed-eviction and contention rows look faster. The counts are in each log; certus waits instead.
- **lmcache counts differently.** A key is one LMCache chunk (256 tokens, `--block-bytes` bytes), and one lookup covers a request's whole prefix. Details are in the [lmcache-connector-bench README](../lmcache-connector-bench/README.md).
