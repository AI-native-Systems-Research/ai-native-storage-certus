# certus-connector-bench

Benchmarks for the Python connector classes (`certus-shmq-connector`) that vLLM uses to talk to certus-server. Unlike `certus-fio` (which calls `Ring.populate`/`Ring.lookup` directly), these exercise the full connector code path: BLAKE2b key hashing, TP key namespacing, per-block region construction, `ThreadPoolExecutor` dispatch, and the lookup cache.

Requires a running `certus-server` with shared memory at `/dev/shm/certus-shmq`.

## Quick start

Start the server (the benchmark never starts or stops it), then run a mode. From the repo root:

```bash
# 1. the server: the settings the benchmark's defaults assume (12 GiB memory tier, evict at 0.8)
sudo target/release/certus-server \
    --drive-count 4 --channels 32 \
    --memory-tier-size 12G --memory-tier-eviction-threshold 0.8 \
    --format

# 2. the benchmark, in another shell, each with no shape flags:
export PYTHONPATH=certus-shmq-connector:apps/python:benchmarks/certus-connector-bench
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py                     # store, warm, cold, mixed eviction
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py --mode contention   # loads + stores in flight together
```

With no shape flags the default mode's store, warm and cold passes and the contention mode run
64-block jobs, 4 in flight, 32 pieces per 2 MiB block, which is vLLM 0.30's default for
Llama-3-8B (see "In-flight passes"). Nothing else is needed on the command line:

- **`--num-blocks` (the working set) is optional.** The default and contention modes derive it
  from the tier: 90% of what it holds before evicting, at most 4,000.
- **`--tier-gib` is optional and defaults to 12.** Pass it only when you start the server with a
  different `--memory-tier-size`, e.g. `--memory-tier-size 32G` and `--tier-gib 32`. The benchmark cannot
  read the tier size from the server, so a wrong value is not detected; every run prints
  `Tier assumed: ...` first (see "Memory tier size").
- `--channels` must exceed the jobs in flight (the modes tell you the number they need; 16 is enough
  for the defaults).

```bash
# a server with a 32 GiB tier
sudo target/release/certus-server --drive-count 4 --channels 32 \
    --memory-tier-size 32G --memory-tier-eviction-threshold 0.8 --format
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py --tier-gib 32
```

## Prerequisites

The benchmark does **not** start or stop the server. You must start it before running any benchmark:

```bash
sudo target/release/certus-server \
    --drive-count 4 \
    --channels 32 \
    --memory-tier-size 12G \
    --memory-tier-eviction-threshold 0.8 \
    --format
```

The tier size and eviction threshold above are what the default and contention modes assume (`--tier-gib 12`,
`--eviction-threshold 0.8`); if you change them, pass the same values to the benchmark.

Server parameters (not controlled by the benchmark):

| Parameter | Default | Description |
|---|---|---|
| `--drive-count` | (required) | Number of NVMe drives (or simulated drives) |
| `--channels` | 8 | Mailbox channels = max concurrent requests. Use 16+ for the in-flight passes and contention |
| `--memory-tier-size` | (required) | DRAM cache size (e.g. `4G`) |
| `--memory-tier-eviction-threshold` | 0.0 | Start evicting when tier is this fraction full (0.7 = 70%) |
| `--shm-path` | `/dev/shm/certus-shmq` | Shared memory path |
| `--max-eviction-attempts` | 2048 | Per-reserve eviction retry limit |
| `--store-backpressure-ms` | 5000 | Max wait for reserve under pressure |
| `--format` | — | Wipe drives on startup |

To sweep drive counts, restart the server between runs and use `--tag` in the benchmark to label each:

```bash
for drives in 1 2 4; do
    sudo target/release/certus-server --drive-count $drives --channels 16 \
        --memory-tier-size 4G --memory-tier-eviction-threshold 0.7 --format &
    sleep 3
    python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
        --mode all --min-duration 5 --csv bench-results/drive_sweep.csv \
        --tag "${drives}-drive"
    sudo kill %1; wait
done
```

## Scripts

### bench_connector_lifecycle.py

End-to-end lifecycle benchmark with multiple modes. Without `--mode`, runs the default mode (store, warm load, cold load, mixed eviction). Each mode targets a different optimization surface.

