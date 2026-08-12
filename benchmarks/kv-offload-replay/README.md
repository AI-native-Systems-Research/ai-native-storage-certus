# Offloading-connector tracing and replay

Capture vLLM `OffloadingConnector` / `OffloadingManager` traffic as JSONL, then
replay it against a fresh cache policy to compare hit ratios, admission, and
evictions without re-running the model.

- **Generation** requires vLLM (the tracing connectors wrap live vLLM classes).
- **Replay** is pure Python by default — no vLLM or GPU needed.

> **Running the live 450×12 comparison workload?** See
> [`RUNBOOK-cpu-and-sharedstorage.md`](RUNBOOK-cpu-and-sharedstorage.md) for the
> CPU-offload and SharedStorage backends, and `../../certus-grpc-connector/README.md`
> for the Certus gRPC path.

> **All four backends' container files live here.** The NoOffload / CPU-offload /
> SharedStorage images are built from `Dockerfile.{nooffload,cpu-offload,sharedstorage}`
> in this directory. The Certus gRPC backend's container files are **symlinks** into
> `../../certus-grpc-connector/` (`Dockerfile.certus-grpc`, `run-bench-certus-grpc.sh`,
> `docker-entrypoint-certus-grpc.sh`) — the real files stay there because that image's
> build needs the connector Python package in its context, but building via the symlink
> works (build context is still the repo root: `podman build -f
> benchmarks/kv-offload-replay/Dockerfile.certus-grpc -t certus-grpc-bench .`). All four
> Dockerfiles take `--build-arg VLLM_VERSION=<x.y.z>` (default `0.23.0`).

## Files

| File | Purpose |
|---|---|
| `tracing_offloading_connector.py` | Drop-in `KVConnectorBase_V1` that wraps vLLM's `OffloadingConnector`. Writes `offloading_trace_<pid>.jsonl` (connector-level calls). |
| `tracing_offloading_manager.py` | Drop-in `CPUOffloadingSpec` that wraps the underlying `OffloadingManager` + `OffloadingHandler`. Writes `offloading_mgr_<pid>.jsonl` (lookup/touch/prepare_store/…) and `offloading_handler_<pid>.jsonl` (GPU↔CPU transfer jobs). |
| `run_sharegpt_offloading.py` | Driver: runs `vllm bench throughput` on ShareGPT with the tracing connectors attached. |
| `run_multiturn_offloading.py` | Driver: the multi-turn 450×12 CPU-offload workload for head-to-head vs Certus. Uses the built-in `OffloadingConnector` by default (no tracing); `TRACE_OFFLOAD=1` switches to the tracing wrappers above. See [`RUNBOOK-cpu-and-sharedstorage.md`](RUNBOOK-cpu-and-sharedstorage.md). |
| `run_multiturn_nooffload.py` | Driver: the multi-turn 450×12 **no-offload** (GPU-only) baseline — same workload with no `kv_transfer_config`, so evicted KV is recomputed rather than fetched. The reference point for the offload backends. Containerized by `Dockerfile.nooffload`. See [`RUNBOOK-cpu-and-sharedstorage.md`](RUNBOOK-cpu-and-sharedstorage.md). |
| `replay_offloading_traces.py` | Replays manager and/or handler traces against a pluggable target. Built-in manager targets: pure-Python LRU (default), vLLM `CPUOffloadingManager`, Certus via `CertusOffloadingSpec` (native or policy-only), and `llmd_fs_backend`. Built-in handler targets: `fs-backend`, Certus via `CertusOffloadingSpec.get_handlers()`. |

## Prerequisites

- Python 3.9+
- For trace generation: vLLM installed, a GPU, and `sharegpt_v3.json` in this directory (from the `anon8231489123/ShareGPT_Vicuna_unfiltered` release).
- For replay (default): nothing beyond the Python standard library.
- For replay against optional backends, install what the backend needs (each is lazy-imported):
  - `cpu-manager` → vLLM
  - `fs-backend` → vLLM + `torch` + CUDA + `llmd_fs_backend` + `storage_offload`
  - `certus-connector` → vLLM ≥ 0.20 + `certus_native` (from `ai-native-storage-certus/certus-connector`, `maturin develop --release`). SPDK-bound NVMe + torch + CUDA only for `use_native: true` runs; policy-only (`use_native: false`) runs on any host.

