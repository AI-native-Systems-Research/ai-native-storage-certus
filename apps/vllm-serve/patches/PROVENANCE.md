# apps/vllm-serve/patches — shmq scheduler fix#3 (provenance)

`run-serve-certus-shmq.sh` bind-mounts `scheduler.fix3.py` over the shmq image's
in-container vLLM scheduler (env `VLLM_FIX3=1`, the default; set `VLLM_FIX3=0`
to run stock). This documents what that file is and how to regenerate it.

## The bug

The stock `vllm/vllm-openai:v0.26.0` `OffloadingConnector` scheduler crashes the
engine under sustained load:

```
vllm/distributed/kv_transfer/kv_connector/v1/offloading/scheduler.py  _build_store_jobs()
    assert len(offload_keys) == len(offload_block_ids)   # AssertionError -> EngineDeadError
```

`offload_keys` advances every step (`update_offload_keys` is unconditional) but
`block_ids` only grows when the scheduler reports newly allocated blocks. On the
finished path `num_offloadable_tokens` jumps to `req.num_tokens`, so `num_chunks`
can cross a chunk boundary whose `blocks_per_chunk` GPU blocks were never
appended to `block_ids` (a finishing decode reused an already-allocated block).
The strided `block_ids` slice then comes up short and the `assert` kills the
engine. Load-dependent; observed under a guidellm sweep against
`run-serve-certus-shmq.sh`. This is engine-side plumbing in the *stock*
connector, so it is backend-agnostic — the same assert hits both the shmq
(`CertusShmqOffloadingSpec`) and cputier (`TieringOffloadingSpec`) paths.

## The fix (fix#3)

Clamp `num_chunks` to the chunks that have both a key and backing GPU blocks
before the slice/assert:

```python
num_chunks = min(num_chunks,
                 len(group_state.offload_keys),
                 len(group_state.block_ids) // blocks_per_chunk)
```

Correctness-preserving: an unbacked trailing chunk on a finishing request (no
future step to flush it) is simply not stored, costing at most a future
prefix-cache hit.

## Why this copy is shmq-only

The canonical fix#3 lives in
`benchmarks/kv-offload-replay/vllm-fix2/scheduler.py` (fork
`dwaddington/vllm` branch `fix/tiering-deferred-finalize-v0.26.0`, commit
`83571bcb`). That file also carries **fix#2** — three
`self.manager.mark_stores_submitted(...)` calls (a deferred-finalize handshake).
That handshake requires `TieringOffloadingManager.mark_stores_submitted()`;
`CertusShmqOffloadingSpec`'s manager does **not** implement it, so bind-mounting
the full vendored file onto the shmq image would `AttributeError`.

`scheduler.fix3.py` is therefore the **shmq image's own** `scheduler.py` with
**only** the fix#3 clamp added — no fix#2 handshake. The shmq crash is the fix#3
assert, not the fix#2 `_req_state` KeyError, so fix#2 is not needed here.

Valid for vLLM 0.26.0 only. Do not apply to the 0.23.0 sharedstorage/shmq images.

## Regenerating

```bash
STORE=(--root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run)
TGT=/usr/local/lib/python3.12/dist-packages/vllm/distributed/kv_transfer/kv_connector/v1/offloading/scheduler.py
# 1. re-extract the pristine baseline from the current shmq image
podman "${STORE[@]}" run --rm --pull=never --entrypoint cat \
  localhost/certus-otel-shmq-bench "$TGT" > scheduler.orig.py
# 2. re-apply the fix#3 clamp block after the `num_chunks = req_status.storable_chunks(...)`
#    call in _build_store_jobs (see the diff: scheduler.orig.py -> scheduler.fix3.py).
```

`scheduler.orig.py` is kept as the un-patched baseline so `diff scheduler.orig.py
scheduler.fix3.py` shows exactly the one clamp block that was added.
