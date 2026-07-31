---
name: profile-kvoffload-replay-all
description: Run all four KV-offload benchmark variants (NoOffload, CPUOffload, SharedStorage, Certus-SPDK) against the same 12-turn ShareGPT replay and present throughput in one side-by-side table. Use when the user wants to compare offload backends end-to-end.
---

# profile-kvoffload-replay-all

Orchestrate the four KV-offload benchmark variants over the same replay workload
and present their throughput in a single comparison table. All real work is done
by `benchmarks/kv-offload-replay/profile_all.sh`; this skill collects inputs,
previews what will run, launches the orchestrator, and formats the result.

## Variants

| Variant | Backend | Image / binary |
|---|---|---|
| NoOffload | GPU-only baseline | `certus-nooffload-bench` |
| CPUOffload | vLLM OffloadingConnector → host RAM | `certus-cpu-offload-bench` |
| SharedStorage | `llmd_fs_backend` RAID0/XFS | `certus-sharedstorage-bench` |
| Certus-SPDK | gRPC client + `certus-server-yaml` (SPDK NVMe) | `certus-grpc-bench` + host server |

## Inputs

- **`--device-pci <DDDD:BB:DD.F>`** — NVMe PCIe address for the Certus-SPDK
  server (repeatable). Omit → Certus-SPDK is SKIPPED.
- **`--shared-fs <dir>`** — filesystem bind-mounted to `/mnt/fs-backend-bench`
  for SharedStorage. Omit → SharedStorage is SKIPPED.
- **`--model-fs <dir>`** — filesystem for the HF cache and gRPC podman store
  (default `/mnt/certus1`). HF cache lives at `<model-fs>/hf-cache`.
- **`--model <hf-id>`** — applied to all four variants. Default
  `NousResearch/Meta-Llama-3-8B`. NOTE: the baked dataset and drivers are built
  for Llama-3; `ibm-granite/granite-4.1-8b` renders empty prompts and fails.
- Tuning: `--num-convs 450`, `--output-tokens 150`, `--max-model-len 8192`,
  `--max-num-seqs 64`, `--gpu all`, `--memory-tier-size 32G`, `--cpu-bytes`,
  `--dram`. `--build` to build missing images. `--only`/`--skip` to subset.
  `--logdir` for output (default `<model-fs>/kvprofile-<runid>`).

## Steps for Claude

1. **Collect inputs** from the invocation. If `--model` was NOT supplied, ask via
   AskUserQuestion — options: `NousResearch/Meta-Llama-3-8B` (recommended /
   default), `ibm-granite/granite-4.1-8b`, custom HF id. Default to Llama-3.
   Do not prompt for anything else; other flags have working defaults.

2. **Preflight summary.** Before launching, state which of the four variants will
   run vs SKIP and why:
   - Check images: `podman image exists certus-nooffload-bench`, `…cpu-offload-bench`,
     `…sharedstorage-bench`; gRPC image in the model-fs store
     (`podman --root <model-fs>/podman/storage image exists localhost/certus-grpc-bench`).
   - SharedStorage needs `--shared-fs` pointing at a real dir; Certus-SPDK needs
     `--device-pci` and a built `target/release/certus-server-yaml`.
   - If images are missing, note that `--build` will build them (SharedStorage
     needs `FS_BACKEND_DIR` with the `llmd_fs_backend` repo). Offer to add `--build`.

3. **Launch** `benchmarks/kv-offload-replay/profile_all.sh` with the resolved
   flags. A full four-variant run takes ~15–60 min (each variant loads the model
   and replays 450 conversations), so run it in the background and monitor the
   log until it finishes. The script tees each variant to `<logdir>/<variant>.log`
   and never aborts the whole run on a single variant's failure.

4. **Report.** Read `<logdir>/results.json` and present the comparison table.
   Call out any SKIPPED/FAILED rows with the reason and the one-line fix (e.g.
   "SharedStorage SKIPPED — pass `--shared-fs /mnt/fs-backend-bench`"; "add
   `--build`"; "check `<logdir>/server.log`"). `tokens_per_sec` is computed
   uniformly as `generations × output_tokens ÷ wall` so variants are comparable
   even though each driver prints a different native metric.

## Notes / prerequisites (not automated)

- The GPU must be free; the script reaps stale `*-bench` containers and warns if
  a GPU already has memory in use.
- The Certus-SPDK server lifecycle (start, poll `:50051`, teardown with SIGTERM
  then SIGKILL after ~8s — it ignores SIGTERM during SPDK teardown) is handled by
  the script. It requires host hugepages + the NVMe device unbound from the
  kernel driver.
- SharedStorage requires the RAID/XFS mount to exist; host RAID/XFS/hugepage
  setup (`tools/configure-bench.sh` and friends) is a prerequisite, out of scope.
- The HF cache lives on `--model-fs` (default `/mnt/certus1`), not `/home` — the
  small `/home` partition fills up mid-download otherwise.