**Call order.** Wherever a mode both looks up and touches the same keys (warm/cold load, the load side of mixed eviction, prefix-miss, the contention loads, scheduler-step, pattern loads), it calls `lookup` first and `touch` after, as vLLM's offloading scheduler does since vLLM 0.21 (vLLM PR #41228; still so in 0.30). `--lookup-order touch-first` restores the vLLM ≤ 0.20 order, where `touch` came first and the connector could answer that request's lookups from the touch reply; with the default batch sizes it reproduces the numbers this benchmark gave before the option existed (a lookup more than `CERTUS_LOOKUP_CACHE_TTL_US` after its touch is a Check even in this order, so very large batches can differ). With the default order a request's lookups can only be answered from an earlier request's touch reply (within the connector's `CERTUS_LOOKUP_CACHE_TTL_US`, default 1000 µs); all other lookups are one Check round trip each. A lookup that misses the cache also Checks the keys committed with it (in one `complete_store` through the same manager) in that one round trip, and the answers serve the following lookups under the same TTL. Store-only paths touch before `prepare_store`, as before.

#### Default mode (no `--mode` or `--mode default`)

Four passes, in this order. Store and warm load run `--inflight` jobs of `--job-blocks`
outstanding through vLLM 0.30's step loop (see "In-flight passes"); the cold pass is the
committed cold pass (below), with the same `--job-blocks` and `--inflight`; `--step-us` is the
poll interval of all three.

1. **Store** (`Store Lifecycle (...)`): prepare_store → touch, submit next tick, reaped →
   complete_store. Each pass's write-through drain and key removal (`reset`) are outside the
   timing; `Sustained incl. SSD write-through drain` includes them.
2. **Warm Load** (`Warm Load Lifecycle (...)`): the working set is stored, written through and
   stays resident; lookup → touch → prepare_load → submit_load, reaped → complete_load, jobs in
   a shuffled order per pass.
3. **Cold Load** (`Cold Load Lifecycle (...)`): same path, SSD→DRAM→GPU. Before every pass the
   tier is flushed and emptied (untimed); jobs of `--job-blocks` in key order. With the default
   cold shape and `--inflight 1` it is the plain sequential cold pass (one job, sleep-polled
   every `--step-us`, phase `demote`); otherwise the shaped cold pass keeps up to `--inflight`
   jobs outstanding (submitted until the slots are full, then one `get_finished()` poll per
   `--step-us`). This is not the step loop: it is the cold pass as committed before the step-loop
   refactor, kept so the cold numbers stay comparable. It has no `--verify`. The cold-load shape
   flags below change it.
4. **Mixed Eviction**: interleaved store + load of `--bs` batches under memory-tier pressure,
   one job at a time, with `take_events` drain (`--no-eviction` skips it).

`--no-warm` and `--no-cold` skip passes 2 and 3. With `--no-warm` the working set may be larger
than the tier (the warm pass needs it resident; a store-only run does not): the stores then wait
for the write-through, which is what the workbench's sustained-store guards measure
(`--no-warm --no-cold --no-eviction --num-blocks 5500`).

`--job-blocks 16 --inflight 1` runs the first three one 16-block job at a time, which is what
this mode did before it ran in flight. Measured in one server session against the committed
modes (2026-11-05, 3 runs each, alternating): warm 14.35 vs 14.16 GB/s (warm-inflight 16 x 1)
and 14.78 (old sequential pass), store 8.74 vs 8.97 and 9.13, cold 9.55 vs 9.48; at the
defaults (64 x 4) warm 23.29 vs 23.29, store 14.54 vs 14.62, cold 24.07 vs 24.13. All within
run-to-run spread (up to 12 % at 16 x 1).

```bash
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --shm-path /dev/shm/certus-shmq

# Longer run, larger eviction working set
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --min-duration 30 --working-set 2048
```

#### Prefix-miss mode (`--mode prefix-miss`)

Measures the cost curve of vLLM's `_maximal_prefix_lookup` at varying hit ratios. Pre-stores a fraction of keys, then runs a per-key sequential lookup breaking at the first miss, followed by a touch of the batch (touch first with `--lookup-order touch-first`).

```bash
# 75% prefix hit ratio (default)
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --mode prefix-miss --hit-ratio 0.75

# Compare: all-miss vs all-hit
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --mode prefix-miss --hit-ratio 0.0
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --mode prefix-miss --hit-ratio 1.0
```

Reports the latency of lookups answering hit and miss separately (`lookup_hit`, `lookup_miss`), plus average prefix length before the first miss.

#### Contention mode (`--mode contention`)

Loads and stores in flight together on one worker, as in serving: `--inflight` load jobs of
`--job-blocks` over the resident working set (default 64 × 4) and `--store-inflight` store jobs of
`--store-job-blocks` fresh keys (default 32 × 1), through the same step loop. Nothing is excluded:
the stores' write-through and any eviction it forces run inside the window, as do `take_events`
drains. Load keys evicted by then miss (`Blocks not loaded`); refused reservations are `Store drops`.
Reveals server-side contention (reserve vs pin, copy_to_store vs lookup, write-through vs promote).

```bash
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py --mode contention --min-duration 10
```

Reports `Contention: Store Thread (...)` and `Contention: Load Thread (...)` (the names are kept
from the earlier two-thread mode; both now run on the one step loop), and `Contention: Load Alone
(...)`: before the window the same loads run alone for 3 s (`CONTENTION_ALONE_SECONDS`) on the
same manager, worker and working set, so the only difference is the concurrent store. Load Alone
is printed only; the YAML, CSV and `--output` carry Store Thread and Load Thread, as before.
`Load service throughput` prints both. The two-thread mode, one 16-block batch at a time with a
second manager and worker, is gone; its numbers are not comparable with these.

#### Scheduler-step mode (`--mode scheduler-step`)

Simulates continuous-batching scheduler steps, driven as vLLM 0.30's offloading connector is (`offloading/scheduler.py`, `worker.py`), on the same step loop as the in-flight passes:

- Each step admits up to `--requests-per-step` new requests. Each request does a maximal-prefix lookup (per-key `lookup`, stopping at the first miss) and then a `touch` of all its keys (touch first with `--lookup-order touch-first`).
- A request's prefix hits are **one load job of its own** (vLLM: one job per request), submitted that step without waiting. At most `--inflight` load jobs are outstanding (default 4); a request that would need another one waits for a later step, lookup and all, as vLLM re-runs a delayed request's lookup.
- Hit keys another outstanding job is already loading are not loaded twice: the request waits for that job (vLLM delays a request whose hit chunks are being loaded, then finds them in its GPU prefix cache).
- Once a request's loads are reaped it computes its suffix: its store is prepared that step (`prepare_store`, then `touch`) and submitted at the start of the next, as vLLM defers stores past the step's sampling.
- Finished jobs are reaped by one non-blocking `get_finished()` per tick, then the loop sleeps to the next `--step-us` boundary (default 100 µs). Nothing waits for a particular job.
- With `--prefix-sharing shared` (default), sessions share one prefix pool and diverge at geometric points (mean = half a request), so short prefixes miss more, as with production prefix caching. With `--prefix-sharing private`, each session gets its own prefix blocks of the same lengths, so no prefix block is shared between sessions. All sessions' prefix blocks are stored up front (about `num_sessions` × the mean prefix length), which can exceed the memory tier and leave some of them on SSD. Keys are never removed between steps, so the cache fills and evicts naturally.

Until 2026-11-06 this mode merged each step's loads into one job and waited for it inside the step (one job in flight, transfer and host work in series), and `--inflight` had no effect. Its numbers from before then are not comparable with the current ones.

```bash
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --mode scheduler-step --requests-per-step 8 --num-sessions 32

# Heavier load
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --mode scheduler-step --requests-per-step 16 --num-sessions 64 \
    --min-duration 30

# vLLM <= 0.20 call order, and no prefix shared between requests
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --mode scheduler-step --lookup-order touch-first
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --mode scheduler-step --prefix-sharing private
```

Reports throughput (blocks loaded plus stored, over the timed window), steps/s and requests/s, prefix hit and suffix store counts, per-call phases (lookup, touch, prepare_load, submit_load, prepare_store, store_touch, submit_store, complete_load, complete_store, take_events; `step` = one tick's scheduling work; `reap_call` = the one `get_finished()` per tick) and each job's submit-to-reap time (`load_job`, `store_job`), with jobs outstanding and concurrency per direction.

#### Pattern mode (`--pattern`)

Runs a certus-fio YAML workload pattern through the connector lifecycle.

```bash
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --pattern cold_prefill_store

python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --pattern warm_prefill_load_and_suffix_store
```

#### Run all modes

```bash
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py --mode all --min-duration 5
```

Runs default, prefix-miss, contention and scheduler-step in turn, each with its own
defaults (a tier-derived `--num-blocks` for default and contention, 128 blocks for the
others, unless given; 32 pieces per block in every mode). Every mode's arguments are checked before
connecting.

`--mode pipelined`, `--pipeline-depth` and `--direction` are gone: the default mode's
store and warm passes keep `--inflight` jobs outstanding, which is what pipelined measured.

#### Common options

```
--shm-path PATH        certus-server shmq mailbox path (default: /dev/shm/certus-shmq)
--bs N                 Batch size in blocks (default: 16): prefix-miss, scheduler-step,
                       pattern and mixed eviction; the in-flight passes use --job-blocks
--num-blocks N         Working set in blocks (default: from the tier, at most 4000, for default and
                       contention; 128 for the others)
--block-bytes N        Per-block size in bytes (default: 2097152 = 2 MiB)
--pieces-per-block N   KV regions per block (default 32 in every mode);
                       --layers N is the same flag (the sibling benches' name for it)
--gpu N                CUDA device index (default: 0)
--tp N                 Tensor-parallel world size: one worker per GPU (default: 1, see below)
--gpus LIST            Devices for the TP ranks, e.g. 2,3 (default: --gpu..--gpu+tp-1)
--min-duration SECS    Minimum seconds per phase (default: 5.0)
--workers N            ThreadPoolExecutor worker count (default: 4)
--working-set N        Blocks per generation of mixed eviction (default: 512); contention ignores it
--csv PATH             Append results to CSV file
--tag TEXT             Tag column for CSV (e.g. branch name)
--lookup-order ORDER   lookup-first (default, vLLM >= 0.21) or touch-first (vLLM <= 0.20)
--pieces-per-block N   GPU tensors per block (default 32 in every mode, Llama-3-8B under LBHNC), see below
--cold-tier-state S    empty (default) or full, with --tier-fill-blocks N; see "Cold-load shape"
--tier-gib G            the server's --memory-tier-size in GiB (default 12); see "Memory tier size"
--eviction-threshold T  the server's --memory-tier-eviction-threshold (default 0.8)
--store-order O        batch (default) or interleaved
--load-unit U          batch (default) or request
--requests R           requests for interleaved/request (default: 16)
--seed N               job order of the in-flight passes (default: 0)
--preset NAME          named cold-load shape: longdocqa
```

#### Tensor parallelism (`--tp`)

`--tp 2` runs the vLLM `--tensor-parallel-size 2` pattern: one scheduler-side
manager and one `CertusShmqWorker` per GPU (rank 0 in the benchmark process on
device `--gpu`, rank i in a spawned process on device `--gpu + i`; `--gpus`
picks other devices). Every rank receives the same transfer spec, moves its own
shard, and a job counts as finished only once all ranks report it.
`--block-bytes` is each rank's shard size, so to model a fixed model under TP
pass the single-GPU block size divided by the TP size. Throughput counts all
shards (`block_bytes × tp` per logical block).

Each process claims shmq channels from its own slice, as the production
connector does, so the server needs `--channels` of at least
`tp × (1 + workers)`. The benchmark checks this up front and names the value to use.
`--verify` is TP=1 only.

Results recorded with `--tp > 1` before this change came from a single-GPU
simulation in which only rank 0 moved data while the other ranks' shards were
reserved, committed and written through without ever being copied. They are
not comparable with real-TP results.

```bash
sudo target/release/certus-server --drive-count 4 --channels 32 \
    --memory-tier-size 4G --memory-tier-eviction-threshold 0.8 --format
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --tp 2 --mode scheduler-step --block-bytes 1048576
```

#### KV layout (`--pieces-per-block`)

`--pieces-per-block N` splits each block into N GPU tensors of `--block-bytes / N`
bytes, and the connector sends N regions per block. This is how vLLM ≥ 0.29 hands
KV to an offloading connector under its default layer-outermost layout (`LBHNC`):
one tensor per layer, so for Llama-3-8B a 2 MiB block arrives as 32 pieces of 64 KiB
in 32 different allocations, and certus-server copies each piece separately.
(vLLM 0.23–0.28 did the same for connectors that did not opt into its cross-layer
layout, which the Certus connector did not; see
`knowledge/inference_workload_mapping.md`, "Layout history".) The default is 32 for
the default and contention modes; 1 elsewhere. 1 is one contiguous region per block, as under vLLM 0.20/0.22 and under
the block-outermost `BLHNC` layout (`CertusConnector` with `CERTUS_KV_CACHE_LAYOUT=BLHNC`). It is a count
of pieces, not of model layers: the same 32-layer model gives 32 pieces under
`LBHNC` and 1 under `BLHNC`. `--block-bytes` must be divisible by N.

```bash
# Llama-3-8B under vLLM's default layout
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --block-bytes 2097152 --pieces-per-block 32
```

#### Cold-load shape (default mode)

By default the cold pass empties the memory tier, then loads the keys back as
`--job-blocks` jobs in the order they were stored, `--inflight` outstanding. vLLM
serving differs in three more ways; each flag below changes one of them. The pass is
reported as `Cold Load Lifecycle (lookup → touch → …)` (the plain pass) or
`Cold Load Lifecycle (shaped: tier=… store=… load=… inflight=…)`; the label contains
`Cold Load Lifecycle`, so tools that parse that label find it, and a non-default shape is
written to the YAML `params`. The store and warm passes are unchanged by these flags.

With the tier empty (the default) and jobs in flight, the cold pass reads about 24 GB/s
(64 x 4, 2026-11-05), slightly above `spdk_nvme_perf`'s 23.1 GB/s for these 4 drives.
With `--cold-tier-state full` (reads evict as they land) 23.8 GB/s; under
`--preset longdocqa` (tier full, interleaved stores, one job per request) about 20.4–21 GB/s.
Every cold pass (plain and shaped) prints `Cold pass SSD reads: X MB for Y MB loaded` and
records `disk_read_bytes`; the counters need a certus-server built with
`--features rw-telemetry` (otherwise X is 0). On such a server (2026-11-06, latest
unstable) every byte loaded was read from the drives: default 24.35 GB/s, 125,829 MB read
for 125,829 MB loaded; `--inflight 1` 14.58 GB/s; `--preset longdocqa` 20.85 GB/s, both
also equal. So the 24 GB/s is drive reads, and the 23.1 GB/s `spdk_nvme_perf` figure is
below what these drives deliver at this queue depth, not a ceiling.

- **`--cold-tier-state full` with `--tier-fill-blocks N`: a full memory tier.** In
  production a cold load happens because the tier is full, so serving it means
  evicting other data. Before every cold pass the benchmark empties the tier, then
  stores N other blocks to bring it back to its eviction threshold, outside the
  timed region (`tier_fill`); the cold reads then evict as they land. N defaults to
  `--tier-gib` × `--eviction-threshold` ÷ block bytes (12 GiB × 0.8 ÷ 2 MiB = 4915);
  the benchmark cannot read the tier size, so pass `--tier-gib` if the server's
  differs. Default `empty`: the tier is cleared and not refilled.
- **`--store-order interleaved`: stores from concurrent requests.** vLLM stores per
  request per scheduler step, so blocks from several requests are committed in
  turn. The keys are split into `--requests` requests, and request r's i-th block
  is stored at step i, one block per store call. Default `batch`: `--job-blocks`
  keys per store job, in key order.
- **`--load-unit request`: one load job per request.** vLLM loads a request's whole
  cached prefix as one job (`prepare_load` of its hit blocks), not in the batches it
  was stored in. Each job is one request's prefix (`--num-blocks / --requests`
  blocks); requests are loaded in random order (`--seed`), and a prefix lookup stops
  at its first miss, as vLLM's does. Default `batch`: `--job-blocks`-key jobs in key
  order.

Cold jobs in flight are `--inflight`, and batch jobs `--job-blocks`, as for the store and
warm passes (the separate `--cold-inflight` flag is gone, and `--bs` no longer sizes them).

`--requests R` (default 16) is the request count for `interleaved` and `request`;
`--num-blocks` must divide evenly by it.

`--preset longdocqa` sets the shape LongDocQA's query round showed on vLLM 0.30 with
Llama-3-8B under the default `LBHNC` layout (`knowledge/serving/observations.yaml`,
`ldq-llama-3-8b-lbhnc-2026-10-26`): 2 MiB blocks in 32 pieces, 4 requests × 625
blocks (one 10,000-token document each), interleaved stores, one load job per
request, 4 in flight (`inflight: 4`, which sets `--inflight` for all three passes), full
tier. Flags given explicitly override the preset's values (e.g. `--inflight 2`, or
`--pieces-per-block 1` for the `BLHNC` layout). `knowledge/serving/check.py` fails if
the preset's block size, pieces per block or blocks per load job drift from that
observation.

