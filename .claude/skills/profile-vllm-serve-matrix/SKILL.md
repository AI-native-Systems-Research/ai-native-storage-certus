---
name: profile-vllm-serve-matrix
description: >-
  Profile the vLLM KV-offload serving stack across vLLM versions
  (0.26/0.27/0.28/0.29/0.30) and produce throughput + mean-TTFT matrices
  comparing the Certus-SHMQ backend against vLLM's native cputier backend for
  the cc131k, mixtral, and multimodal workloads. Builds container images as
  needed via the helper scripts, drives each server with guidellm, and marks a
  cell X when a (version × backend × workload) combination will not run. Uses
  4× SSD (SPDK), a 60G memory tier (default, override with --mem-tier), and
  builds certus-server with the full-optimized profile (O(1) LRU + TinyLFU
  eviction) in release mode with the DRAM->SSD demotion threshold set to 0.8 so
  the NVMe spill path is exercised. This is a heavy, long-running,
  hardware-exclusive benchmark — invoke it explicitly.
argument-hint: "[--versions 0.26,0.27,...] [--workloads cc131k,mixtral,multimodal] [--backends certus,cputier] [--mem-tier 60G] [--max-seconds 1200] [--dry-run] [--out DIR]"
user-invocable: true
disable-model-invocation: true
metadata:
  author: Daniel Waddington
  source: apps/vllm-serve
---

# profile-vllm-serve-matrix

Build a comparison **matrix** of the vLLM KV-offload serving stack: for each
supported vLLM version, run the same workload against both KV-offload backends
and record **throughput** (total tokens/sec: prompt + output) and **mean TTFT** (ms). One
matrix per workload. A cell that cannot run (unsupported build, known crash,
storage/hardware incompatibility) gets an **`X`**.

```
Dimensions:
  versions  : 0.26.0  0.27.0  0.28.0  0.29.0  0.30.0        (override with --versions)
  backends  : certus (SHMQ)   |   cputier (vLLM native tiering)
  workloads : cc131k  |  mixtral  |  multimodal
  metrics   : throughput (total tok/s)  +  mean TTFT (ms)
Fixed hardware config (per the operator's standing requirement):
  storage   : 4× SSD
  mem tier  : 60 GiB
```

> **This runs real hardware exclusively.** It binds both A100s and the 4 NVMe
> SSDs, launches `certus-server`, builds containers, and pulls models. A full
> 5×2×3 sweep is dozens of server launches and hours of wall-clock. Confirm the
> node is idle before starting. Support `--dry-run` to print the full plan
> (every cell, every command, every image to build) **without executing** — do
> that first and show it to the operator.

---

## 0. Arguments & defaults

Parse `$ARGUMENTS` (all optional):

| flag | default | meaning |
|------|---------|---------|
| `--versions`  | `0.26.0,0.27.0,0.28.0,0.29.0,0.30.0` | vLLM patch tags (FULL tag, not `0.26`). |
| `--workloads` | `cc131k,mixtral,multimodal` | subset of workloads to run. |
| `--backends`  | `certus,cputier` | subset of backends. |
| `--mem-tier`  | `60G` | memory tier size (**do not lower without being asked** — the requirement is 60G). |
| `--max-seconds` | `1200` | per-cell guidellm run duration in seconds, applied uniformly to **every** workload's driver (overrides each driver's own default, e.g. cc131k/multimodal's 600s and moe's 180s). |
| `--drives`    | `4` | SSD count for the certus SPDK server (**requirement: 4**). |
| `--cputier-fs-path` | *(none — ask)* | host directory backing cputier's fs disk tier (`FS_TIER_HOST`). |
| `--out`       | `/mnt/certus1/vllm-serve-matrix/<timestamp>` | results dir (**must be on /mnt/certus1, never /home**). |
| `--dry-run`   | off | print the plan and exit without running anything. |

