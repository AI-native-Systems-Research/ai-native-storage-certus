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

Without disk-tier flags, the arms get matched hardware: certus uses the four SPDK drives `0000:61:00.0`–`0000:64:00.0` (the same set `benchmarks/vllm-serve` uses), and the cputier fs tier and the lmcache L2 both go under `/raid0`, a 4-drive XFS RAID0 on separate NVMe drives:

```bash
benchmarks/connector-sweep/run_compare.sh --out bench-results/connector-compare.html --dram 4G --working-set 4096
```

`--device-pci` (repeatable) or `--drive-count N` replaces the certus drive list; `--fs-tier` and `--l2-dir` move the file tiers.

`--out` is resolved against the directory you run from. Artifacts go to `<out-without-.html>.d/`: `params.json` (records every arm's exact command line), `certus.csv`, `cputier.csv`, `lmcache.csv`, the three bench logs and `server.log`.

```bash
run_compare.sh --out <path> ... --skip-lmcache      # drop an arm (--skip-certus / --skip-cputier)
run_compare.sh --out <path> ... --report-only       # re-render the HTML from existing artifacts
run_compare.sh --help                               # every option and its default
```

## Before running

- **GPU free.** Stop a running `deploy/lmcache` stack first (`deploy/lmcache/teardown.sh`); its vLLM holds most of the GPU.
- **certus:** the script runs `cargo build -p certus-server --release` itself (a no-op when current), so `target/release/certus-server` always matches the tree; an explicit `--server-bin` is used as-is. No other `certus-server` may be running, and the `--device-pci` SSDs must be bound to `vfio-pci`. Free 1 GiB hugepages cap the memory tier.
- **cputier:** `docker.io/vllm/vllm-openai:v0.30.0` must be in the default podman store.
- **lmcache:** `localhost/vllm-lmcache-bench:vllm0.30.0` must exist (`deploy/lmcache/build.sh`), and ports 5575 and 8095 must be free. The arm is skipped automatically for `--mode contention` and for `--tp` above 1.
- **Sizing:** `--working-set × --block-bytes` should exceed `--dram`, or the mixed-eviction phase never fills the tier. Keep `2 × working-set × block-bytes` within free GPU memory, because the certus bench allocates two buffers that size.
- **Disk tiers:** put `--fs-tier` and `--l2-dir` on the same local NVMe filesystem (not overlay or tmpfs, which reject `O_DIRECT`) for a fair cold-path comparison. The defaults do this: both live on `/raid0`, whose four drives are not the four certus drives, so no arm shares an SSD with another.

## What makes it a fair comparison

| Held equal | How |
|---|---|
| Workload | Every workload flag is given once and forwarded verbatim to all three benches |
| Memory tier | `--dram` sets certus `--memory-tier-size`, cputier `--cpu-bytes` and the lmcache L1 |
| KV layout | `--layers` (default 32, vLLM's default layer-outermost `LBHNC`) splits every block into the same number of GPU tensors on every arm: certus `--pieces-per-block`, cputier and lmcache `--layers`. `--layers 1` gives the contiguous `BLHNC` layout that `CertusConnector` requests |
| NUMA | Every arm's CPUs and memory are bound to the GPU's node (`--numa`, default the GPU's). certus runs under `numactl`. The podman arms inherit its memory policy and pin their CPUs with `taskset` inside the container, because rootless podman has no cpuset controller |
| Disk hardware | certus: 4 SPDK drives (61–64). cputier fs tier and lmcache L2: `/raid0`, an XFS RAID0 over 4 other NVMe drives, `O_DIRECT` |
| Disk I/O parallelism | `--io-threads` (default 16) sets cputier's fs read and write threads and lmcache's `fs_native` workers |
| Timing | Store rows exclude each pass's disk write-through drain (certus DRAM→SSD, cputier CPU→fs, lmcache L1→L2) and report the sustained figure including it (`sustained_gbs`). Cold rows empty the memory tier before *every* pass, outside the timed window, so every block is read back from disk |
| Proof of work | Every bench writes `store_drops`, `load_misses` and, on cold and mixed rows, `disk_read_bytes` to its CSV. certus-server is built with `rw-telemetry` so its SSD reads are counted |

`params.json` records all of it, and the report's parity table turns any mismatch red.

## Reading the results

- **⚠ rows are not like for like.** The report flags a row when an arm dropped stores, missed loads, or read less than half of a cold pass's bytes from disk. A backend that drops stores into a full tier (cputier and lmcache do; certus waits) does less work and looks faster. Read the flag before calling a winner.
- **Store rows have two figures.** The bar is the GPU→memory-tier store path. "incl. disk drain" under the label is the sustained rate including the write-through to disk, which is what a write-heavy workload sees.
- **The disk tiers are different designs on equal hardware.** certus drives raw NVMe through SPDK; cputier writes one file per block and lmcache one file per chunk, both on `/raid0`. lmcache also writes every chunk through to L2 as it is stored, while certus and cputier write through in the background.
- **lmcache counts differently.** A key is one LMCache chunk (256 tokens, `--block-bytes` bytes), and one lookup covers a request's whole prefix. Details are in the [lmcache-connector-bench README](../lmcache-connector-bench/README.md).
