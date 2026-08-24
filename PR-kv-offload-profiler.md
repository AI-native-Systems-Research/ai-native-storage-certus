# KV-offload replay profiler: 5-backend orchestration, host auto-config, GPU clock pinning + utilization telemetry

## Summary

Turns `benchmarks/kv-offload-replay/profile_all.sh` from a best-effort script into
a repeatable, self-configuring harness that runs the same 450×12 replay workload
across **five** KV-offload backends and emits a single comparison table plus
GPU-utilization telemetry. Adds the host-side plumbing (`tools/configure-bench.sh`)
to flip NVMe/hugepage/RAID state between phases, pins vLLM base-image versions so
all backends are compared apples-to-apples, and pins GPU clocks so wall-time
differences reflect the backend under test rather than GPU auto-boost drift.

Net change is confined to the benchmark harness, its drivers/Dockerfiles, the
Certus-grpc launch wrapper, and host-config tooling. **No change to the
`certus_grpc_connector` package** vs `unstable` (an intermediate connector revert
and its revert cancel out).

## Backends compared

| Variant | Backend |
|---|---|
| NoOffload | GPU-only baseline |
| CPUOffload | vLLM `OffloadingConnector` → host RAM |
| SharedStorage | `llmd_fs_backend` RAID0/XFS |
| Certus-SPDK | gRPC client + `certus-server-yaml` (SPDK NVMe) |
| **Tiered-CPU-FS** *(new)* | CPU primary + filesystem secondary |

## What's included

**Orchestration & result integrity**
- 5-variant run with `--only`/`--skip` subsetting; unknown variant tokens are rejected.
- Per-variant results flushed to `result-<variant>.json` the instant each finishes, so a
  crash or subset run never loses a completed backend; aggregate `results.json` at the end.
- Reaps stale `*-bench` containers by image before a GPU-free check that *warns* (never
  kills a foreign process).
- Headless-safe sudo preflight; `--device-pci` devices are verified to exist before any
  storage backend runs.

**Certus-SPDK lifecycle & DRAM-tier sizing**
- Server started/stopped by the harness (SIGTERM→SIGKILL escalation for SPDK teardown),
  pinned to the NVMe/hugepage NUMA node so the SPDK reactor can't land on the wrong socket.
- `--total-mem <GiB>` derives the DRAM tier from the host's total memory: the harness sizes
  the 1G hugepage pool from it (minus DPDK/SPDK overhead) and `configure-bench.sh` links the
  boot-time `mem=` cap to the same budget, so the reserved pool and the OS's visible RAM stay
  consistent. Rejected if it exceeds physical RAM. Replaces the old fixed 32 GiB tier — e.g.
  `--total-mem 64` → a 46080 MiB (45 GiB) `spdk_zmalloc` pool.
- Certus-SPDK ordered **before** the host-RAM backends so it consumes the boot-reserved 1G
  pool while intact; the pool is released between phases so CPUOffload/SharedStorage aren't
  starved. In-run NVMe-group reconfigure; user-writable hugetlbfs so the non-root server maps
  segments.
- Server built with `--features rw-telemetry` so per-round SSD read/write bytes/ops/latency
  are real (via `GetIoStats`), not zeros. The gRPC replay driver logs per-round wall time
  alongside those I/O deltas, so a Certus run yields the same per-round latency curve as the
  other backends.
- Replay dataset resolved from `certus-connector/` with a fallback to the script directory,
  so the preflight existence check stops false-warning on the current repo layout.

**SharedStorage isolation**
- Auto-selects its RAID device and targets a **separate** device/mount so a Certus teardown
  can never dismantle the model-fs `md0`; chowns the mount; no longer clobbers Certus's
  hugepage boot param. Drops the manual `--shared-fs`/`--disk-dev` flags.

**vLLM version parametrization**
- `VLLM_VERSION` is a build-arg across all offload-replay images; `--vllm-version <x.y.z>`
  pins every backend to one base image for a clean 5-way run (default `0.26.0`).
- `MAX_ROUNDS` cap plumbed through all drivers and the orchestrator.
- SharedStorage driver fixed for the vLLM 0.23 spawn model (+ `msgpack` dep).

**Certus client transport**
- `--client-network host` A/Bs the Certus transport; defaults to host networking (dials the
  server over loopback, skipping the rootless slirp4netns/pasta userspace proxy — ~10% faster
  on the 450×12 workload).

**GPU stability & telemetry** *(this session)*
- Pins every GPU to its own queried max SM clock (`nvidia-smi -pm 1` + `-lgc <max>,<max>`,
  never hardcoded) before any backend and resets on exit — removes the ~12% wall-time drift
  from auto-boost on byte-identical work.
- Background sampler snapshots per-GPU util/mem/clock/temp/power every 2 s
  (`GPU_SAMPLE_SEC`) to `gpu-timeline.csv`, with per-variant windows in `gpu-markers.csv`.
  `gpu_report.py` slices the timeline by window into a per-variant table (avg/max/p95 util,
  avg SM clock, max mem, avg power, sample count) plus an over-time utilization sparkline,
  teed to `gpu-summary.txt`. Sampler is reaped on every exit path.

**Host-config tooling (`tools/configure-bench.sh`)**
- Don't pipe `yes` into `mdadm` (avoids pipefail SIGPIPE); reclaim member drives from stray
  arrays; scope Certus teardown to its own drives; always chown hugepages; fix a reconfigure
  hang and root-device `MD_DEVICE` handling; fix an undefined `error` call in `check_root`.

## Test plan

- `bash -n benchmarks/kv-offload-replay/profile_all.sh` and `python3 -m py_compile
  benchmarks/kv-offload-replay/gpu_report.py` — clean.
- `gpu_report.py` smoke-tested against synthetic timeline/marker CSVs (per-variant table +
  sparkline render correctly).
- Full 5-way run on the bench host produces `results.json`, per-variant `result-*.json`,
  `gpu-timeline.csv`, `gpu-markers.csv`, and `gpu-summary.txt`; per-round SSD I/O is non-zero
  with the rw-telemetry server build, and each Certus round now prints its wall time.
- `--total-mem` verified end-to-end at 64 GiB (46080 MiB pool derived; `mem=` cap matched);
  a value above physical RAM is rejected with a clear error.

## Notes

- Certus-SPDK requires the `certus-server-yaml` binary built with `--features rw-telemetry`
  (`GDRCOPY_LIB_PATH=/usr/local/lib` to avoid the `-lgdrapi` linker error) and the NVMe device
  unbound from the kernel driver with the 1G hugepage pool reserved at boot.
- SharedStorage needs its RAID/XFS mount and the `llmd_fs_backend` repo to build its image.
