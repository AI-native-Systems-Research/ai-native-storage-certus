# Runbook: compare Certus builds end to end with LongDocQA

How to measure whether a Certus change (a server build, a connector change, or both)
speeds up real vLLM serving, and how the 2026-10-26 comparison of optimizer bundles and of
the `BLHNC` KV layout was run. Results from that comparison are in
`knowledge/serving/observations.yaml`.

`RUNBOOK-certus-iostat.md` is the single-run recipe with SSD I/O capture. This one runs
the same workload repeatedly on several builds under identical settings, so the
differences between builds are not hidden by setup differences.

## What a run measures

`run_one.sh` formats a fresh certus-server, starts vLLM 0.30 with the Certus connector,
and runs `long_doc_qa.py` once: 51 documents × 10,000 tokens, 4 requests in flight.

- **Warmup round:** every document is prefilled and its KV stored to Certus (~62 GiB for
  Llama-3-8B), overflowing the 12 GiB memory tier onto SSD.
- **Query round:** the same documents again. vLLM finds each document's KV in Certus
  (external prefix hit ~49.7% over both rounds) and loads it instead of prefilling.

With `OUTPUT_LEN=1` (the default here) the query round is cold KV load plus a few tokens
of prefill, so it measures the load path. With 100 output tokens, decode dilutes the
difference between builds.

## One-time setup

### 1. Images (rootless podman, default store, ~44 GB)

```bash
podman pull docker.io/vllm/vllm-openai:v0.30.0                    # ~22 GB
# Connector image: the vLLM base plus certus_shmq_connector, installed editable.
# Built from any checkout; run_one.sh mounts the connector under test over it.
podman build --build-arg VLLM_VERSION=0.30.0 \
    -f certus-shmq-connector/Dockerfile -t localhost/certus-shmq-connector:vllm0.30.0 .
# Client image (pins long_doc_qa.py by sha256; see README.md)
podman build -t localhost/long-doc-qa:latest benchmarks/long-doc-qa
```

`certus-shmq-connector/build_connector_container.sh` builds the same connector image
into a separate store (`/mnt/certus1`); `run_one.sh` expects the default store, so use the
`podman build` line above.

### 2. Model

`NousResearch/Meta-Llama-3-8B` (15 GB) in `~/.cache/huggingface` (mounted read-only into
vLLM, `HF_HUB_OFFLINE=1`). Llama-3-8B's native context is 8,192 tokens, so `run_one.sh`
applies RoPE ×4 as `run_bench.sh` does. Its output past 8,192 tokens is junk: about 5 of 51
query requests stream only empty strings and count as failed. That is the model, not
storage. Compare builds on query round time, which counts every request, not on mean TTFT
alone.

### 3. Server binaries, one per build

Build each candidate in its own worktree and copy the binary out, so builds do not
overwrite each other:

```bash
git worktree add ../wt-base origin/unstable
git worktree add ../wt-cand <candidate-branch>
for w in base cand; do
  ( cd ../wt-$w
    for d in deps/spdk deps/spdk-build target; do ln -s "$OLDPWD/$d" "$d"; done   # share builds
    # target/ is shared and cargo's crate hashes are equal across worktrees, so make this
    # worktree's sources newer than the last build or cargo reuses another build's crates.
    git ls-files -z -- '*.rs' '*.toml' '*.c' '*.h' | xargs -0 touch
    LIBRARY_PATH=/usr/local/lib cargo build -p certus-server --release )
  cp ../wt-$w/target/release/certus-server ./certus-server-$w
done
```

Check each binary has its change (e.g. `strings certus-server-cand | grep <new symbol>`).
A change that is connector-only (Python) can reuse the base binary.

## Running a comparison

```bash
L=benchmarks/long-doc-qa
for round in 1 2 3; do               # rotate the order each round so drift is shared
  for b in base cand; do
    $L/run_one.sh --server ./certus-server-$b --connector ../wt-$b \
        --label $b --out results/$b-$round || echo "FAILED $b $round"
  done
done
$L/summarize_runs.py results --base base
```

