# connector-sweep

Runs the four KV-offload connector benches with one shared set of workload parameters, then renders a side-by-side HTML report:

| Arm | Bench | Backend |
|---|---|---|
| certus | [`certus-connector-bench`](../certus-connector-bench/README.md) (host) | `certus-server` over `/dev/shm/certus-shmq`, SPDK NVMe |
| cputier | [`cputier-connector-bench`](../cputier-connector-bench/README.md) (vLLM ≥ 0.30 image) | vLLM `TieringOffloadingSpec`: pinned CPU tier + `fs` disk tier |
| lmcache | [`lmcache-connector-bench`](../lmcache-connector-bench/README.md) (vLLM + LMCache image) | LMCache MP server: pinned-RAM L1 + `fs_native` L2 |
| 3fs | [`3fs-connector-bench`](../3fs-connector-bench/README.md) (vLLM + 3FS image) | vLLM `HF3FSKVConnector`: 3FS via USRBIO, **no memory tier** |

Every workload flag is given once and forwarded identically to each bench, so the arms can't drift. `--dram` sets the three memory tiers: certus-server's `--memory-tier-size`, cputier's `--cpu-bytes` and LMCache's L1. The 3fs arm has no memory tier, so it's a disk-only baseline with no mixed-eviction row. The arms run one after another because they share the GPU.

The 3fs arm needs a live 3FS cluster mounted at `/mnt/3fs` (`sudo deploy/3fs/run.sh`) and the vLLM + 3FS image. Without the mount it's skipped with a note in the report. `--h3fs-mock` runs it instead on vLLM's file-backed mock client on `/raid0`. That checks the code path, but it's **not** a 3FS measurement, and the report shows a banner saying so.

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
    --dram 4G --working-set 4096 --num-blocks 1024 --layers 1