```bash
# vLLM-like cold load on a 4 GiB tier (eviction threshold 0.8)
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py --tier-gib 4 \
    --store-order interleaved --load-unit request --requests 8 --cold-tier-state full

# LongDocQA's shape; start certus-server with --memory-tier-size 12G
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --preset longdocqa --working-set 2500
```

#### In-flight passes (vLLM 0.30's step loop)

vLLM 0.30's offloading worker (`offloading/worker.py`) submits each step's new loads and
the stores deferred from the previous step without waiting, and reaps whatever finished
with one non-blocking `get_finished()` per step, so several requests' jobs are
outstanding at once. The default mode's store and warm passes and the contention
mode drive the connector that way: up to `--inflight` jobs of `--job-blocks` keys
outstanding, each tick submitting into free slots, reaping once and idling to the next
`--step-us` boundary. Stores are prepared (`prepare_store`, then `touch`, as vLLM's
`_build_store_jobs`) in one tick and submitted in the next (`prepare_store_kv` defers
them). The modes `warm-inflight`, `store-inflight` and `mixed-inflight` that ran these
passes separately are gone: they are now the default mode's passes and `--mode contention`.

Job size and jobs in flight are independent knobs because both vary by workload: a
vLLM load job is one request's prefix hit, a store job one request's chunk per step.