## Generating traces

```bash
PYTHONPATH=. python run_sharegpt_offloading.py \
    --num-conversations 200 \
    --num-prompts 200
```

- `--num-conversations N` filters `sharegpt_v3.json` to the first N valid (≥1 human→gpt pair) conversations and writes `sharegpt_subset_<N>.json`, which is then fed to vLLM.
- `--num-prompts M` is vLLM's own flag: how many prompts it samples from that pool. Other `vllm bench throughput` flags pass through unchanged (for example, `--model`, `--max-model-len`, `--gpu-memory-utilization`).

Every process writes its own trace files. After a run you should see:

```
offloading_trace_<pid>.jsonl     # connector-level (verbose; references vLLM objects)
offloading_mgr_<pid>.jsonl       # manager-level  (clean JSON — use this for replay)
offloading_handler_<pid>.jsonl   # handler-level  (GPU↔CPU transfer jobs)
```

The manager trace is the canonical input for `replay_offloading_traces.py`.

## Reference traces

Pre-generated traces live under `traces/` so you can replay without a GPU.

| Path | Workload | Model | GPU | Wall | Transfers | Notes |
|---|---|---|---|---|---|---|
| `traces/sharegpt/199-prompts.{mgr,handler}.jsonl` | ShareGPT v3, 199 prompts | Meta-Llama-3-8B | A30 24 GB | 86 s | 199 GPU→CPU, 0 CPU→GPU | vLLM 0.19.1, `max-model-len 4096`, `gpu-mem-util 0.90`, block size 16. Write-path only — no cache hits landed, so load latency / bandwidth is not exercised. Replay results: [`traces/sharegpt/199-prompts.results.md`](traces/sharegpt/199-prompts.results.md). |

Substitute the paths above for the `offloading_mgr_*.jsonl` / `offloading_handler_*.jsonl` globs in any example below.

## Replaying traces

### Default (pure Python, no vLLM)

```bash
python replay_offloading_traces.py \
    --manager-trace offloading_mgr_*.jsonl \
    --num-blocks 256
```

Example output:

```
=== manager replay ===
  target: simple-lru {'num_blocks': 256, 'block_size': 16, 'policy': 'lru'}
  lookup:       calls=17  req=260 blocks  hit=0 (0.00%)
  prepare_store: calls=199  rejected=0 (0.00%)
                 admitted=442/442  evicted=186 (186 unique)
  complete_store: calls=20  blocks=442
  wall: 0.012s  ops/s=36864  admitted_blocks/s=35811
  latency_ms (per method):
    method               n     mean      p50      p95      p99      max
    lookup              17    0.021    0.017    0.055    0.055    0.055
    touch              219    0.018    0.015    0.048    0.053    0.058
    prepare_store      199    0.014    0.008    0.036    0.126    0.604
    complete_store      20    0.028    0.023    0.062    0.062    0.062
```

The manager replay captures wall time, per-method latency percentiles, and aggregate ops/s — enough to benchmark one policy against another. Shrink `--num-blocks` to force eviction pressure; grow it to see the hit-rate ceiling.

### Against vLLM's `CPUOffloadingManager`

```bash
python replay_offloading_traces.py \
    --manager-trace offloading_mgr_*.jsonl \
    --target cpu-manager \
    --num-blocks 1024 --policy lru
```

vLLM is imported lazily — only this target pulls it in.

### Against Certus (via `CertusOffloadingSpec`)

One target (`certus-connector`) with two run configurations, toggled by `extra_config.use_native`:

**Native (real SPDK + NVMe IO):**

```bash
python replay_offloading_traces.py \
    --manager-trace offloading_mgr_*.jsonl \
    --target certus-connector \
    --target-args '{"extra_config": {"use_native": true,
                                      "data_pci_addrs": ["0000:61:00.0"],
                                      "metadata_pci_addr": "0000:62:00.0"}}'
```

Spec returns `NativeCertusOffloadingManager` — thin Python adapter over `certus_native.CertusEngine`. Manager methods delegate to the Rust engine; `store_async` / `load_async` hit SPDK → NVMe.