```

That is close to the run behind the 2026-10-30 comparison (one A30, 4 GiB memory tier on every arm, an 8 GiB mixed-eviction working set, both file-based disk tiers on the `/cache` NVMe), which ran before the benches kept jobs in flight; its numbers are not comparable with today's defaults (12G tier, 32 layers, 64-block jobs x 4 in flight).

Without disk-tier flags, the arms get matched hardware: certus uses the four SPDK drives `0000:61:00.0`–`0000:64:00.0` (the same set `benchmarks/vllm-serve` uses), and the cputier fs tier and the lmcache L2 both go under `/raid0`, a 4-drive XFS RAID0 on separate NVMe drives:

```bash
benchmarks/connector-sweep/run_compare.sh --out bench-results/connector-compare.html --working-set 8192
```

`--device-pci` (repeatable) or `--drive-count N` replaces the certus drive list; `--fs-tier` and `--l2-dir` move the file tiers.

**Modes.** `--mode compare` (the default) runs, on every arm, only the modes the four benches drive the same way: `default` (store, warm, cold; mixed eviction on the arms with a memory tier) and then `prefix-miss`, one bench call each. `--mode default`, `--mode prefix-miss` or `--mode contention` runs one of them (contention: certus and cputier only). `scheduler-step` is not compared: certus-connector-bench's scheduler-step runs vLLM 0.30's scheduler (one load job per request, `--inflight` outstanding, one reap per tick), the other three benches' still merge a step's loads and wait for them inside the step, so the row would compare two harnesses rather than connectors. Run it with certus-connector-bench directly. Each bench's own `--mode all` is unchanged.

**Changing the tier size.** Give only `--dram` (e.g. `--dram 12G`): it sets every arm's tier and the working set derived from it. `--num-blocks` is only needed to fix a number by hand.

**Small memory tier, large `--num-blocks`.** Every bench refuses a warm working set its tier cannot hold (the warm pass would not be warm). `--no-warm` skips the warm pass on every arm, so store and cold compare at any size; `--allow-warm-overflow` runs it anyway, and the report flags those rows (⚠, with the bytes that came back from disk) instead of comparing them as warm.

**Certus cold rows need a telemetry server** to show their disk reads: give `--server-bin` a certus-server built with `--features rw-telemetry` (the default, the tree's own build, is built that way). In a fresh git worktree that build needs the ignored link `kernel/modules/gdrcopy/src/libgdrapi.so -> libgdrapi.so.2`, which the main checkout has; without it the link fails with `unable to find library -lgdrapi`.

`--out` is resolved against the directory you run from. Artifacts go to `<out-without-.html>.d/`: `params.json` (records every arm's exact command line), `certus.csv`, `cputier.csv`, `lmcache.csv`, the three bench logs and `server.log`.

```bash
run_compare.sh --out <path> ... --skip-lmcache      # drop an arm (--skip-certus / --skip-cputier / --skip-3fs)
run_compare.sh --out <path> ... --h3fs-mock         # 3fs arm on the mock client (no cluster; code path only)
run_compare.sh --out <path> ... --report-only       # re-render the HTML from existing artifacts (keeps the run's params.json)
run_compare.sh --out <path> ... --latex             # also write the throughput table as LaTeX: <path>.tex
run_compare.sh --help                               # every option and its default
```

The LaTeX table (`--latex`, or `make_report.py --latex PATH`) is a `booktabs` `table` with one row per result and one column per arm, in GB/s. Store rows get an extra "incl. disk drain" row. The fastest arm in each row is in bold. A value from an arm that skipped work in that row (dropped stores, load misses, or a cold pass that barely touched disk) carries a dagger, is never bold, and is explained in a footnote under the table. A second table (`tab:connector-sweep-latency`) gives each row's per-job latency (submit to reap, including the wait behind the other jobs in flight), p50 / p99 in ms, per arm. Cold loads get two rows, because cputier and lmcache read from disk while the lookup waits, before the transfer, while certus and 3fs read during the transfer. Add `\usepackage{booktabs}` to the preamble.

## Before running

- **GPU free.** Stop a running `deploy/lmcache` stack first (`deploy/lmcache/teardown.sh`); its vLLM holds most of the GPU.
- **certus:** the script runs `cargo build -p certus-server --release` itself (a no-op when current), so `target/release/certus-server` always matches the tree; an explicit `--server-bin` is used as-is. No other `certus-server` may be running, and the `--device-pci` SSDs must be bound to `vfio-pci`. Free 1 GiB hugepages cap the memory tier.
- **cputier:** `docker.io/vllm/vllm-openai:v0.30.0` must be in the default podman store.
- **lmcache:** `localhost/vllm-lmcache-bench:vllm0.30.0` must exist (`deploy/lmcache/build.sh`), and ports 5575 and 8095 must be free. The arm is skipped automatically for `--mode contention` and for `--tp` above 1.
- **Sizing:** `--working-set × --block-bytes` should exceed `--dram`, or the mixed-eviction phase never fills the tier (8G at 2 MiB: above 4,096; the mixed pass now fills the tier first, so it evicts at any size). `--num-blocks` (the warm working set) must fit every arm's tier: each bench refuses one over 90% of its tier (certus: of `--dram` × `--eviction-threshold`). The benches size their GPU regions by the jobs in flight (`--inflight` × `--job-blocks` plus the store slots), not by the working set.
- **Disk tiers:** put `--fs-tier` and `--l2-dir` on the same local NVMe filesystem (not overlay or tmpfs, which reject `O_DIRECT`) for a fair cold-path comparison. The defaults do this: both live on `/raid0`, whose four drives are not the four certus drives, so no arm shares an SSD with another.

## What makes it a fair comparison

| Held equal | How |
|---|---|
| Workload | Every workload flag is given once and forwarded verbatim to all three benches |
| Memory tier | `--dram` sets certus `--memory-tier-size`, cputier `--cpu-bytes` and the lmcache L1. 3fs has none, and the parity table shows it as "none" |
| KV layout | `--layers` (default 32, vLLM's default layer-outermost `LBHNC` for Llama-3-8B) splits every block into the same number of GPU tensors on every arm; certus takes `--layers` as its `--pieces-per-block`. `--layers 1` is the contiguous `BLHNC` layout `CertusConnector` can request |
| Jobs in flight | `--job-blocks` (64), `--inflight` (4) and `--step-us` (100) go to every arm, and `--store-job-blocks` / `--store-inflight` when given: each bench's default store, warm and cold passes and its contention mode keep that many jobs outstanding and reap them with one non-blocking poll per scheduler tick, as vLLM 0.30 does. Every CSV row records the five, and the parity table checks them. A connector that gains little from jobs in flight shows it: cputier chains its transfers on the GPU, LMCache's server runs `--max-workers` 1 by default, 3FS has one worker thread per direction |
| NUMA | Every arm's CPUs and memory are bound to the GPU's node (`--numa`, default the GPU's). certus runs under `numactl`. The podman arms inherit its memory policy and pin their CPUs with `taskset` inside the container, because rootless podman has no cpuset controller |
| Disk hardware | certus: 4 SPDK drives (61–64). cputier fs tier and lmcache L2: `/raid0`, an XFS RAID0 over 4 other NVMe drives, `O_DIRECT`. 3fs: wherever `deploy/3fs` put its storage targets (`DATA_DIR`, default `/cache/3fs`). Start it with `DATA_DIR` on `/raid0` to match |
| Disk I/O parallelism | `--io-threads` (default 16) sets cputier's fs read and write threads, lmcache's `fs_native` workers and 3fs's USRBIO I/O jobs (`hf3fs_client_numjobs`) |
| Memory tier sizing | `--dram` 8G with `--eviction-threshold` 0.8 (certus-server's `--memory-tier-size`, the certus bench's `--tier-gib` / `--eviction-threshold`, cputier's `--cpu-bytes` and `CPU_BYTES`, LMCache's L1). `--num-blocks` is derived from it when not given, by each bench's rule (default and contention: min(4000, 90% of dram x 0.8 / block bytes), 2,948 at 8G; other modes 128) and passed to every arm, 3FS included, so every arm moves the same keys. To compare at another tier size change only `--dram` |
| Timing | Store rows exclude each pass's disk write-through drain (certus DRAM→SSD, cputier CPU→fs, lmcache L1→L2) and report the sustained figure including it (`sustained_gbs`). Cold rows empty the memory tier before *every* pass, outside the timed window, so every block is read back from disk |
| Proof of work | Every bench writes `store_drops`, `load_misses` and, on cold and mixed rows, `disk_read_bytes` to its CSV. certus-server is built with `rw-telemetry` so its SSD reads are counted |

`params.json` records all of it, and the report's parity table turns any mismatch red.

## Reading the results

- **⚠ rows are not like for like.** The report flags a row when an arm dropped stores, missed loads, or read less than half of a cold pass's bytes from disk. A backend that drops stores into a full tier (cputier and lmcache do; certus waits) does less work and looks faster. Read the flag before calling a winner.
- **Store rows have two figures.** The bar is the GPU→memory-tier store path. "incl. disk drain" under the label is the sustained rate including the write-through to disk, which is what a write-heavy workload sees.
- **The disk tiers are different designs on equal hardware.** certus drives raw NVMe through SPDK; cputier writes one file per block and lmcache one file per chunk, both on `/raid0`. lmcache also writes every chunk through to L2 as it is stored, while certus and cputier write through in the background.
- **lmcache counts differently.** A key is one LMCache chunk (256 tokens, `--block-bytes` bytes), and one lookup covers a request's whole prefix. Details are in the [lmcache-connector-bench README](../lmcache-connector-bench/README.md).
