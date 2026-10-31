# certus-shmq-connector

vLLM KV-cache offload to certus-server over a shared-memory ring. Two classes plug into vLLM:

- `CertusShmqOffloadingSpec` (`certus_shmq_connector.spec`): the offloading spec. It stores and
  loads KV blocks in Certus.
- `CertusConnector` (`certus_shmq_connector.connector`): vLLM's
  `OffloadingConnector` with one change, the KV cache layout it asks for (below).

## Use

certus-server must be running and own the shared-memory path. Then:

```bash
vllm serve <model> --kv-transfer-config '{
    "kv_connector": "CertusConnector",
    "kv_connector_module_path": "certus_shmq_connector.connector",
    "kv_role": "kv_both",
    "kv_connector_extra_config": {
        "spec_name": "CertusShmqOffloadingSpec",
        "spec_module_path": "certus_shmq_connector.spec",
        "shm_path": "/dev/shm/certus-shmq",
        "slab_size_bytes": 2097152
    }
}'
```

`slab_size_bytes` must be at least the KV bytes per block (2 MiB for Llama-3-8B at 16 tokens).
Run vLLM with `--ipc=host` in a container, so it sees the host's `/dev/shm`.

## Offloaded block size

By default each Certus key holds one GPU block (16 tokens). `"block_size": 64` (or
`"blocks_per_chunk": 4`) in `kv_connector_extra_config` makes each key hold 64 tokens: vLLM's
4 GPU blocks for it are stored side by side in one slot, and the slot size scales with them.
A request then needs 4x fewer keys, so 4x fewer lookups and slot reservations on vLLM's
scheduler thread. On node4 (Qwen2.5-7B blocks, 477 MB per load, memory tier full), 64-token
keys loaded at 21.6 GB/s against 16.9 GB/s, and cut the scheduler-thread connector time per
load from 21 ms to 4.4 ms.

vLLM can start a load inside a key (its first blocks are already on the GPU). The connector
still reads the whole slot, and sends the blocks vLLM did not ask for to a small scratch GPU
buffer. Needs vLLM 0.26 or newer; on older versions only the default is accepted.

## KV cache layout

`CertusConnector` asks vLLM for the same layout as vLLM's own `OffloadingConnector`: `LBHNC` on
vLLM 0.29 and later, which splits each block into one piece per layer (32 for Llama-3-8B). Certus
copies a block's pieces as batched GPU copies, so the split costs little: on vLLM 0.30,
Llama-3-8B, FlashAttention, LongDocQA's query round took 3.06 s with `LBHNC` and 3.27 s with
`BLHNC` (one contiguous piece per block; medians of 3 runs each). `NixlConnector` also asks for
`LBHNC`, so the default works beside it in a `MultiConnector`, which refuses connectors that ask
for different layouts ("KV cache layout mismatch").

- **Opting into another layout.** `CERTUS_KV_CACHE_LAYOUT=<name>`, e.g. `BLHNC`, makes the class
  request that layout instead (`CERTUS_KV_CACHE_LAYOUT=` empty: the stock request). Not beside
  NIXL: `MultiConnector[NixlConnector, CertusConnector]` with `BLHNC` stops the engine at start.
- **A request, not a setting.** vLLM uses a requested layout only when every attention backend in
  the model supports it. Otherwise it logs a warning and keeps its default; the connector works as
  before. FlashAttention and FlashInfer accept `BLHNC`. CPU, ROCm, FlexAttention, TurboQuant and
  some MLA paths do not. See `vllm/v1/attention/backends/*` `supported_kv_cache_layouts()`.
- **vLLM 0.29 or later** for `BLHNC`: earlier versions used other layout names and had no
  block-outermost layout.
- **`VLLM_KV_CACHE_LAYOUT`** (vLLM's) takes precedence over the request.

The connector log shows the result: `KV cache layout (vLLM 0.30+): LBHNC` and `KV 32 region(s)`.

## Tests

```bash
PYTHONPATH=certus-shmq-connector:apps/python python3 -m pytest certus-shmq-connector/tests -q
```