**Policy-only (no IO):**

```bash
python replay_offloading_traces.py \
    --manager-trace offloading_mgr_*.jsonl \
    --target certus-connector \
    --target-args '{"extra_config": {"use_native": false,
                                      "slab_size_bytes": 131072,
                                      "dram_cache_bytes": 8589934592}}'
```

Spec returns `CertusOffloadingManager` — the tiered DRAM + NVMe manager implemented in Python. Simulates the same tiering policy (LRU eviction, promotion/demotion thresholds, DRAM-slot / NVMe-slab budgets from `TieringConfig`), but no bytes are moved: `nvme_slab` / `dram_slot` are Python integer IDs, not real addresses.

Use the policy-only config to isolate manager-layer and spec-layer Python cost from real storage cost. Running both configs on the same trace and diffing the wall time and per-method p50 tells you how much of the total was Python vs. SPDK. (On our 442-block sample trace: 6 ms policy-only vs. 12 ms native — Python is not the bottleneck at scale, but the diff grows with trace size.)

Both configs route through the same `CertusOffloadingSpec`, so `extra_config` plumbing (slab/DRAM budgets, tiering thresholds) is identical. The `certus_connector` Python package adapts its vLLM imports to 0.20+ via a small `sys.modules` shim the replay installs on its behalf.

**Requirements**: `certus_native` built from `ai-native-storage-certus/certus-connector` (`maturin develop --release`), vLLM ≥ 0.20, torch + CUDA; SPDK-bound NVMe required only for the `use_native: true` config.

### Against the llmd_fs_backend (real files on disk)

```bash
python replay_offloading_traces.py \
    --manager-trace offloading_mgr_*.jsonl \
    --target fs-backend \
    --target-args '{"root_dir": "/tmp/kv-fs-replay",
                     "num_gpu_blocks": 1024,
                     "per_block_bytes": 4096}'
```

Stands up a `SharedStorageOffloadingManager` plus `StorageOffloadingHandlers` wired to the `storage_offload` C++ engine, backed by a fabricated zero-filled GPU KV tensor. `prepare_store` issues a real `transfer_async` (GPU → disk); `complete_store` polls `get_finished()` until all in-flight writes have landed, so subsequent `lookup`s see the files and report accurate hit rates.

Requires `torch` + CUDA, `vllm`, `llmd_fs_backend`, `storage_offload`. Writes real files under `root_dir` — clean up afterwards with `rm -rf`. The `storage_offload` engine pads each file to its staging-buffer size, so disk usage can exceed payload bytes by a large factor.

### Against your own policy

Point `--target` at `module.path:ClassName`. Constructor kwargs come from `--target-args`:

```bash
python replay_offloading_traces.py \
    --manager-trace offloading_mgr_*.jsonl \
    --target mypkg.mypolicy:TwoQCache \
    --target-args '{"protected_blocks": 512, "probation_blocks": 512}'
```

### Target protocol

A replay target is any object with these methods. Keys are hex-string block hashes, passed through as-is from the trace.

```python
class MyTarget:
    def __init__(self, num_blocks: int, block_size: int = 16, **kwargs): ...

    def lookup(self, keys: list[str]) -> int:
        """Return the number of leading keys present (prefix hit)."""

    def touch(self, keys: list[str]) -> None:
        """Mark keys as recently used. May be called with empty list."""

    def prepare_load(self, keys: list[str]) -> None:
        """Reserve keys for a read. May raise if missing."""

    def complete_load(self, keys: list[str]) -> None: ...

    def prepare_store(self, keys: list[str]):
        """Admit new keys, evicting as needed. Return an object with
        .block_hashes_to_store and .block_hashes_evicted, or None if
        the request cannot fit."""

    def complete_store(self, keys: list[str], success: bool = True) -> None: ...
```

The shape matches vLLM's `OffloadingManager` so existing implementations can be wrapped in a few lines — see `_make_cpu_manager_target` in `replay_offloading_traces.py`.

## What the replay measures (and what it doesn't)

