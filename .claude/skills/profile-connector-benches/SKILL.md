---
name: profile-connector-benches
description: Run certus-connector-bench and cputier-connector-bench with identical workload parameters, then write an HTML report comparing the two KV-offload backends side by side (throughput, IOPS, per-phase latency, parameter parity).
argument-hint: "<output-html-path>"
---

# profile-connector-benches

Benchmark the two KV-offload connector backends with the same workload and present
them side by side:

| Arm | Bench | Backend |
|---|---|---|
| certus | `tools/certus-connector-bench/bench_connector_lifecycle.py` (host) | `certus-server` over `/dev/shm/certus-shmq`, SPDK NVMe |
| cputier | `tools/cputier-connector-bench/run-in-container.sh` (vLLM ≥ 0.30 image) | vLLM `TieringOffloadingSpec`: pinned CPU tier + `fs` disk tier |

All work is done by `run_compare.sh` in this skill's directory. It takes every
workload flag **once** and forwards the same list to both benches, so the arms
cannot drift. The DRAM tier size `--dram` becomes certus-server's
`--memory-tier-size` and cputier's `--cpu-bytes` (and the container's
`CPU_BYTES`). `make_report.py` then checks parity from the columns each bench
actually wrote to its CSV and flags any difference in red.

## Argument

- `<output-html-path>` (required): where to write the HTML report, e.g.
  `bench-results/connector-compare.html`. If it is missing, ask for it. Raw
  artifacts (params.json, both CSVs, both bench logs, server.log) go to the
  sibling directory `<path-without-.html>.d/`.

Everything else has working defaults. Pass through any extra flags the user
gives (see `run_compare.sh --help`): `--mode` (default `all`), `--bs`,
`--num-blocks`, `--block-bytes`, `--min-duration`, `--warmup`, `--working-set`,
`--pipeline-depth`, `--direction`, `--hit-ratio`, `--requests-per-step`,
`--num-sessions`, `--lookup-order`, `--gpu`, `--tp`, `--dram`; certus-only
`--device-pci`/`--drive-count`, `--eviction-threshold`, `--workers`,
`--channels`; cputier-only `--fs-tier`, `--image`.

## Steps for Claude

1. **Resolve the output path** from the argument. Make it absolute relative to the
   repo root if it is relative.

2. **Preflight** and tell the user what will run:
   - GPU free: `nvidia-smi --query-gpu=index,memory.used --format=csv`.
   - certus arm: `target/release/certus-server` exists (the script builds it with
     `cargo build -p certus-server --release` if not; it runs as the
     current user, no sudo); hugepages are available. Pick the NVMe
     device(s): list them with `lspci -Dnd ::0108` and their NUMA node from
     `/sys/bus/pci/devices/<addr>/numa_node`. Prefer devices on the GPU's NUMA
     node and pass them as `--device-pci`; otherwise the script uses
     `--drive-count 1`. Warn if a chosen SSD is on a different NUMA node from
     the GPU. Make sure no other `certus-server` is running (it would own
     `/dev/shm/certus-shmq`).
   - cputier arm: `podman image exists docker.io/vllm/vllm-openai:v0.30.0`
     (or `--image`). `--fs-tier` (default `~/cputier-bench-fs-tier`) must not be
     on overlay storage; for a fair cold-path comparison it should be on NVMe
     comparable to the certus devices. Say where it is (`df -h <dir>`).
   - If a prerequisite for one arm is missing, offer `--skip-certus` or
     `--skip-cputier` instead of aborting; the report then shows that arm as
     having no results.

3. **Launch** in the background (a `--mode all` run is several minutes per arm;
   the arms run one after the other because they share the GPU):
   ```bash
   .claude/skills/profile-connector-benches/run_compare.sh --out <output-html-path> [flags]
   ```
   Wait for it to finish. The script stops certus-server (SIGTERM, then SIGKILL
   after 8 s) on exit, including on failure.

4. **Report.** Give the user the HTML path and a short summary taken from the
   CSVs in `<path>.d/`: which backend wins each result row and by how much,
   and anything notable in the per-phase latency (for example cputier's
   `lookup_wait` on the cold path, or cputier store drops in its log). If
   `make_report.py` printed a parameter mismatch warning, or an arm failed,
   say so first and point at the log under `<path>.d/`. To re-render the HTML
   from existing artifacts, run `run_compare.sh --out <path> --report-only`
   with the same flags.

## What is and is not held equal

- Equal (forwarded identically): mode, `bs`, `num_blocks`, `block_bytes`,
  `min_duration`, `warmup`, `working_set`, `pipeline_depth`, `direction`,
  `hit_ratio`, `requests_per_step`, `num_sessions`, `lookup_order`, `tp`, GPU(s),
  DRAM tier size.
- Inherently different: the disk tier (SPDK raw NVMe vs fs files), and options
  only one bench has: certus `--workers`, `--channels`, eviction threshold,
  `--prefix-sharing` (left at its default, `shared`, which is how cputier
  behaves); cputier `--layers`, `--blocks-per-chunk`, fs thread counts (left at
  their defaults). The report lists these under "Backend-specific settings".
- Read cputier store throughput with care: it excludes the per-pass fs
  write-through drain (see `tools/cputier-connector-bench/README.md`).
