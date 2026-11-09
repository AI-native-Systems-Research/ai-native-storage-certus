# Serving knowledge: models, deployments, observations

Facts about the models and serving engines that call Certus, kept in one place so
targets, benchmarks and the connector cite them instead of re-deriving them.

| file | holds | does not hold |
|---|---|---|
| `models.yaml` | a model's own architecture, true in any serving engine, each value from its `config.json` at a pinned revision | dtype, TP, block size, layout — anything an engine or deployment chooses |
| `deployments.yaml` | one model served by one engine version with given settings (KV dtype, TP, block tokens), and the KV layout that engine gives it | measured performance |
| `observations.yaml` | values seen in one run of one workload on one deployment, each with its basis and evidence | general claims |

The layout *rules* (how an engine version turns layers into KV tensors) live in the engine's
interface contract, e.g. `knowledge/contracts/vllm-offloading-interface-v030.yaml`
(`kv_caches_layout_at_v030`). A deployment names the rule it relies on; it does not restate it.

## No stored derived values

Derived numbers are computed by `check.py`, not stored. A stored number can drift from the
facts it came from. `python3 knowledge/serving/check.py` prints every derived value and fails
on a broken reference or a stored value that disagrees with its inputs.

Formulas (bytes; `dtype_bytes` from the deployment):

```
kv_bytes_per_token_per_layer  standard attention (MHA/GQA/MQA):  2 (K,V) x num_kv_heads x head_dim x dtype_bytes
                              MLA (cached latent):              (kv_lora_rank + qk_rope_head_dim) x dtype_bytes
kv_bytes_per_token            sum over layers that carry KV (layer_kinds; mamba/linear layers carry none)
layer_bytes                   offload_block_tokens x kv_bytes_per_token_per_layer / tensor_parallel
block_bytes                   offload_block_tokens x kv_bytes_per_token / tensor_parallel
piece_bytes                   block_bytes / pieces_per_block  (= layer_bytes for a layer-outermost layout)
```

`pieces_per_block` (KV tensors one offloaded block is spread over) depends on the engine's
resolved KV cache layout, not only on the layer count: layers that alias the same bytes share a
tensor and a packed layout gives one. A deployment records it as observed, or marks it
`assumed` with the rule it follows.

## Bases

Every value in `observations.yaml` says how it was obtained:

- `measured` — read from a log or metric of the run; quote the line in `evidence`.
- `configured` — a setting of the run (client flag, server arg).
- `computed` — worked out from other values; give the formula.

Report measured next to configured where they can differ (e.g. jobs in flight).

## Adding entries

- **Model.** Fetch `config.json` at a revision, record its sha, copy the fields; leave out
  anything the config does not give. Mark `head_dim` `derived` when the config omits it.
- **Deployment.** Name the model, engine and version, the KV dtype, TP and both block token
  counts. Name the layout rule from the engine's contract and give `pieces_per_block` with its
  basis (`observed` with an observation id, or `assumed`).
- **Observation.** Name the deployment and workload, the date, the reproduction (script and
  settings) and quote the evidence lines. Do not commit large logs.

Then run `check.py`.
