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

## Using these chunks as a workload

The `sharegpt` workload (`WORKLOAD_NAME=sharegpt` in `run_multiturn_common.py`)
is selected by human-turn count — `SHAREGPT_MIN_TURNS`/`SHAREGPT_MAX_TURNS`.
Exactly two configs are prepared, and only these two are accepted:

- **`min-turns 12` (`12/12`)** → `data/sharegpt_12turn_450.json` (the 450-conv,
  12-turn subset every bench image bakes as its `DATASET_PATH`).
- **`min-turns 1` (`1/1`)** → *this whole directory* — the full 94,145-conversation
  corpus. `load_convs` reads every `*.json` chunk here in sorted order and
  concatenates them into one conversation stream, capped by `NUM_CONVS`.

`max-turns` defaults to `min-turns`, so `--min-turns 1` and `--min-turns 12`
alone each select a prepared config. Any other pair — in particular a `max-turns`
that isn't `1` (with min 1) or `12` (with min 12) — is rejected; point
`DATASET_PATH` at a custom turn-filtered file for anything else. Through
`profile_all.sh`/`run-bench.sh`, supplying `--min-turns`/`--max-turns` implies
`--workload sharegpt`, so you don't have to pass both.

```bash
# full corpus, first 2000 conversations, async
WORKLOAD_NAME=sharegpt SHAREGPT_MIN_TURNS=1 NUM_CONVS=2000 WORKLOAD_MODE=async \
  python benchmarks/kv-offload-replay/run_multiturn_offloading.py

# through the orchestrator (mounts this dir into the container variants;
# --min-turns implies --workload sharegpt)
benchmarks/kv-offload-replay/profile_all.sh --min-turns 1 --num-convs 2000
```

`min-turns 1` means "draw from the whole corpus"; `load_convs` still keeps only
conversations with ≥ 2 human turns (a single-turn conversation has nothing to
replay across rounds). Because these chunks are **not** baked into the images,
the orchestrator bind-mounts this directory read-only and points `DATASET_PATH`
at the mount. You can also point a driver at a single chunk directly with
`DATASET_PATH=data/sharegpt/003.json`.

> The container variants bake the driver code, so the full-corpus path only works
> in images built with directory-aware `load_convs` — rebuild the bench images
> (`profile_all.sh --rebuild`) if `--min-turns 1` was added after they were built.

## Regenerating

```bash
python3 benchmarks/kv-offload-replay/tools/chunk_sharegpt_sizes.py \
  --chunk-size 10000 --outdir data/sharegpt --prefix ""
```

Run without `--outdir` to only print per-file size stats. See the script's
`--help` for other options (`--ensure-ascii`, `--indent`, `--chunk-size`).