**Defaults.** With no shape flags the default and contention modes use the values below;
an explicit flag (and a `--preset` value) always wins, and the other modes keep their own
defaults. They come from the
2026-11-05 sweeps on latest unstable (Llama-3-8B, 2 MiB blocks, 12 GiB tier, 4 NVMe
drives).

| flag | default | why |
|---|---|---|
| `--job-blocks` | 64 | from 16 to 128 blocks the plateau at 2+ jobs in flight is the same (warm 22.7 GB/s, 93% of the 24.4 GB/s host-to-device ceiling); 64 keeps the per-job host cost small |
| `--inflight` | 4 | store and cold load reach their plateau only at 4 (store 14.6 vs 13.1 GB/s at 2); LongDocQA's client runs 4 requests (about 2 measured) |
| `--pieces-per-block` | 32 | what vLLM 0.30 gives Llama-3-8B (LBHNC); the 3fs, lmcache and cputier benchmarks use 32 layers per block too. At 4 in flight 1 and 32 pieces measure the same; at 1 in flight a 1-piece store is 19% faster |
| `--num-blocks` | 90% of what the tier holds, at most 4000 | 4,000 blocks (7.8 GiB at 2 MiB) on a 12 GiB tier, 1,474 on 4 GiB, 4,000 on 32 GiB (see below) |
| `--store-job-blocks`, `--store-inflight` | contention: 32, 1; default mode's store pass: `--job-blocks`, `--inflight` | loads are the larger stream in serving (multi-turn moved 3.3x more bytes loaded than stored) |
| `--step-us` | 100 | removes the GIL stalls (job p99 6.3 ms vs up to 27 ms) |
| `--workers` | 4 | production's pool size |
| `--tier-gib`, `--eviction-threshold` | 12, 0.8 | what the targets start the server with |

