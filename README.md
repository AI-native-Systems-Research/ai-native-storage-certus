# Certus

Certus is a generative, domain-specific storage system for GPU inference KV-cache
workloads. It is built entirely by AI agents: no code in the repository is written
by hand. The project has two aims:

1. Develop an AI methodology that creates, optimizes and formally validates
   generated code.
2. Show what that methodology is worth by using it to build a highly specialized
   KV-cache offload tier for LLM serving (vLLM).

Blog posts about the work: https://ai-native-systems-research.github.io/ai-native-systems-research/blog/

## What it does

A `certus-server` process runs a tiered KV-cache (CUDA-pinned DRAM in front of
NVMe SSD). vLLM reaches it through the `certus-shmq-connector`, which talks to
the server over a lock-free shared-memory mailbox in `/dev/shm`. Only small
control messages go through the mailbox. KV blocks move GPU ↔ DRAM ↔ SSD by
CUDA and SPDK DMA.

```
vLLM --shmq--> Dispatcher --> MemoryTier (DRAM) --> BlockDevice --> NVMe SSD
                    |             ^                     ^
                    |             | promote (cold hit)  | async write-through
                 GpuServices (CUDA DMA / CUDA IPC to client GPU)
```

Main features:

- **Lazy write-through**: a populate returns once the block is in DRAM. It is
  persisted to SSD in the background.
- **Pluggable backends**: SPDK userspace NVMe, kernel `io_uring` on raw devices,
  or file-backed.
- **Pluggable eviction policies**: LRU, session-lineage, and optimized
  (O(1) LRU + TinyLFU admission).
- **Optional GPUDirect P2P**: SSD ↔ GPU through a BAR1 staging ring, skipping
  the DRAM copy.
- **Optional multi-node cooperation**: a local miss is looked up on peer nodes
  (found through Zyre discovery), and the value arrives by one-sided RDMA write.

The full architecture is in
[design/certus-architecture-overview.md](design/certus-architecture-overview.md).
Per-profile deployment and data-flow diagrams are under
[design/profiles/](design/profiles/).

## Component model

Certus is built on a COM-inspired Rust component framework
([lib/component-framework](lib/component-framework/README.md)). Components
provide typed interfaces (`define_interface!`) and declare their dependencies as
receptacles (`define_component!`). Each component is specified, generated,
tested and verified on its own, so an agent working on one only needs that
component and its bindings in context. Actors run on dedicated threads and
communicate over lock-free channels, with NUMA-aware placement.

The server's component graph is declared in a YAML profile and assembled at
compile time (see [apps/certus-server-yaml](apps/certus-server-yaml/README.md)):

| Profile | Description |
|---------|-------------|
| `full` (default) | SPDK NVMe + DRAM tier + GPU DMA, LRU eviction |
| `full-session-lists` / `full-optimized` | `full` with an alternative eviction policy |
| `full-p2p` | GPUDirect P2P cold path (`dispatcher-p2p`) |
| `full-remote` | Multi-node cooperation over RDMA |
| `full-kernel-block` / `full-fs-block` | Kernel `io_uring` or file-backed block devices |
| `minimal` | Logger only, no hardware dependencies |

## AI-driven methodology