**cputier fs-tier path is required and has no default.** If `--cputier-fs-path`
is not passed (and the cputier backend is in scope), **ask the operator which
filesystem path to use** for cputier's disk tier before doing anything else —
e.g. via AskUserQuestion, offering a sensible candidate like
`/mnt/certus1/kv-fs-tier` plus "Other". Do not silently fall back to a hardcoded
path. Record the chosen path in the results header. On `--dry-run`, still ask
(or state the path if given) so the printed plan is concrete. This directory is
created if missing and bind-mounted into the cputier serve container.

All paths under `apps/vllm-serve/` are relative to the repo root
`/home/dwaddington/ai-native-storage-certus`. This skill's helper lives beside
this file: `extract_metrics.py`.

---

## 1. Backend & workload reference

### Backends

| backend | server side | image (per version `<v>`) | store |
|---------|-------------|---------------------------|-------|
| **certus** | host `certus-server-yaml`, SPDK, 4 drives, 60G tier, 64 channels | `localhost/certus-shmq-connector:vllm<v>` | `/mnt/certus1/podman/{storage,run}` (needs `--root/--runroot`) |
| **cputier** | none — self-contained in the serve container | `certus-offload:vllm<v>` (stock) — **except 0.26 uses `certus-offload-fix026`** | DEFAULT podman store (no flags) |

- **certus** uses the 4 SSDs via SPDK (`certus-server-yaml --drive-count 4`);
  cputier's fs disk tier writes under the operator-supplied `--cputier-fs-path`
  (`FS_TIER_HOST`). Both get the same **60G memory tier**. Serve script:
  `apps/vllm-serve/run-serve-certus-shmq.sh`.
  It is CLIENT-ONLY — a `certus-server` must already be publishing the mailbox at
  `SHM_PATH` (`/dev/shm/certus-shmq`). `VLLM_FIX3=1` (default) is a no-op on
  0.30, a defensive clamp on 0.27–0.29, and load-bearing on 0.26 — leave it on
  for every version.
- **cputier** serve script: `apps/vllm-serve/run-serve-cputier.sh`. Self-
  contained; `CPU_BYTES=<mem-tier>`, `FS_TIER=1`, `FS_TIER_HOST=<--cputier-fs-path>`.
  The **0.26** cell must use `certus-offload-fix026` (vLLM 0.26.0 + fix#2
  deferred-finalize overlay); without fix#2 the tiering path crashes under load
  (`_req_state` KeyError). fix#2 is **0.26.x-only** — on 0.27–0.30 the stock
  `certus-offload:vllm<v>` image is used and fix#2 is not available, so those
  cputier cells are **crash-risk** (see §5 known-X).

> The `run-serve-cputier-*-2gpu.sh` wrappers HARD-PIN `CPU_BYTES=30G` and would
> clobber the 60G requirement. **Do not use the wrappers.** Call
> `run-serve-cputier.sh` directly with the full model env + `CPU_BYTES=60G`.

### Workloads (model + serve env + guidellm driver)

| workload | model / served-name | serve env (both backends unless noted) | guidellm driver |
|----------|---------------------|----------------------------------------|-----------------|
| **cc131k** | `Qwen/Qwen2.5-14B-Instruct` / `qwen2.5-14b` | `MAX_MODEL_LEN=131072 TENSOR_PARALLEL=2 GPU=all GPU_MEM_UTIL=0.9 DTYPE=float16` (YaRN auto-enables) | `run-guidellm-cc131k.sh` (throughput, mooncake-131k trace, 600s) |
| **mixtral** | `RedHatAI/Mixtral-8x7B-Instruct-v0.1-FP8` / `mixtral-8x7b` | `DTYPE=auto MAX_MODEL_LEN=32768 TENSOR_PARALLEL=2 GPU_MEM_UTIL=0.85` | `run-guidellm-synthetic-moe.sh` (concurrent=32, 180s) |
| **multimodal** | `Qwen/Qwen3-VL-32B-Instruct-FP8` / `qwen3-vl-32b` | `DTYPE=auto MAX_MODEL_LEN=32768 TENSOR_PARALLEL=2 GPU_MEM_UTIL=0.80 EXTRA_SERVE_ARGS='--limit-mm-per-prompt {"image":2,"video":0}'` | `run-guidellm-synthetic-multimodal.sh` (concurrent=32, 1080p, 600s) — needs `guidellm[vision]`/Pillow |