LongDocQA's own job is 625 blocks, so pass `--job-blocks 625` to match it.

`--job-blocks 16 --inflight 1` reproduces the sequential passes the default mode ran before
(same-session numbers under "Default mode").

`--step-us N` (default 100) is the in-flight passes' scheduler tick, and how long the other modes wait between polls for finished transfers (they slept 100 µs there before the flag existed). `--step-us 0` is a
tight loop that releases the GIL each tick (`time.sleep(0)`), so the pool threads, which
run the connector's ring I/O in Python, are not starved as they would be by a loop that
keeps the GIL (`os.sched_yield()` does); but it still leaves some jobs waiting several
GIL switch intervals (warm 64 × 1, 10 runs each: job p99 6.4–27.1 ms and 14 % spread at
0, 6.2–6.4 ms and 8 % at 100, same median), so 100 is the default. Each report's phase
`get_finished` is the per-job time from submit to reap (what the sequential passes' `get_finished`
measured, the wait for one job), `reap_call` the one `get_finished()` call per tick; it also
prints the connector's `transfer_time`
(`*_transfer`, what vLLM logs as `kv_offload_load_time` / `kv_offload_store_time`),
the mean number of jobs outstanding, and the measured concurrency (summed
`transfer_time` over wall time, the formula behind LongDocQA's 2.91 in
`knowledge/serving/observations.yaml`). `--verify` stamps every stored block and
compares every loaded block byte for byte (its timings include that work; TP=1 only).
The server needs `min(--workers, jobs in flight) + 1` channels.