Components are developed spec-first with
[Spec Kit](https://github.com/github/spec-kit): each component has `specs/**/spec.md`
alongside its code. The CI pipeline's spec-sync gate fails when a component's
code drifts from its specs. On top of that, Claude Code skills in
[.claude/skills/](.claude/skills/) automate the lifecycle:

- **Create and evolve**: `component-make-new`, `component-make-new-actor`,
  `component-new-version`, `component-like-existing`, `component-sync-specs`.
- **Audit**: `component-check-spec-translation`, `component-check-verif-translation`,
  `component-check-leakage`, `component-sweep`.
- **Formal verification**: `component-verify` runs the full pipeline. It builds a
  property inventory, proves it with [Creusot](https://github.com/creusot-rs/creusot)
  and [Kani](https://github.com/model-checking/kani), and renders a scoring page.
  Spin/Promela models of concurrent protocols are in [modelling/spin](modelling/spin).
- **Repair**: `component-repair` fixes the code behind a verification finding.
- **Profile**: `profile-hardware-ceiling`, `profile-kvoffload-replay-all`,
  `profile-connector-benches`, `profile-compare-with`.

Code-optimization experiments (evolutionary search over component variants) are
recorded under [evolution/](evolution/).

Tooling setup:

```bash
curl -fsSL https://claude.ai/install.sh | bash
uv tool install specify-cli --from git+https://github.com/github/spec-kit.git@v0.5.1
specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip
```

## Getting started

### Prerequisites

- Linux (tested on RHEL 9 / Fedora), x86_64.
- Rust stable, edition 2021, MSRV 1.75 (`deps/install_rust.sh`).
- For the full server: NVMe SSDs, an NVIDIA GPU with CUDA, and SPDK.
  `full-remote` also needs RDMA NICs and Zyre. RHEL 9 driver install scripts
  are in [deploy/rhel9](deploy/rhel9/README.md).

### Build and test without hardware

The default workspace members exclude everything that needs SPDK, CUDA or RDMA:

```bash
cargo build
cargo test --all -- --test-threads 1
cargo clippy -- -D warnings
cargo doc --no-deps
```

### Host setup for SPDK (one-time)

```bash
deps/install_deps.sh                          # system packages (sudo)
pip install -r deps/requirements.txt
deps/build_spdk.sh                            # builds SPDK into deps/spdk-build/
sudo deps/set_limits.sh                       # memlock unlimited (re-login afterwards)
components/spdk-env/scripts/add_kernel_options.sh   # IOMMU + 1G hugepages (reboot afterwards)
```

`add_kernel_options.sh` sets `intel_iommu=on`. On AMD, use `amd_iommu=on`
instead. For multi-node cooperation, also run `deps/build_zyre.sh`.

### Build and run the server

```bash
scripts/build-certus-spdk.sh        # = CERTUS_PROFILE=full cargo build -p certus-server-yaml --release
sudo ./target/release/certus-server-yaml --drive-count 4 --format --memory-tier-size 32G
```

`--format` erases existing on-disk data. Other profiles have their own build
scripts (`scripts/build-certus-*.sh`). [scripts/run-certus-shmq-server.sh](scripts/run-certus-shmq-server.sh)
starts the server with NUMA pinning, using the configuration from the benchmark
runs.

### Connect vLLM

[certus-shmq-connector/](certus-shmq-connector/) is a pure-Python vLLM
`OffloadingSpec` connector, with a container build (`build_connector_container.sh`)
and a launcher (`run-bench.sh`). The container must share the host IPC namespace
(`--ipc=host`) so it can see the server's `/dev/shm` mailbox. Kubernetes and llm-d
deployment material is under [deploy/](deploy/).

## Benchmarking and profiling

| Location | Purpose |
|----------|---------|
| [benchmarks/](benchmarks/BENCHMARKING.md) | Trace replay (policy layer) and live-serving KV-offload benchmarks |
| [apps/python](apps/python/README.md) | Direct `certus-server` API benchmarks and integration tests |
| [apps/iops-benchmark](apps/iops-benchmark/README.md), [apps/extent-benchmark](apps/extent-benchmark/README.md) | Raw NVMe and extent-allocator performance |
| [apps/workload-generator](apps/workload-generator/README.md) | Multi-node synthetic workload generator |
| [tools/](tools/) | Connector benches, hardware-ceiling probes, `certus-fio`, simulators |
| [profiling/](profiling/) | Recorded hardware ceilings and performance matrices |

Microbenchmarks use Criterion: `cargo bench`, or for example
`cargo bench -p extent-manager --bench create_benchmark`.

## Repository layout

| Path | Contents |
|------|----------|
| `lib/` | Component framework, `spdk-sys` FFI, `shm-queue` mailbox, `shmq-dispatcher` |
| `components/` | Certus components (dispatcher, memory tier, dispatch map, block devices, extent manager, eviction policies, GPU services, remote lookup/RDMA, Zyre) and shared `interfaces` |
| `apps/` | `certus-server-yaml` and `certus-server`, benchmarks, test utilities, vLLM serving scripts |
| `certus-shmq-connector/` | vLLM connector |
| `design/` | Architecture overview and per-profile design artifacts |
| `knowledge/` | Internal wiki: component architecture, SPDK, KV-cache workload analysis |
| `modelling/` | Creusot and Spin formal models |
| `benchmarks/`, `evaluators/`, `profiling/`, `evolution/` | Performance evaluation and optimization experiments |
| `deploy/`, `scripts/`, `tools/`, `deps/` | Deployment, build/run scripts, tooling, third-party dependencies |

Many components have their own `README.md` and `CLAUDE.md`. Read those before
working in that directory.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) and
[SECURITY.md](SECURITY.md). Licensed under the [Apache License 2.0](LICENSE).