The manager trace captures **every scheduler↔manager call with its full key list** — `lookup`, `touch`, `prepare_load`, `complete_load`, `prepare_store`, `complete_store`. That's enough to replay any alternative manager *faithfully at the per-call level*: each call gets the exact same key sequence the original vLLM run presented, so each manager's own admission / eviction / retention decisions are real.

The handler trace captures **the shape of data movement** — transfer direction, block count, timing — but **not** the KV tensor bytes. Replay either moves zero-filled / uninitialised buffers of the right size (real-worker mode) or applies a per-block cost model (simulated mode).

### Faithfully measured

| Metric | Comes from |
|---|---|
| Cache hit rate | Each alternative manager's `lookup` return values on the original key sequences. |
| Admission count / rejection rate | `prepare_store` decisions. |
| Eviction count and identity | `prepare_store` output: `evicted_keys`. |
| Manager-layer wall time and per-method p50 / p99 latency | Measured live during replay. |
| Handler throughput (MB/s) and submit→done latency | Measured live; real IO in real-worker mode. |

### Not closed-loop

Trace replay is **open-loop**. The manager trace is a recording of what happened in one specific vLLM run — it's not a script that adapts to a new manager's responses. Concretely:

Original run:
```
scheduler:  manager.lookup([A, B, C, D, E])
manager:    → 3            (A, B, C are hits)
scheduler:  prepare_load([A, B, C]); prefill [D, E]; later prepare_store([new blocks])
```

Replay against a different manager:
```
replay:     manager_new.lookup([A, B, C, D, E])
manager_new: → 1           (only A is hit in its state)
replay:     (still marches to the next captured record — a prepare_load for whatever
             the original manager reported, NOT what manager_new just said)
```

So the `lookup` count is real, but the subsequent calls are fixed from the original execution. In production, a different manager's different hit rates would have caused different prefills, different block hashes, different eviction pressure later — we don't re-derive any of that.

### What that means for interpretation

The replay answers:

> "Presented with the same key requests, how does manager X compare to manager Y on per-call decisions and on storage cost?"

It does **not** answer:

> "What would the end-to-end serving throughput have been if we'd run vLLM with manager X from the start?"

To answer the second question, run vLLM with manager X as the active connector and capture a fresh trace — i.e., re-do the *generation* step with the new manager, not replay. The tracing connector is backend-agnostic, so any `OffloadingManager`-shaped backend plugs in cleanly.

In practice, per-call decisions strongly predict end-to-end behaviour: a manager with a higher hit rate on the same key stream will almost certainly be better end-to-end too. The open-loop caveat matters most when you're chasing fine-grained tail-latency differences or want to report production-accurate tokens/sec.

## Trace formats

`offloading_mgr_<pid>.jsonl` (one JSON object per line):

```json
{"ts": 2.46, "method": "touch", "keys": ["41176c…", "594304…"]}
{"ts": 2.46, "method": "lookup", "keys": [...]}
{"ts": 2.46, "method": "prepare_store", "keys": [...]}
{"ts": 2.47, "method": "complete_store", "keys": [...], "success": true}
```

`offloading_handler_<pid>.jsonl` (read by the simulated-handler replay):

```json
{"ts": 5.22, "method": "transfer_async", "transfer_type": "GPU->CPU",
 "job_id": 0,
 "src": {"medium": "GPU", "block_ids": [3, 4, 5, ...], "group_sizes": [12]},
 "dst": {"medium": "CPU", "block_ids": [0, 1, 2, ...]}}
```

`offloading_trace_<pid>.jsonl` is the connector-level trace — useful for timing and call sequence, but arguments contain `repr()` of live vLLM objects so it is not self-contained. The manager and handler traces are the portable ones.

## Handler replay

Two modes — simulated (default) or real worker.

### Simulated cost model

```bash
python replay_offloading_traces.py \
    --handler-trace offloading_handler_*.jsonl \
    --per-block-ms 0.05
```

Applies a per-block service time to each `transfer_async` event and reports p50/p95/p99 latency and aggregate throughput. No external deps.

### Real worker (FS backend or Certus)

Two backends:

```bash
# Drive the real llmd_fs_backend worker
python replay_offloading_traces.py \
    --handler-trace offloading_handler_*.jsonl \
    --handler-target fs-backend \
    --handler-target-args '{"root_dir": "/tmp/kv-fs-handler",
                             "num_gpu_blocks": 1024,
                             "per_block_bytes": 16384}'

# Drive Certus via CertusOffloadingSpec.get_handlers()
python replay_offloading_traces.py \
    --handler-trace offloading_handler_*.jsonl \
    --handler-target certus-connector \
    --handler-target-args '{"extra_config": {"use_native": true,
                                              "data_pci_addrs": ["0000:61:00.0"],
                                              "metadata_pci_addr": "0000:62:00.0"}}'
```

`certus-connector` instantiates `CertusOffloadingSpec`, calls `spec.get_handlers()` to obtain `GpuToCertusHandler` / `CertusToGpuHandler`, and drives them through a `TransferSpec` with `CertusLoadStoreSpec` destinations. With `use_native: true` the handler's `transfer_async` invokes `CertusEngine.store_async` / `load_async`, issuing real CUDA DMA + SPDK NVMe I/O. With `use_native: false` the spec falls back to `MockCertusEngine` — `transfer_async` returns immediately, no bytes moved, useful as a zero-IO baseline for comparing handler-layer overhead.

The replay driver replays every `transfer_async` event against the real worker: it synthesizes a destination (block hashes for FS, u64 `CacheKey`s for Certus) matching the trace's block count and direction, calls the worker's `transfer_async`, then drains completions at each `wait` / `get_finished` event in the trace. For `in`-direction transfers (storage → GPU) it reuses hashes/keys written by earlier `out`-direction transfers so the worker can actually find them.

The reported latency is real wall time (submit → completion drain) and the throughput reflects real disk or NVMe bandwidth. Synthetic destinations mean the content on disk isn't meaningful — only the shape, timing, and direction of transfers are preserved.

Custom handler targets: pass `--handler-target module.path:ClassName`. The class must expose `transfer_async(job_id, n_blocks, direction)`, `wait(job_ids)`, and `get_finished()` — see `_make_fs_handler_target` for a reference implementation.

## Benchmarking example

Generate a sizeable trace with some reuse and eviction pressure, then replay it through the production Certus stack (both manager and handler):

```bash
# 1. Generate: 500 ShareGPT conversations through vLLM + TracingOffloadingConnector
PYTHONPATH=. python run_sharegpt_offloading.py --num-conversations 500 --num-prompts 500

# 2a. Native run: manager + handler with real SPDK IO
python replay_offloading_traces.py \
    --manager-trace offloading_mgr_*.jsonl \
    --handler-trace offloading_handler_*.jsonl \
    --target certus-connector \
    --handler-target certus-connector \
    --target-args '{"extra_config": {"use_native": true,
                                      "data_pci_addrs": ["0000:61:00.0"],
                                      "metadata_pci_addr": "0000:62:00.0"}}' \
    --handler-target-args '{"extra_config": {"use_native": true,
                                              "data_pci_addrs": ["0000:61:00.0"],
                                              "metadata_pci_addr": "0000:62:00.0"}}' \
    --output-json bench_native.json

# 2b. Policy-only run: same trace, no IO — isolates Python overhead
python replay_offloading_traces.py \
    --manager-trace offloading_mgr_*.jsonl \
    --handler-trace offloading_handler_*.jsonl \
    --target certus-connector \
    --handler-target certus-connector \
    --target-args '{"extra_config": {"use_native": false}}' \
    --handler-target-args '{"extra_config": {"use_native": false}}' \
    --output-json bench_policy.json
```

Diff `bench_native.json` and `bench_policy.json` for the "what did SPDK NVMe cost vs. what did Python cost" breakdown. For a backend comparison, run the same trace with `--target fs-backend --handler-target fs-backend` (matching `num_gpu_blocks` / `per_block_bytes`). For a lower-bound replay-driver cost, run with `--target simple-lru --num-blocks <big>` — no eviction, no backend, just the replay loop.

## Comparing all backends: `profile_all.sh`

`profile_all.sh` runs the same 12-turn ShareGPT replay workload through up to four
KV-offload backends — `nooffload` (GPU-only baseline), `certus-spdk` (gRPC client +
`certus-server` over DRAM + raw NVMe), `cpuoffload` (vLLM's host-RAM
`OffloadingConnector`), and `sharedstorage` (`llmd_fs_backend` RAID0/XFS) — and emits
a side-by-side throughput table. Each backend is preflighted independently: ready ones
run, the rest are marked `SKIPPED` with a reason, and per-backend failures never fail
the whole run.