`--shape longdocqa` (default and contention modes) sets LongDocQA's job shape (625-block
loads × 4, 105-block stores × 4, 2 MiB blocks in 32 pieces); explicit flags override it. Shapes are for
checking the benchmark against a measured workload, not for a target's primary
metric. `knowledge/serving/check.py` fails if a shape disagrees with the
observation it names.

```bash
# 12 GiB tier (threshold 0.8): 4,000 blocks of 2 MiB fit (the defaults, spelled out)
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py \
    --job-blocks 64 --inflight 4 --num-blocks 4000 \
    --block-bytes 2097152 --pieces-per-block 32 --workers 4
# the old sequential shape
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py --job-blocks 16 --inflight 1
```

```
--job-blocks N         (default, contention) blocks per job (default: 64)
--inflight N           (default, contention) jobs outstanding: store, warm, cold (shaped pass when > 1), contention loads (default: 4)
--store-job-blocks N   blocks per store job (contention: 32; default mode: --job-blocks)
--store-inflight N     store jobs outstanding (contention: 1; default mode: --inflight)
--step-us US           the in-flight passes' tick and every other mode's poll interval; 0 = tight loop (default: 100)
--shape NAME           (default, contention) validation shape: longdocqa
--verify               (default, contention) byte-compare every loaded block
```

