---
name: profile-connector-benches
description: Run certus-connector-bench, cputier-connector-bench and lmcache-connector-bench with identical workload parameters, then write an HTML report comparing the three KV-offload backends side by side (throughput, IOPS, per-phase latency, parameter parity).
argument-hint: "<output-html-path>"
---

# profile-connector-benches

Benchmark the three KV-offload connector backends with the same workload and present
them side by side:

| Arm | Bench | Backend |
|---|---|---|
| certus | `benchmarks/certus-connector-bench/bench_connector_lifecycle.py` (host) | `certus-server` over `/dev/shm/certus-shmq`, SPDK NVMe |
| cputier | `benchmarks/cputier-connector-bench/run-in-container.sh` (vLLM ≥ 0.30 image) | vLLM `TieringOffloadingSpec`: pinned CPU tier + `fs` disk tier |
| lmcache | `benchmarks/lmcache-connector-bench/run-in-container.sh` (vLLM + LMCache image) | LMCache MP server: pinned-RAM L1 + `fs_native` L2 (the wrapper starts and stops the server) |

All work is done by `benchmarks/connector-sweep/run_compare.sh` (it also runs
standalone; see that directory's README). It takes every
workload flag **once** and forwards the same list to every bench, so the arms
cannot drift. The DRAM tier size `--dram` becomes certus-server's
`--memory-tier-size`, cputier's `--cpu-bytes` (and the container's
`CPU_BYTES`) and the LMCache server's L1 (`--l1-size-gb`). `make_report.py` then checks parity from the columns each bench
actually wrote to its CSV and flags any difference in red.

## Argument

- `<output-html-path>` (required): where to write the HTML report, e.g.
  `bench-results/connector-compare.html`. If it is missing, ask for it. Raw
  artifacts (params.json, the three CSVs, the three bench logs, server.log) go
  to the sibling directory `<path-without-.html>.d/`.

The disk-tier path (cputier fs tier and lmcache L2), the certus SSD count and the
memory tier size are asked for interactively (step 3). Everything else has working defaults. Pass through any extra flags the user
gives (see `run_compare.sh --help`): `--mode` (default `all`), `--bs`,
`--num-blocks`, `--block-bytes`, `--min-duration`, `--warmup`, `--working-set`,
`--pipeline-depth`, `--direction`, `--hit-ratio`, `--requests-per-step`,
`--num-sessions`, `--lookup-order`, `--gpu`, `--tp`, `--dram`; certus-only
`--device-pci`/`--drive-count`, `--eviction-threshold`, `--workers`,
`--channels`; cputier-only `--fs-tier`, `--image`; lmcache-only `--l2-dir`,
`--lmcache-image`, `--chunk-size`. `--skip-certus` / `--skip-cputier` /
`--skip-lmcache` drop an arm.

## Steps for Claude

1. **Resolve the output path** from the argument. Make it absolute relative to the
   repo root if it is relative.

