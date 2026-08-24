# ShareGPT dataset — 10,000-conversation chunks

The full ShareGPT Vicuna dataset split into files of **10,000 conversations
each, in the order they appear in the source**. The last file is a partial
(4,145 conversations). Each file is a standalone ShareGPT-format JSON array
(same schema as the source: a list of `{"id", "conversations": [...]}`), so any
driver can consume one directly via `DATASET_PATH`.

## Source

`ShareGPT_V3_unfiltered_cleaned_split_no_imsorry.json` from the HuggingFace
dataset [`anon8231489123/ShareGPT_Vicuna_unfiltered`][hf] — the final processed
stage of that repo (HTML-cleaned, long conversations split, refusal turns
removed). **94,145 conversations** total, ~640 MiB.

Note: the HF repo lists ~4.35 GB, but that is the sum of six redundant copies of
the corpus at different processing stages (raw HTML, HTML-cleaned, V3 split, and
this no-imsorry variant) plus a build wheel. Only this one file is used here.

[hf]: https://huggingface.co/datasets/anon8231489123/ShareGPT_Vicuna_unfiltered

## Files

Conversations are contiguous and non-overlapping; concatenating the files in
order reproduces the source exactly.

| File | Conversations (source index) | Count | Size |
|------|------------------------------|------:|-----:|
| `000.json` | `[0:10000]`      | 10,000 | 65.31 MiB |
| `001.json` | `[10000:20000]`  | 10,000 | 63.99 MiB |
| `002.json` | `[20000:30000]`  | 10,000 | 65.74 MiB |
| `003.json` | `[30000:40000]`  | 10,000 | 65.01 MiB |
| `004.json` | `[40000:50000]`  | 10,000 | 64.43 MiB |
| `005.json` | `[50000:60000]`  | 10,000 | 65.35 MiB |
| `006.json` | `[60000:70000]`  | 10,000 | 64.79 MiB |
| `007.json` | `[70000:80000]`  | 10,000 | 65.34 MiB |
| `008.json` | `[80000:90000]`  | 10,000 | 65.12 MiB |
| `009.json` | `[90000:94145]`  |  4,145 | 27.36 MiB |
| **Total** |                  | **94,145** | **612.43 MiB** |

Sizes are compact UTF-8 (`separators=(",", ":")`, `ensure_ascii=False`).

## Running it as a workload

These chunks are registered as the `sharegpt` workload in
`benchmarks/kv-offload-replay/run_multiturn_common.py`, so any of the five
multi-turn drivers can replay one with `WORKLOAD_NAME=sharegpt`. It is meant for
`WORKLOAD_MODE=async`, where each of the ~10,000 conversations runs as its own
coroutine (vLLM's `max_num_seqs` bounds the running batch; the rest queue):

```bash
# nooffload baseline, async, chunk 000 (all 10k convs as coroutines)
WORKLOAD_NAME=sharegpt WORKLOAD_MODE=async \
  python benchmarks/kv-offload-replay/run_multiturn_nooffload.py

# pick a different chunk (000..009) and cap the conversation count
WORKLOAD_NAME=sharegpt SHAREGPT_CHUNK=3 NUM_CONVS=2000 WORKLOAD_MODE=async \
  python benchmarks/kv-offload-replay/run_multiturn_offloading.py
```

`SHAREGPT_CHUNK` selects the chunk (default `000`); `NUM_CONVS` caps how many of
that chunk's conversations are loaded (default 10,000); `DATASET_PATH` still
overrides with an explicit path. The same knobs work for the shmq / fs_bench
drivers.

The selector env is `WORKLOAD_NAME`, **not** `WORKLOAD` — the bench container
images already use `WORKLOAD` for the driver-script path their entrypoint execs,
so the two must not collide. `SHAREGPT_DIR` overrides the chunk directory; the
container harness sets it to the read-only bind-mount of this directory, since
the `__file__`-relative default does not survive the image's flattened layout.

### Through the orchestrator

`profile_all.sh` forwards the same selection to every backend and bind-mounts
this directory into the container variants automatically:

```bash
benchmarks/kv-offload-replay/profile_all.sh \
  --workload sharegpt --sharegpt-chunk 3 --async --num-convs 2000
```

`--workload <name>` sets `WORKLOAD_NAME`, `--sharegpt-chunk <n>` sets
`SHAREGPT_CHUNK`; omit `--workload` for the default 450×12 dataset.

## Regenerating

```bash
python3 benchmarks/kv-offload-replay/tools/chunk_sharegpt_sizes.py \
  --chunk-size 10000 --outdir data/sharegpt --prefix ""
```

Run without `--outdir` to only print per-file size stats. See the script's
`--help` for other options (`--ensure-ascii`, `--indent`, `--chunk-size`).