#### Per-mode options

```
--hit-ratio F          (prefix-miss) Fraction of keys pre-stored, 0.0-1.0 (default: 0.75)
--requests-per-step N  (scheduler-step) Requests per scheduler step (default: 8)
--num-sessions N       (scheduler-step) Distinct conversations sharing the prefix pool (default: 32)
--prefix-sharing MODE  (scheduler-step) shared (default), or private: no prefix block shared between sessions
```

### bench_connector_path.py

Measures the connector's 3-phase store and check→pin→lookup load paths at the `Ring` level with connector overhead (reserve → copy_to_store → commit_store for stores; check → pin → lookup for loads).

```bash
python benchmarks/certus-connector-bench/bench_connector_path.py \
    --shm-path /dev/shm/certus-shmq --bs 16 --num-blocks 128
```

### bench_connector_race.py

Reproduces and validates the check→pin→lookup eviction race. Two threads run concurrently: a storer that fills the memory tier forcing eviction, and a loader that uses `check_and_pin` to atomically verify+protect keys. Validates that pinned keys see zero load failures.

```bash
python benchmarks/certus-connector-bench/bench_connector_race.py \
    --shm-path /dev/shm/certus-shmq --duration 30
```

## Results

Output goes to `bench-results/` in the repo root. The lifecycle benchmark writes `connector_lifecycle.csv` with per-phase throughput, IOPS, and latency percentiles (p50/p99/mean) for every sub-operation. The `mode` column identifies which benchmark mode produced each row.

### Memory tier size

The numbers depend on the server's memory tier. A warm load is only warm while its keys stay in
the tier; a working set larger than `tier x eviction threshold` is evicted to the drives and
promoted back, and nothing in the pass's own output shows it: with 4,000 blocks (7.8 GiB) on a
4 GiB tier, the warm in-flight pass ran, reported 24.45 GB/s and "0 blocks not loaded" (2026-11-05).

