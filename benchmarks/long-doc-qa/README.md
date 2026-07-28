# long_doc_qa benchmark (containerized client)

A containerized runner for LMCache's [`long_doc_qa.py`][src] — a long-document QA
throughput/latency benchmark that exercises **KV-cache reuse** by repeating
generated documents and measuring the second ("query") round against a cold
warmup round.

[src]: https://github.com/LMCache/LMCache/blob/dev/benchmarks/long_doc_qa/long_doc_qa.py

## What it is

`long_doc_qa.py` is a **pure OpenAI-API client**. It:

1. generates `--num-documents` documents, each `--document-length` tokens of
   filler (`"hi hi hi …"` — no tokenizer needed),
2. sends them once (warmup, cold cache), then repeats them `--repeat-count`
   times (query round, warm cache),
3. reports TTFT and throughput for each round to `warmup_round.csv` /
   `query_round.csv` (and PNGs with `--visualize`).

It **connects to an already-running server** — it does not start one and needs
no GPU, model weights, or tokenizer. That makes this image reusable against
**any** OpenAI-compatible backend: Certus, LMCache, CPU-offload, or plain vLLM.

The script is vendored (`long_doc_qa.py`, Apache-2.0, upstream header intact),
pinned to LMCache `dev`. Re-copy from upstream to bump.

## Build

```bash
docker build -t long-doc-qa-bench benchmarks/long-doc-qa
# podman: podman build -t long-doc-qa-bench benchmarks/long-doc-qa
```

## Run

The **only mandatory** setting is the server target (`BASE_URL`, or `HOST`+`PORT`)
— there is no default. `MODEL` defaults to `auto` (resolved from the server's
`/models`).

```bash
# server listening on the host at :8000
docker run --rm --network host \
    -e BASE_URL=http://localhost:8000/v1 \
    -e NUM_DOCUMENTS=16 -e REPEAT_COUNT=4 -e JSON_OUTPUT=1 \
    -v "$PWD/results:/workspace/results" \
    long-doc-qa-bench
```

Without `--network host`, reach a host server via `host.docker.internal`
(`BASE_URL=http://host.docker.internal:8000/v1`; on Linux add
`--add-host=host.docker.internal:host-gateway`).

Pass benchmark flags directly to bypass the env layer entirely:

```bash
docker run --rm --network host long-doc-qa-bench \
    --base-url http://localhost:8000/v1 --num-documents 16 \
    --repeat-count 4 --document-length 20000 --json-output
```

## Environment knobs

| Env | Flag | Notes |
|---|---|---|
| `BASE_URL` | `--base-url` | **Required** (or `HOST`+`PORT`). e.g. `http://localhost:8000/v1` |
| `HOST`, `PORT` | `--host`,`--port` | Alternative to `BASE_URL` |
| `MODEL` | `--model` | Default `auto` (from `/models`) |
| `DOCUMENT_LENGTH` | `--document-length` | Tokens per doc (upstream default 20000) |
| `NUM_DOCUMENTS` | `--num-documents` | Docs to generate (default 8) |
| `OUTPUT_LEN` | `--output-len` | Tokens generated per prompt (default 100) |
| `REPEAT_COUNT` | `--repeat-count` | Repeats per prompt → cache reuse (default 2) |
| `REPEAT_MODE` | `--repeat-mode` | `random` / `tile` / `interleave` |
| `SHUFFLE_SEED` | `--shuffle-seed` | Seed for `random` mode |
| `MAX_INFLIGHT_REQUESTS` | `--max-inflight-requests` | Concurrency (default 2) |
| `SLEEP_TIME_AFTER_WARMUP` | `--sleep-time-after-warmup` | Seconds |
| `HIT_MISS_RATIO` | `--hit-miss-ratio` | e.g. `3:1` to force misses |
| `EOS_TOKEN_ID` | `--eos-token-id` | Bias against EOS to hit exact output len |
| `TRIM_FRACTION` | `--trim-fraction` | Trim outliers before averaging |
| `OUTPUT` | `--output` | Response dump file |
| `COMPLETIONS` | `--completions` | `1` to use the completions API |
| `VISUALIZE` | `--visualize` | `1` to also write `*_round.png` |
| `JSON_OUTPUT` | `--json-output` | `1` to print a JSON summary |
| `OPENAI_API_KEY` | — | Passed through; default `sk-dummy` |

Output files (`warmup_round.csv`, `query_round.csv`, optional PNGs) land in
`/workspace/results` — mount it to keep them.

## Notes

- **`--help` crashes** with `ValueError: incomplete format`. This is an upstream
  bug in `long_doc_qa.py` (the `--trim-fraction` help text contains bare `%`
  signs that argparse tries to interpret). It only affects `--help`; every real
  run is fine. Use the flag/env table above instead of `--help`. The vendored
  script is kept byte-for-byte identical to upstream, so this is intentionally
  not patched here.
