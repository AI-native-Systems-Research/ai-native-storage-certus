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

From the repo root, with the defaults (8 GiB memory tier on every arm, `--mode compare`):

```bash
benchmarks/connector-sweep/run_compare.sh --out bench-results/connector-compare.html
```

That is the whole command. It builds and starts certus-server (rw-telemetry, the four SPDK
drives `0000:61:00.0`–`0000:64:00.0`), runs each arm in turn, stops the server and writes the
report. To compare at another tier size, add only `--dram`:

```bash
benchmarks/connector-sweep/run_compare.sh --out bench-results/compare-12g.html --dram 12G
```

`--dram` sets certus-server's `--memory-tier-size` (and the certus bench's `--tier-gib`),
cputier's `--cpu-bytes` and LMCache's L1, and the working set every arm moves is derived from
it: 2,948 blocks at 8G, 4,000 at 12G or more, 1,474 at 4G (prefix-miss: 128). Do not pass
`--num-blocks` unless you want to fix that number by hand. Nothing else needs to change.

What the default run expects (see "Before running"). An arm whose prerequisites are missing
fails, the other arms still run and the report is still written, but the script exits non-zero;
skip such an arm explicitly (`--skip-lmcache`, `--skip-cputier`, ...). Only the 3fs arm skips
itself, with a note, when there is no 3FS mount:

- certus: the four drives bound to `vfio-pci`, no other certus-server running, enough free
  1 GiB hugepages for `--dram`.
- cputier: `docker.io/vllm/vllm-openai:v0.30.0`, and its disk tier at `/raid0` (`--fs-tier DIR`
  elsewhere). Its store pass writes every pass through to that disk, so it needs room.
- lmcache: `localhost/vllm-lmcache-bench:vllm0.30.0` (`deploy/lmcache/build.sh`); its L2 at
  `/raid0` (`--l2-dir DIR`).
- 3fs: a 3FS mount at `/mnt/3fs` and the vLLM + 3FS image; or `--h3fs-mock` to run the code
  path on vLLM's file-backed mock (not a 3FS measurement; the report says so).

Other common choices:

```bash
run_compare.sh --out <path> --mode contention         # loads + stores together (certus and cputier only)
run_compare.sh --out <path> --skip-lmcache            # drop an arm (--skip-certus / --skip-cputier / --skip-3fs)
run_compare.sh --out <path> --server-bin PATH         # a prebuilt certus-server (rw-telemetry, for the cold disk check)
run_compare.sh --help                                 # every option and its default
```

The 2026-10-30 comparison used an older shape (one A30, `--dram 4G --working-set 4096
--num-blocks 1024 --layers 1`, both file tiers on `/cache`, one batch at a time); its numbers
are not comparable with these defaults (8G tier, 32 layers, 64-block jobs x 4 in flight).

`--device-pci` (repeatable) or `--drive-count N` replaces the certus drive list; `--fs-tier` and `--l2-dir` move the file tiers.

**Modes.** `--mode compare` (the default) runs, on every arm, only the modes the four benches drive the same way: `default` (store, warm, cold; mixed eviction on the arms with a memory tier) and then `prefix-miss`, one bench call each. `--mode default`, `--mode prefix-miss` or `--mode contention` runs one of them (contention: certus and cputier only). `scheduler-step` is not compared: certus-connector-bench's scheduler-step runs vLLM 0.30's scheduler (one load job per request, `--inflight` outstanding, one reap per tick), the other three benches' still merge a step's loads and wait for them inside the step, so the row would compare two harnesses rather than connectors. Run it with certus-connector-bench directly. Each bench's own `--mode all` is unchanged.