The guidellm drivers set `TARGET=http://127.0.0.1:${PORT}` and the right
`MODEL`/`PROCESSOR`; you only need the server up on `PORT` (default 8000). Each
driver writes `OUTPUT` JSON + a `.metrics.txt`; capture the JSON path per cell.

The per-driver run durations shown above (600s / 180s / 600s) are the drivers'
own defaults; this skill overrides all of them with `MAX_SECONDS=$MAXSECS`
(from `--max-seconds`, default 1200) so every cell runs for the same wall-clock.

---

## 2. Prerequisites (verify, don't assume)

Run these checks first; on failure, stop and tell the operator what to fix (some
need sudo and are the operator's to run):

1. **Repo / branch.** Working on a feature branch (never `unstable`). This skill
   only reads scripts + writes results; it doesn't commit.
2. **GPUs free.** `nvidia-smi` shows 2 A100s with no other process holding them.
3. **guidellm on the host.** `guidellm --version`. Multimodal additionally needs
   `python -c "import PIL"` (Pillow) or the scheduler DEADLOCKS — if the
   multimodal workload is selected and Pillow is missing, mark all multimodal
   cells X with reason `no-guidellm-vision` (or have the operator
   `pip install 'guidellm[vision]'`).
4. **SPDK host state (certus only).** The 4 NVMe (`0000:61:00.0 0000:62:00.0
   0000:63:00.0 0000:64:00.0`) must be `vfio-pci`-bound with hugepages reserved.
   `certus-shmq-connector/setup-host.sh` / `tools/configure-bench.sh` do this
   (sudo). If not bound, mark all **certus** cells X with reason `no-vfio` unless
   the operator sets it up.
5. **Podman stores.** shmq images: `/mnt/certus1/podman/storage`. cputier
   images: default store. Both resolvable.
6. **Disk space + HF cache** on `/mnt/certus1` (models are large; FP8 checkpoints
   download on first use to `/mnt/certus1/hf-cache`).

---

## 3. Procedure

Group the work **by backend** so each backend's server topology is set up once.
Within a backend, iterate versions, and within a version iterate workloads.
Between cells you MUST fully tear down the serve container (it holds both GPUs)
before starting the next.

### 3.0 Setup

```bash
OUT="${OUT:-/mnt/certus1/vllm-serve-matrix/$(date +%Y%m%d_%H%M%S)}"
mkdir -p "$OUT"
# Resolve the tunable knobs from the arguments (§0). MEMTIER is the tier size that
# feeds BOTH backends so they share an identical tier — but in two forms:
#   - certus  wants the human string ("60G") for run-bench-spdk-4drive.sh --mem
#     (-> --memory-tier-size, which parses human sizes).
#   - cputier wants raw BYTES for CPU_BYTES (the serve script uses it in shell
#     arithmetic and injects it as cpu_bytes_to_use:<int> in JSON — "60G" breaks it).
# DRIVES is fixed at 4 by requirement.
MEMTIER="<from --mem-tier, default 60G>"           # human string, e.g. 60G
MEMTIER_BYTES="$(numfmt --from=iec "$MEMTIER")"    # bytes for cputier CPU_BYTES
DRIVES="<from --drives, default 4>"                # requirement: 4
MAXSECS="<from --max-seconds, default 1200>"       # per-cell guidellm run duration (s);
                                                   # exported as MAX_SECONDS to every driver.
# CPUTIER_FS_PATH: the cputier fs-tier host dir. NO default — resolve it from
# --cputier-fs-path, else ASK the operator (§0) before this phase runs.
CPUTIER_FS_PATH="<from --cputier-fs-path or the operator's answer>"
mkdir -p "$CPUTIER_FS_PATH"
# results.tsv accumulates one row per cell; the matrices are rendered from it at the end.
: > "$OUT/results.tsv"   # columns: workload  version  backend  thr  ttft  reqs  status  json
```

For each cell, after the guidellm JSON is written, append a row:

```bash
read thr ttft reqs < <(python3 "$SKILL_DIR/extract_metrics.py" "$CELL_JSON" \
  | sed -E 's/thr=([^ ]*) ttft=([^ ]*) reqs=([^ ]*).*/\1 \2 \3/')
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
  "$WL" "$V" "$BK" "$thr" "$ttft" "$reqs" "$STATUS" "$CELL_JSON" >> "$OUT/results.tsv"
```

`STATUS` ∈ `ok | build-failed | server-failed | serve-crashed | driver-failed |
skipped-<reason>`. Any status other than `ok`, or `thr=X`, renders as **`X`**.

### 3.1 Image build (both backends, per version)

Build only what's missing. Never rebuild an image that `podman image exists`
already finds.

```bash
# certus (shmq) image -> /mnt/certus1 store
command podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run \
  image exists "localhost/certus-shmq-connector:vllm$V" \
  || certus-shmq-connector/build_connector_container.sh "$V"

# cputier image -> default store
if [[ "$V" == 0.26.* ]]; then
  command podman image exists certus-offload-fix026 \
    || IMAGE=certus-offload-fix026 FIX_TIERING=1 \
         bash benchmarks/kv-offload-replay/build_cputier_container.sh 0.26.0
else
  command podman image exists "certus-offload:vllm$V" \
    || bash benchmarks/kv-offload-replay/build_cputier_container.sh "$V"
fi
```

If a build fails, mark **every** cell for that (version × backend) `X` with
`STATUS=build-failed` and move on — do not abort the whole matrix.

### 3.2 certus (SHMQ) phase

Launch **one** `certus-server` for the whole certus phase (it is model-agnostic
— content-addressed KV blocks; `SLAB_SIZE_BYTES=2097152` must match the
connector default, which it does). Recommended: restart with `--format` between
**workloads** (different models) for a cold tier; keep it up across a single
workload's version sweep.

```bash
# 4 drives, 60G tier, 64 channels (--channels 64: clears the membership channel
# race floor of 32 AND gives headroom for the vLLM 0.29+ LBHNC KV layout, which
# exhausts a 32-channel budget under concurrent load).
# --profile full-optimized: build certus-server with the O(1) LRU + TinyLFU
#   eviction policy (REQUIRED — the plain `full` profile is the script default and
#   must NOT be used here). The profile is baked in at compile time.
# --eviction-threshold 0.8 (REQUIRED): DRAM memory tier demotes to the NVMe SSDs
#   at 80% full. Without it the server default is 0.0 (demotion DISABLED) and the
#   tier is DRAM-only — the 4 SSDs never take spill, so the storage path under
#   test is not actually exercised.
# --format initialises the on-disk layout (destroys tier data —
# fine for a benchmark). Launch in the BACKGROUND and wait for the mailbox.
scripts/run-bench-spdk-4drive.sh --server-only --format \
  --profile full-optimized --eviction-threshold 0.8 --drives "$DRIVES" --mem "$MEMTIER"
#   (run_in_background; it blocks holding the server. Wait until /dev/shm/certus-shmq exists,
#    then proceed. Kill this job to tear the server down.)
```

> `run-bench-spdk-4drive.sh` builds `certus-server-yaml` with the profile passed
> via `--profile` (`CERTUS_PROFILE=full-optimized cargo build --release
> -p certus-server-yaml --features spdk` — the build is **always `--release`**)
> and launches `numactl --cpunodebind=0 --membind=0
> target/release/certus-server-yaml --drive-count <n> --shm-path
> /dev/shm/certus-shmq --channels 64 --memory-tier-size <MEMTIER>
> --memory-tier-eviction-threshold 0.8 [--format]`.
> Pass `--profile full-optimized` explicitly: the script's own default is the
> plain `full` profile, which lacks the O(1) LRU + TinyLFU evictor. Remember the
> stale-binary trap: the replay path uses `target/release/certus-server` and does
> NOT auto-rebuild — but this launcher builds `certus-server-yaml` explicitly, so
> a fresh optimized `--features spdk` build is guaranteed here.

Then per version, per workload:

```bash
PORT=8000
IMG="localhost/certus-shmq-connector:vllm$V"
# launch serve (background), passing the workload's model env (§1 table):
IMAGE="$IMG" MODEL=… SERVED_MODEL_NAME=… DTYPE=… MAX_MODEL_LEN=… \
  TENSOR_PARALLEL=2 GPU=all GPU_MEM_UTIL=… PORT=$PORT [EXTRA_SERVE_ARGS=…] \
  apps/vllm-serve/run-serve-certus-shmq.sh          # run_in_background
# wait for readiness (curl -sf http://127.0.0.1:$PORT/v1/models), with a timeout
#   (e.g. 15 min for a cold FP8 download). On timeout: STATUS=server-failed, X, teardown.
# run the workload's guidellm driver against it (MAX_SECONDS sets the run duration):
OUTPUT="$OUT/certus-$V-$WL.json" TARGET=http://127.0.0.1:$PORT MAX_SECONDS="$MAXSECS" \
  apps/vllm-serve/run-guidellm-<driver>.sh
# extract + append row (§3.0). Then TEAR DOWN the serve container before next cell:
command podman --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run \
  ps --filter "ancestor=$IMG" -q | xargs -r command podman \
  --root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run kill
```

At the end of the certus phase (or before switching workload for a cold tier),
kill the `certus-server` background job and wait for `/dev/shm/certus-shmq` to
disappear.

### 3.3 cputier phase

No external server. Per version, per workload — call `run-serve-cputier.sh`
**directly** (not the 2gpu wrappers) with `CPU_BYTES=$MEMTIER` so cputier's CPU
tier matches the certus memory tier exactly (both driven by `--mem-tier`):

```bash
PORT=8000
if [[ "$V" == 0.26.* ]]; then IMG=certus-offload-fix026; else IMG="certus-offload:vllm$V"; fi
rm -rf "${CPUTIER_FS_PATH:?}/"* 2>/dev/null || true    # COLD disk tier for THIS cell
IMAGE="$IMG" MODEL=… SERVED_MODEL_NAME=… DTYPE=… MAX_MODEL_LEN=… \
  TENSOR_PARALLEL=2 GPU=all GPU_MEM_UTIL=… PORT=$PORT [EXTRA_SERVE_ARGS=…] \
  CPU_BYTES="$MEMTIER_BYTES" FS_TIER=1 FS_TIER_HOST="$CPUTIER_FS_PATH" \
  apps/vllm-serve/run-serve-cputier.sh              # run_in_background
# wait for /v1/models; run the driver (same as certus, incl. MAX_SECONDS="$MAXSECS"):
#   OUTPUT="$OUT/cputier-$V-$WL.json" TARGET=http://127.0.0.1:$PORT MAX_SECONDS="$MAXSECS" \
#     apps/vllm-serve/run-guidellm-<driver>.sh
# then extract + append row.
# teardown (default store, no --root):
command podman ps --filter "ancestor=$IMG" -q | xargs -r command podman kill
# CLEAR THE FS TIER CACHE AFTER THIS EXPERIMENT — must be AFTER teardown (the dir
# is bind-mounted into the running container; never rm under a live mount):
rm -rf "${CPUTIER_FS_PATH:?}/"* 2>/dev/null || true
```

`SHM_BYTES` auto-sizes to `CPU_BYTES + 4G`; the container `--shm-size` follows.
Ensure the host has ≥ ~`MEMTIER + 4G` MemAvailable for the pinned CPU tier (the
script warns if not). **Clear `$CPUTIER_FS_PATH` after every cell** (shown above, post-
teardown) so each experiment runs against a cold disk tier and no residue leaks
into the next version/workload — the leading `rm` guards the case where a prior
cell crashed before its own cleanup ran.

### 3.4 Robustness rules

- **Every cell is independent.** Wrap each in failure handling: a crash, an OOM,
  a startup timeout, or a driver error marks that one cell `X` and continues.
- **Never leave a server holding the GPUs.** Always run teardown even on failure
  (trap/`finally`-style). Confirm `nvidia-smi` is clear before the next launch.
- **One serve container at a time** — both GPUs are TP=2.
- **Cold FS tier per experiment.** For every cputier cell, delete the contents of
  `$CPUTIER_FS_PATH` after the cell's teardown (and defensively before its launch)
  so no cached KV blocks carry over between versions/workloads. Clear only after
  the container is killed — the path is bind-mounted live. (The certus backend
  gets its cold tier from `certus-server --format`; this rule is the cputier
  equivalent.)
- Save every guidellm JSON + `.metrics.txt` under `$OUT/` so cells are auditable.

---

## 4. Output

Render **three matrices** (one per workload) from `$OUT/results.tsv`, plus a
config header. Print them to the chat AND save to `$OUT/MATRIX.md`. Format:

```
# vLLM serve KV-offload matrix — <timestamp>
Storage: 4× SSD · cputier fs tier: <CPUTIER_FS_PATH> · Memory tier: <MEMTIER> (certus --memory-tier-size / cputier CPU_BYTES) · certus profile: full-optimized (release), DRAM→SSD demotion @ 0.8 · run duration: <MAXSECS>s/cell · TP=2 · guidellm on host
Metric cells: throughput total tok/s  /  mean TTFT ms   (X = did not run; see notes)

## cc131k   (Qwen2.5-14B, 131k ctx, throughput profile)
| vLLM  | certus thr | certus TTFT | cputier thr | cputier TTFT |
|-------|-----------:|------------:|------------:|-------------:|
| 0.26.0|      NN.NN |       NNN.N |       NN.NN |        NNN.N |
| 0.27.0|      NN.NN |       NNN.N |         X   |          X   |
| …     |            |             |             |              |

## mixtral   (Mixtral-8x7B-FP8, 32k, concurrent=32)
| … same shape … |

## multimodal   (Qwen3-VL-32B-FP8, 1080p, concurrent=32)
| … same shape … |

### Notes
- 0.NN cputier X: <reason> (e.g. fix#2 tiering crash under load — 0.26.x-only overlay).
- <any per-cell failure reason captured in STATUS>.
```

Round throughput to 2 dp, TTFT to 1 dp. Any cell with `thr=X` or non-`ok`
`STATUS` prints `X` (both sub-columns) with a footnote naming the reason.

---

## 5. Known likely-X cells (expectation, still verify by running)

Don't pre-fill these — run and let the result speak — but expect and be ready to
explain:

- **cputier, 0.27 / 0.28 / 0.29 / 0.30:** fix#2 (`_req_state` KeyError in
  `TieringOffloadingManager` under load) is a **0.26.x-only** overlay. Stock
  images for these versions may crash under sustained store load → `X`
  (`serve-crashed`, reason `no-fix2`). There is also a second tiering bug
  (`_build_store_jobs` offload_keys/block_ids length) that fix#2 does not cover;
  it may surface here too.