The benchmark cannot read the tier size from the server, so you state it: `--tier-gib` and
`--eviction-threshold` default to 12 and 0.8, what the targets start the server with. Pass
`--tier-gib` whenever the server's `--memory-tier-size` differs. Then:

- The default and contention modes refuse a `--num-blocks` that does not fit (the warm pass
  and contention's loads need it resident), and one that makes fewer jobs than `--inflight`,
  at argument validation, before connecting, naming the largest (smallest) that works.
- A small tier with a large `--num-blocks` is still possible, two ways. `--no-warm` skips the
  warm pass: store and cold do not need the set resident, so they run at any size.
  `--allow-warm-overflow` runs the warm pass (and contention's loads) anyway; its loads then
  come partly from the drives, so the result is marked, not hidden: its label ends
  `OVERFLOW: set exceeds tier, not warm`, its CSV row has `warm_overflow=1` and its SSD reads
  (`disk_read_bytes`, with an rw-telemetry server), it does not fail on lookup misses (it
  counts them), and `connector-sweep/make_report.py` flags the row. The same two flags exist in
  the cputier, LMCache and 3FS benches (3FS has no tier, so it never overflows).
- Without `--num-blocks` they use 90% of `tier x threshold / block bytes`, at most 4,000.
- The cold pass takes `--tier-fill-blocks` from the same numbers unless you give it.
- Every default and contention run prints `Tier assumed: ...` first, so a wrong `--tier-gib`
  is visible in the results; it is not detected.

| server tier | usable (x 0.8) | default `--num-blocks` (2 MiB) | cold tier fill | measured (warm / store / contention load+store / cold) |
|---|---|---|---|---|
| 4 GiB | 3.2 GiB, 1,638 blocks | 1,474 (2.9 GiB) | 1,638 blocks | 23.3 / 14.3 / 16.4+5.9 / 21.4 GB/s |
| 8 GiB | 6.4 GiB, 3,276 blocks | 2,948 (5.8 GiB) | 3,276 blocks | not measured |
| 12 GiB | 9.6 GiB, 4,915 blocks | 4,000 (7.8 GiB) | 4,915 blocks | 23.3 / 14.6 / 14.5+6.0 / 21.0 GB/s |
| 32 GiB | 25.6 GiB, 13,107 blocks | 4,000 (7.8 GiB) | 13,107 blocks | 23.3 / 14.6 / 13.2+6.5 / 22.6 GB/s |

**Running with another tier size, e.g. 8 GiB.** Give the size once to each side; everything
else (working set, cold fill) follows from it:

```bash
# certus: the server's tier, and the same number to the benchmark
sudo target/release/certus-server --drive-count 4 --channels 32 \
    --memory-tier-size 8G --memory-tier-eviction-threshold 0.8 --format
python benchmarks/certus-connector-bench/bench_connector_lifecycle.py --tier-gib 8
# the siblings: their tier is their own flag (3fs has no memory tier)
CPU_BYTES=8G benchmarks/cputier-connector-bench/run-in-container.sh --cpu-bytes 8G
L1_GB=8 benchmarks/lmcache-connector-bench/run-in-container.sh
# all four at once: run_compare.sh sets every one of the above from --dram
benchmarks/connector-sweep/run_compare.sh --out bench-results/compare-8g.html --dram 8G
```

cputier and LMCache derive their default `--num-blocks` from their own tier by the same rule
(cputier from `--cpu-bytes`, LMCache from the L1 its server reports), so at 8 GiB all three move
2,948 blocks; 3FS, which has no tier, takes the 12 GiB figure (4,000). `run_compare.sh` passes
one `--num-blocks` to every arm (default 1,024), so its arms always match.

The defaults carry over to bigger and smaller tiers. The cost of a bigger tier is the cold pass:
each pass with `--cold-tier-state full` empties the tier and refills it (`--tier-fill-blocks`), so cold
at 32 GiB rewrites 25.6 GiB around every pass: one whole cold run (the store and warm passes included)
took 166 s against 84 s at 12 GiB. A 32 GiB tier also needs that much
pinned hugepage memory on the GPU's NUMA node. Larger blocks shrink every block count here in
proportion (8 MiB blocks: a quarter as many). Reading the tier size and the tier's promotion and
eviction counters from the server would make the check exact; there is no such call yet.
