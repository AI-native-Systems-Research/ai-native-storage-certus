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

## Using these chunks

These chunks are **not** wired as a named workload. The `sharegpt` workload
(`WORKLOAD_NAME=sharegpt` in `run_multiturn_common.py`) is defined by human-turn
count — `SHAREGPT_MIN_TURNS`/`SHAREGPT_MAX_TURNS`, default `12/12` — and only the
12-turn subset (`data/sharegpt_12turn_450.json`) is pre-prepared; any other turn
count expects an explicit `DATASET_PATH`.

A chunk here is a plain ShareGPT-format array, so to replay one, point a driver
at it directly:

```bash
DATASET_PATH=data/sharegpt/003.json NUM_CONVS=2000 WORKLOAD_MODE=async \
  python benchmarks/kv-offload-replay/run_multiturn_offloading.py
```

`DATASET_PATH` always wins over any `WORKLOAD_NAME`; `NUM_CONVS` caps how many of
the file's conversations are loaded. These chunks are the raw corpus you would
turn-filter (e.g. to build a new pre-prepared subset), not something the drivers
select by name.

## Regenerating

```bash
python3 benchmarks/kv-offload-replay/tools/chunk_sharegpt_sizes.py \
  --chunk-size 10000 --outdir data/sharegpt --prefix ""
```

Run without `--outdir` to only print per-file size stats. See the script's
`--help` for other options (`--ensure-ascii`, `--indent`, `--chunk-size`).