- **cputier, multimodal (any version):** VL activation memory on 40G A100s is
  tight at `GPU_MEM_UTIL=0.80`; OOM at startup → `X` (`server-failed`).
- **certus, any version:** if the host is not vfio/hugepage-configured → all
  certus cells `X` (`no-vfio`) until the operator runs `setup-host.sh`.
- **multimodal, both backends:** missing `guidellm[vision]`/Pillow → scheduler
  deadlock; guard by pre-checking and marking `X` (`no-guidellm-vision`) rather
  than hanging.
- **certus & cputier, 0.30:** `apply_fix3.py` no-ops on 0.30 (the clamp is
  upstream) — this is correct, not a failure; 0.30 certus should run.

---

## 6. Guardrails (standing operator constraints)

- Results on **/mnt/certus1**, never /home. Temp files under `$CLAUDE_JOB_DIR/tmp`.
- Feature branch only; **never** commit to `unstable`. This skill does not commit
  results or the skill itself unless the operator asks; if asked, stage only the
  intended files (never `git add -A`) and keep the tree clean for
  `spec-sync-hash.sh`.
- Do not rebind NVMe or reconfigure the host without explicit instruction — those
  need sudo.
- `--dry-run` first: print the full plan (cells, images to build, storage config)
  and get a go-ahead before consuming hours of exclusive hardware time.
```