2. **Gather facts** to offer as answer options in step 3:
   - GPU: `nvidia-smi --query-gpu=index,name,memory.used --format=csv` and its
     NUMA node (`nvidia-smi topo -m`).
   - NVMe SSDs for certus: `lspci -Dnd ::0108`, with each device's NUMA node
     (`/sys/bus/pci/devices/<addr>/numa_node`) and driver (`basename $(readlink
     /sys/bus/pci/devices/<addr>/driver)`; SPDK needs `vfio-pci` or `uio_pci_generic`).
   - Hugepages: `grep -i huge /proc/meminfo` (free hugepage bytes cap the
     certus memory tier).
   - Free host RAM: `free -g` (caps the cputier CPU tier, which lives in the
     container's `/dev/shm`, and the lmcache L1, which is pinned host RAM;
     the arms run one at a time, so they don't add up).
   - Candidate disk-tier filesystems (cputier fs tier, lmcache L2): `df -hT` mounts that are local
     block devices (xfs/ext4), and for each the backing device type
     (`lsblk -s -o NAME,TYPE,TRAN,MODEL <source>`) so the user can see which are NVMe.

3. **Ask the user** with one AskUserQuestion call holding these three questions
   (skip any the invocation already answered via `--fs-tier`/`--l2-dir`,
   `--device-pci` / `--drive-count`, or `--dram`):
   - **Disk-tier filesystem** for cputier's fs tier and lmcache's L2 — options:
     the default (`/raid0/cputier-bench-fs-tier` and `/raid0/lmcache-bench-l2`;
     state the filesystem and backing device, e.g. "xfs on a 4-NVMe md RAID0"), plus up to two
     other candidate mounts from step 2, preferring NVMe-backed ones. For a
     mount `<m>`, pass `--fs-tier <m>/cputier-bench-fs-tier --l2-dir
     <m>/lmcache-bench-l2`, so both disk tiers sit on the same device. Note that
     an NVMe-backed disk tier gives the fairest cold-path comparison with
     certus, and that it must not be overlay storage (no `O_DIRECT`). "Other"
     lets the user type a path.
   - **Number of certus SSDs** — options `1`, `2`, ... up to the number of
     SPDK-bound NVMe devices found (max 4 options; recommend `4`, the script's
     default of `0000:61:00.0`–`0000:64:00.0`, which matches the 4-drive `/raid0`
     used by the file tiers). Then pick that many devices, GPU-NUMA
     node first, and pass each as `--device-pci`. Warn if any chosen SSD is on
     a different NUMA node from the GPU.
   - **Memory tier size** (applies to every arm: certus `--memory-tier-size`,
     cputier `--cpu-bytes`, lmcache L1) — options `4G` (recommended, the default),
     `8G`, `16G`, filtered to sizes that fit both the free hugepages and free
     RAM (less ~4G headroom for the container's `/dev/shm`). Remind the user
     that `--working-set` (default 512 × 2 MiB = 1 GiB) must exceed it for the
     mixed-eviction phase to apply pressure, and offer to scale `--working-set`
     with it (e.g. 2 × tier ÷ `--block-bytes`), passed to every arm.
     **Cap it by GPU memory:** the certus bench allocates two GPU buffers of
     `max(num_blocks, working_set) × block_bytes` each at startup, so
     `2 × working_set × block_bytes` must fit in the GPU's free memory with
     ~1 GiB to spare, or the certus arm dies with CUDA OOM before writing any
     results (8192 × 2 MiB blocks needs 32 GiB; an A30 has 24). If no working
     set both fits the GPU and exceeds the tier, say so and only offer tier
     sizes that leave room for one.

   If the chosen disk-tier directories do not exist, create them (`mkdir -p`).

4. **Preflight** and tell the user what will run, with the answers from step 3:
   - GPU free (no memory in use).
   - certus arm: `target/release/certus-server` exists and is newer than the
     last commit touching `apps/certus-server`, `components` or `lib` (otherwise
     rebuild: `cargo build -p certus-server --release`); it runs as the current
     user, no sudo. Make sure no other `certus-server` is running (it would own
     `/dev/shm/certus-shmq`).
   - cputier arm: `podman image exists docker.io/vllm/vllm-openai:v0.30.0`
     (or `--image`).
   - lmcache arm: `podman image exists localhost/vllm-lmcache-bench:vllm0.30.0`
     (or `--lmcache-image`; build it with `deploy/lmcache/build.sh`, about a
     minute). Ports 5575 and 8095 must be free (the bench's private LMCache
     server). A running `deploy/lmcache` stack on 5555/8080 doesn't conflict,
     but its vLLM holds most of the GPU, so the GPU-free check above catches it.
   - The lmcache arm is skipped automatically for `--mode contention` (it has
     no contention mode) and `--tp` > 1 (single-rank); tell the user if that
     applies.
   - If a prerequisite for one arm is missing, offer `--skip-certus`,
     `--skip-cputier` or `--skip-lmcache` instead of aborting; the report then
     shows that arm as having no results.

5. **Launch** in the background (a `--mode all` run is several minutes per arm;
   the arms run one after the other because they share the GPU):
   ```bash
   benchmarks/connector-sweep/run_compare.sh --out <output-html-path> \
       --fs-tier <answer> --l2-dir <answer> --device-pci <addr> [--device-pci <addr> ...] \
       --dram <answer> [--working-set N] [flags]
   ```
   Wait for it to finish. The script stops certus-server (SIGTERM, then SIGKILL
   after 8 s) on exit, including on failure; the lmcache wrapper removes its
   own server container and L2 run directory.

6. **Report.** Give the user the HTML path and a short summary taken from the
   CSVs in `<path>.d/`: which backend wins each result row and by how much,
   and anything notable in the per-phase latency (for example cputier's
   `lookup_wait` on the cold path, cputier store drops in its log, or
   lmcache's store drops in its mixed-eviction row). If
   `make_report.py` printed a parameter mismatch warning, or an arm failed,
   say so first and point at the log under `<path>.d/`. To re-render the HTML
   from existing artifacts, run `run_compare.sh --out <path> --report-only`
   with the same flags.

## What is and is not held equal

- Equal (forwarded identically): mode, `bs`, `num_blocks`, `block_bytes`,
  `min_duration`, `warmup`, `working_set`, `pipeline_depth`, `direction`,
  `hit_ratio`, `requests_per_step`, `num_sessions`, `lookup_order`, `tp`, GPU(s),
  DRAM tier size.
- Inherently different: the disk tier (SPDK raw NVMe vs one file per block vs
  one file per LMCache chunk), and options only one bench has: certus
  `--workers`, `--channels`, eviction threshold, `--prefix-sharing` (left at its
  default, `shared`, which is how cputier behaves); cputier `--layers`,
  `--blocks-per-chunk`, fs thread counts; lmcache `--chunk-size`, L2 I/O threads
  (all left at their defaults). The report lists these under "Backend-specific
  settings".
- lmcache semantics differ (see `benchmarks/lmcache-connector-bench/README.md`):
  a key is one 256-token chunk of `--block-bytes` bytes and keys chain through
  the prefix; one lookup answers a request's whole prefix; there is no
  `touch`, so `--lookup-order` changes nothing; and its L1 evicts on a 1 s
  background tick, so stores into a full L1 are dropped (counted in the
  mixed-eviction row).
- Read cputier and lmcache store throughput with care: both exclude the
  per-pass write-through drain to their disk tier (see each bench's README).