**The host reconfiguration is automatic.** The shared NVMe group you pass with
`--device-pci` is flipped between phases *by the script itself* via
`tools/configure-bench.sh` — bound to `vfio-pci` + 1G hugepages for the `certus-spdk`
phase (SPDK), and to the kernel `nvme` driver + RAID0/XFS for the `sharedstorage`
phase. You do **not** run `configure-bench.sh` yourself; `profile_all.sh` invokes it
(runtime-only, no reboot) and requests a reboot only if the boot-reserved 1G-hugepage
pool falls short. `sudo` is cached once, up front.

### Running only the Certus-SPDK backend

To profile just the Certus stack (`--only certus-spdk`) — building the client image
first, pinning the vLLM base image, and capping the replay at 12 turns:

```bash
time benchmarks/kv-offload-replay/profile_all.sh \
    --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 \
    --device-pci 0000:63:00.0 --device-pci 0000:64:00.0 \
    --max-rounds 12 \
    --model-fs /mnt/fs-backend-bench \
    --vllm-version 0.23.0 \
    --only certus-spdk \
    --evict-threshold 1 \
    --build
```

### Parameters used above

| Flag | Meaning |
|------|---------|
| `--device-pci <DDDD:BB:DD.F>` | NVMe PCIe address of the shared drive group; **repeatable** (one per drive). The `certus-spdk` phase binds these to `vfio-pci`; needed for that backend to run at all. |
| `--max-rounds <n>` | Cap every backend at N replay rounds/turns (`0` = replay all turns). `12` matches the 12-turn dataset. |
| `--model-fs <dir>` | Filesystem for the HF model cache and the gRPC podman image store. Default `/mnt/certus1`. Also the default location of the run's `--logdir`. |
| `--vllm-version <x.y.z>` | Pin the vLLM base-image version for the built images (passed as the `VLLM_VERSION` build arg; images are tagged `:vllm<x.y.z>` so versions coexist). Implies the images must be built at that version — pass `--build` too. |
| `--only certus-spdk` | Run only the Certus-SPDK backend. Other valid names: `nooffload`, `cpuoffload`, `sharedstorage` (comma-separated). |
| `--evict-threshold <f>` | Certus-SPDK DRAM→SSD demotion threshold. Default `0.6`; `1` effectively defers demotion until the DRAM tier is full. |
| `--build` | Build any missing bench image before its run. Required the first time (or after `--vllm-version` changes), since the image tag won't exist yet. |

Other useful flags: `--memory-tier-size <sz>` (Certus-SPDK server DRAM pool, e.g.
`32G`), `--num-convs <n>` (conversations to replay, default 450), `--gpu <sel>` (CDI
GPU selector: `all` | `0` | `0,1` | `<uuid>`), and `--logdir <dir>` (output dir;
defaults to `<model-fs>/kvprofile-<runid>`). Run `profile_all.sh --help` for the full
list.

> **Note:** the container network mode is not a CLI flag — `profile_all.sh` launches
> the gRPC client with `--ipc=host` already, so the host `certus-server` can open the
> container's CUDA IPC handles. There is no `--client-network` option; passing an
> unknown flag exits with an error.

### Outputs

Per run: `<logdir>/<variant>.log` (per-backend stdout/stderr),
`<logdir>/result-<variant>.json` flushed as each backend finishes (survives a crash or
a `--only` subset), `<logdir>/results.json` (aggregate), and the comparison table on
stdout. Wrapping the invocation in `time` (as above) captures total wall-clock across
all selected phases including the host reconfiguration.

## Other datasets

`--num-conversations` is ShareGPT-specific (the filter expects the ShareGPT V3 schema with `id` and `conversations[{from,value}]`). For other datasets, skip the flag and use `vllm bench throughput`'s native dataset options (`--dataset-name random`, `--dataset-name sonnet`, a custom `--dataset-path`, etc.) — the tracing connectors do not care where the prompts come from.