About 5 minutes per run (vLLM startup ~1.5 min, warmup round ~80 s, query round ~4–8 s).
Exit code 3 means certus-server came up degraded; rerun that run.

Settings, all environment variables of `run_one.sh` (see its header):

| Variable | Default | Notes |
|---|---|---|
| `GPU` | 0 | the GPU on the NVMe drives' NUMA node (below) |
| `OUTPUT_LEN` | 1 | 100 to include decode |
| `CONNECTOR` | `certus` | `CertusConnector`, which requests the stock `LBHNC` (`KV_LAYOUT=BLHNC` sets vLLM's `VLLM_KV_CACHE_LAYOUT` for a `BLHNC` run); `stock` = vLLM's `OffloadingConnector` (`LBHNC`) |
| `KV_LAYOUT` | unset | sets `VLLM_KV_CACHE_LAYOUT`, which overrides the connector's request |
| `DRIVES`, `TIER` | 4 drives on this host, `12G` | as in `RUNBOOK-certus-iostat.md` |

## Reading the results

`summarize_runs.py` prints, per run and as medians per label:

- **query round time** (client): the headline. It counts all requests.
- **query TTFT** (client, successful requests only).
- **load / store / lookup time**: sums of vLLM's `kv_offload_load_time`,
  `kv_offload_store_time` and `kv_offload_lookup_sync_delay_seconds_sum` from its
  `KV Transfer metrics` log lines. Lookup time is what a connector-side lookup change
  (e.g. batched residency Checks) moves.
- **layout** and **regions per block** from the connector's log: `LBHNC` / 32 or `BLHNC` / 1
  for Llama-3-8B. A run that shows anything else did not get the layout you meant.
- **NUMA node of the memory tier**: must be the GPU's.

Also check `connector_identity.txt` in each run directory: the sha256 of the
`manager.py` vLLM imported must match the candidate checkout's.

SSD byte counts (`iostat_samples.csv`) stay 0 unless the server is built with the
`rw-telemetry` feature; the vLLM-side totals above do not need it.

## Pitfalls that invalidated runs

1. **GPU on the wrong NUMA node.** On this host GPU 1 is on NUMA node 1 and the drives,
   the memory tier and GPU 0 are on node 0. On GPU 1 the query round took 43 s instead of
   8 s and hid every difference between builds. `run_one.sh` warns when the GPU and the
   drives are on different nodes. Check with `nvidia-smi topo -m`.
2. **Starting a server while the previous one still holds the drives.** It logs
   `Cannot open /dev/vfio/<n>: Device or resource busy`, puts its memory tier on NUMA node
   -1, and loads ~2× slower. `run_one.sh` waits for the drives' VFIO groups to be free and
   exits 3 if the server still came up degraded. A server binary copied under another
   name does not match `pgrep -x certus-server`; match on the binary path.
3. **The connector under test is not the one vLLM runs.** The image has the connector it
   was built with; `run_one.sh` mounts `--connector`'s copy over it. Verify with
   `connector_identity.txt`.
4. **Stale crates from a shared `target/`** (setup step 3). A first build of an optimizer
   bundle failed to compile against another worktree's `interfaces` crate.
5. **Children inheriting the hardware lock.** Background processes started by a runner
   must close the lock's file descriptor (`9>&-` in `run_one.sh`), or the next run finds
   the lock held.
6. **Quoting the `--kv-transfer-config` JSON.** A `\"` inside single quotes is a literal
   backslash; vLLM then rejects the config at startup and the run fails before any load.

## Correctness before speed

A faster load of wrong bytes is not a gain. Before trusting a speed-up from a change to
the data path, check a byte-exact round trip at the deployment's layout (store stamped
random blocks, flush or overflow the memory tier, load back, compare every byte). The
2026-10-26 comparison did this for every build at 32 × 64 KiB pieces per block, from the
memory tier, from SSD, and with 7,000–7,500 blocks overflowing the tier. In LongDocQA,
query outputs differ from warmup outputs for the early documents; that is the RoPE-scaled
model's junk output, not corruption.