**Certus cold rows need a telemetry server** to show their disk reads: the default (the tree's own build, which run_compare.sh builds with `--features rw-telemetry`) has it; a `--server-bin` must be built that way too. In a fresh git worktree that build needs the ignored link `kernel/modules/gdrcopy/src/libgdrapi.so -> libgdrapi.so.2`, which the main checkout has; without it the link fails with `unable to find library -lgdrapi`.

`--out` is resolved against the directory you run from. Artifacts go to `<out-without-.html>.d/`: `params.json` (records every arm's exact command line), `certus.csv`, `cputier.csv`, `lmcache.csv`, the three bench logs and `server.log`.

```bash
run_compare.sh --out <path> ... --skip-lmcache      # drop an arm (--skip-certus / --skip-cputier / --skip-3fs)
run_compare.sh --out <path> ... --h3fs-mock         # 3fs arm on the mock client (no cluster; code path only)
run_compare.sh --out <path> ... --report-only       # re-render the HTML from existing artifacts (keeps the run's params.json)
run_compare.sh --out <path> ... --latex             # also write the throughput table as LaTeX: <path>.tex
run_compare.sh --help                               # every option and its default
```

The LaTeX table (`--latex`, or `make_report.py --latex PATH`) is a `booktabs` `table` with one row per result and one column per arm, in GB/s. Store rows get an extra "incl. disk drain" row. The fastest arm in each row is in bold. A value from an arm that skipped work in that row (dropped stores, load misses, or a cold pass that barely touched disk) carries a dagger, is never bold, and is explained in a footnote under the table. A second table (`tab:connector-sweep-latency`) gives each row's per-job latency (submit to reap, including the wait behind the other jobs in flight), p50 / p99 in ms, per arm. Cold loads get two rows, because cputier and lmcache read from disk while the lookup waits, before the transfer, while certus and 3fs read during the transfer. Add `\usepackage{booktabs}` to the preamble.

## Memory tier size and `--num-blocks`

`--num-blocks` is the working set: the distinct blocks (keys) the store, warm and cold passes
cycle through. The warm pass only measures memory-tier loads while the whole set stays in the
tier, so the set is sized from the tier.

**How it is computed when not given.** In the default and contention modes:

```
num_blocks = min(4000, 0.9 x tier_bytes x eviction_threshold / block_bytes)
```

90% of what the tier holds before it starts evicting (leaving room for the bench's own
bookkeeping), capped at 4,000 blocks (7.8 GiB at 2 MiB): beyond that throughput no longer
changes and each pass only takes longer. The other modes (prefix-miss, scheduler-step,
pattern) use 128 at any tier size.

| tier (`--dram`) | usable (x 0.8) | `--num-blocks` (default, contention) | cold and mixed-eviction fill |
|---|---|---|---|
| 4G | 3.2 GiB | 1,474 | 1,638 |
| 8G (default) | 6.4 GiB | 2,948 | 3,276 |
| 12G | 9.6 GiB | 4,000 | 4,915 |
| 32G | 25.6 GiB | 4,000 (the cap) | 13,107 |

(2 MiB blocks, eviction threshold 0.8.) Each bench computes it from its own tier: certus from
`--tier-gib` x `--eviction-threshold`, cputier from `--cpu-bytes`, LMCache from the L1 its
server reports; 3FS has no tier and takes the 8G figure. `run_compare.sh` computes it once
from `--dram` and passes it to every arm, so every arm moves the same keys. The cold pass
(`--cold-tier-state full`) and the mixed-eviction pass fill the tier to its eviction threshold
before they start, from the same numbers, so they evict at any tier size.

**To change the tier size, change only the size**: `run_compare.sh --dram 12G`; or, running a
bench by hand, the server's `--memory-tier-size 12G` with `--tier-gib 12` (cputier
`--cpu-bytes 12G` and `CPU_BYTES=12G`, LMCache `L1_GB=12`). Everything above follows. The
certus bench cannot read the server's tier size, so tell it: a wrong `--tier-gib` is not
detected, and every run prints `Tier assumed: ...` first.

**If `--num-blocks` is set higher than the tier holds.** Only the warm pass and contention's
loads need the set resident; store and cold do not (cold empties the tier before every pass).

| flags | store, cold | warm, contention loads |
|---|---|---|
| none (default) | — | **refused** before anything runs, naming the largest `--num-blocks` that fits |
| `--no-warm` | run at any size | warm skipped (contention still refused: its loads need the set resident) |
| `--allow-warm-overflow` | run at any size | run, and **marked**: label `OVERFLOW: set exceeds tier, not warm`, CSV `warm_overflow=1` and the bytes that came back from disk (`disk_read_bytes`), ⚠ in the report |

Why refuse rather than warn: an overflowing warm pass still prints a clean-looking number. On a
4 GiB certus tier with 8,000 blocks it reported 24.6 GB/s with 0 misses, and 129.6 GB of it was
read from the drives (certus demotes evicted blocks to SSD and promotes them back, so the loads
succeed, slowly or not); cputier in the same situation reported 0.74 GB/s, read back from its
disk tier. Neither is a memory-tier load. A warning in a log is easy to miss; the refusal, or
the mark on the row with `--allow-warm-overflow`, is not. The cutoff is 90% of the usable tier
(certus, cputier, LMCache alike); 3FS has no tier, so nothing is refused there.

## Before running

- **GPU free.** Stop a running `deploy/lmcache` stack first (`deploy/lmcache/teardown.sh`); its vLLM holds most of the GPU.
- **certus:** the script runs `cargo build -p certus-server --release` itself (a no-op when current), so `target/release/certus-server` always matches the tree; an explicit `--server-bin` is used as-is. No other `certus-server` may be running, and the `--device-pci` SSDs must be bound to `vfio-pci`. Free 1 GiB hugepages cap the memory tier.
- **cputier:** `docker.io/vllm/vllm-openai:v0.30.0` must be in the default podman store.
- **lmcache:** `localhost/vllm-lmcache-bench:vllm0.30.0` must exist (`deploy/lmcache/build.sh`), and ports 5575 and 8095 must be free. The arm is skipped automatically for `--mode contention` and for `--tp` above 1.
- **Sizing:** nothing to set: `--num-blocks` is derived from `--dram` so it fits every arm's tier, and the mixed-eviction pass fills the tier before it starts, so it evicts at any `--working-set`. A `--num-blocks` given by hand must fit every arm's tier (each bench refuses one over 90% of it; certus: of `--dram` × `--eviction-threshold`), unless `--no-warm` or `--allow-warm-overflow`. The benches size their GPU regions by the jobs in flight (`--inflight` × `--job-blocks` plus the store slots), not by the working set.
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
