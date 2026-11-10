# synthetic-agentic KV-offload benchmark (inference-perf, server mode)

Runs the [inference-perf](https://github.com/lenadankin/inference-perf/tree/synthetic_data_enabled)
`synthetic_agentic` workload — procedural agentic traffic (tool loops, recursive
sub-agent fan-out, mid-session context compaction, a shared system-prompt prefix)
— against the KV-offload backends and reports throughput side by side.

## Why this is separate from the in-process drivers

The existing KV-offload comparison (`../kv-offload-replay/`) embeds vLLM's
`AsyncLLM` in-process and replays a flat ShareGPT turn stream. `synthetic_agentic`
is a **ReplayGraph DAG**, not a flat turn list, and inference-perf that produces
it is a **pure HTTP load generator** — it only talks to an OpenAI endpoint
(`server.base_url`); there is no in-process/Python-API mode. So this workload runs
in **server mode**: a `vllm serve` process per backend, driven over HTTP.

## The two axes are independent

- **Workload = client.** inference-perf running `synthetic_agentic` against a
  `base_url`. It does not know which KV connector the server uses. → this dir.
- **Backend = server.** One vLLM server; the KV backend is just a
  `--kv-transfer-config` connector *parameter* (`none` / `cpu` / `tiered` /
  `certus`) — not a separate server. → `serve_vllm.sh`.

They meet only at the endpoint. The **fixed `seed`** in
`configs/synthetic_agentic.yaml` makes the workload deterministic (all randomness
derives from `(seed, session_index)`), so every backend replays a byte-identical
request stream — the "generate once, replay for fair comparison, save resources"
property, with no materialised trace file required.

## Pieces

| File | Role |
|---|---|
| `serve_vllm.sh` | Serve one vLLM server; connector is a `--kv-transfer-config` parameter (4 arms). Workload-agnostic. |
| `Dockerfile.inference-perf` | The inference-perf client image (HTTP load generator). Backend-agnostic. |
| `configs/synthetic_agentic.yaml` | The workload definition (template; `BASE_URL`/`MODEL` injected at run). |
| `docker-entrypoint-inference-perf.sh` | Client entrypoint: `run` (drive load) or `generate` (dump audit graph). |
| `generate_trace.sh` | Optional: dump deterministic replay graphs for inspection/audit. |

## Run it

The single entry point is the existing orchestrator, with the new workload value:

```bash
# 1. (optional) inspect what will be replayed
./generate_trace.sh

# 2. run all four backends over the same workload
../kv-offload-replay/profile_all.sh --workload synthetic-agentic \
    --logdir /mnt/certus1/agentic-$(date +%s)
```

`profile_all.sh --workload synthetic-agentic` launches each selected backend as a
vLLM server (via `serve_vllm.sh`), drives it with the inference-perf client, scrapes
`/metrics`, and records a row in the usual `results.json` schema (rendered by the
`profile-kvoffload-replay-all` skill). `--only` / `--skip` subset the backends.

### Standalone (debugging one backend)

```bash
# server: pick a connector
CONNECTOR=none DETACH=1 ./serve_vllm.sh up
# client: drive it
podman run --rm --network host \
    -e BASE_URL=http://localhost:8000 -e MODEL=NousResearch/Meta-Llama-3-8B \
    -v $HOME/.cache/huggingface:/root/.cache/huggingface:z \
    -v $PWD/results:/results:z \
    synthetic-agentic-client:latest run
./serve_vllm.sh stop
```

## Preconditions

- GPU free; HF cache for the model + tokenizer (mounted, offline by default).
- `certus` connector needs the host `certus-server` shmq endpoint already running
  (`serve_vllm.sh` does not start it) and `--ipc=host`.
- `tiered` needs the offload image (see `../kv-offload-replay/Dockerfile.offload`),
  which now bakes the tiering fix **by default** so tiering survives at scale, and
  a writable `FS_ROOT_DIR`. (Opt out with `--build-arg VLLM_FIX_TIERING=0` only to
  reproduce the stock upstream crash.)
- Llama-3-8B: `MAX_MODEL_LEN` > 8192 is applied with RoPE linear x4 so agentic
  sessions + compaction (trigger 8500) fit; large-context models skip scaling.

## Artifacts

Graphs, reports, logs, and `results.json` are **bench artifacts** — written under
the logdir / `results/`, never committed.
