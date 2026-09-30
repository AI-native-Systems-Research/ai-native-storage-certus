# apps/vllm-serve/patches — shmq scheduler fix#3 (provenance)

`run-serve-certus-shmq.sh` applies fix#3 to the shmq image's in-container vLLM
scheduler (env `VLLM_FIX3=1`, the default; set `VLLM_FIX3=0` to run stock).

Since the move to version-tagged images (`certus-shmq-connector:vllmX.Y.Z`), the fix
is applied by the **in-place patcher `apply_fix3.py`**, NOT by bind-mounting a
whole `scheduler.py`. The old whole-file mount was pinned to vLLM 0.26.0 and
would drop a stale scheduler onto a drifted API on any other base image. The
patcher instead:

- locates the installed scheduler via `importlib`
  (`vllm.distributed.kv_transfer.kv_connector.v1.offloading.scheduler.__file__`),
  so there is no hardcoded `pythonX.Y` site-packages path;
- inserts **only** the fix#3 clamp, anchored on a block that is byte-stable
  across the versions it supports;
- is **idempotent** (a `certus-fix3` marker makes re-runs a no-op) and
  **non-fatal by contract**: on any anchor mismatch or compile failure it leaves
  the file untouched, prints a WARNING, and the server runs the stock scheduler.

`run-serve-certus-shmq.sh` bind-mounts `apply_fix3.py` into the container at
`/opt/certus/apply_fix3.py` and runs it once, right before `vllm serve`.

## The bug

The stock `OffloadingConnector` scheduler crashes the engine under sustained
load:

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
prefix-cache hit. The clamp is **identity except in the exact crash case** —
`min(...)` returns `num_chunks` unchanged whenever `block_ids` already backs all
chunks — so it cannot regress any currently-working scenario.

## The anchor and the versions it covers

`apply_fix3.py` inserts the clamp immediately **before** this block (16-space
indent), verified UNIQUE (`count == 1`) in each version it patches:

```python
                start_chunk_idx = group_state.next_stored_chunk_idx
                if num_chunks <= start_chunk_idx:
                    continue
                offload_keys = group_state.offload_keys[start_chunk_idx:num_chunks]
```

`storable_chunks(...)` (just above) is deliberately NOT part of the anchor: its
signature drifted (2 args in 0.26, 3 in 0.27+), so we anchor on the lines below
it that did not change. `blocks_per_chunk`, `num_chunks`, `group_state` and
`start_chunk_idx` are all in scope at the insertion point. In 0.26–0.29 this is a
single-pass loop, so clamping right before the slice fixes both the
`offload_keys` slice and the strided `block_ids` slice.

### The fix landed upstream incrementally

The clamp has three independent guards — `num_chunks` must not exceed the raw
offloadable-chunk count, the **backing GPU blocks** (`len(block_ids) //
blocks_per_chunk`), or the **available keys** (`len(offload_keys)`). vLLM added
them to `storable_chunks()`'s `return min(...)` over three releases:

| vLLM      | `storable_chunks` returns                                          | block_ids-shortfall (the crash we observed) | offload_keys-shortfall | patcher does |
|-----------|--------------------------------------------------------------------|:--:|:--:|--------------|
| 0.26.0    | `num_chunks` (raw)                                                 | **unclamped → crashes** | unclamped | **applies clamp** (both guards) |
| 0.27–0.29 | `min(num_chunks, num_allocated_chunks)`                            | fixed upstream | unclamped | **applies clamp** (adds the offload_keys guard) |
| 0.30.0+   | `min(num_chunks, num_allocated_chunks, num_keyed_chunks)`          | fixed upstream | **fixed upstream** | **no-op** (detects `num_keyed_chunks`) |

`num_allocated_chunks == len(block_ids) // blocks_per_chunk`;
`num_keyed_chunks == len(offload_keys)`. Validated on the stock base images
(patcher applies cleanly, idempotent, recompiles on 0.26/0.28; no-ops on 0.30):

- **0.26.0** — no upstream clamp at all. This is where the crash was observed
  (a finishing request crosses an unbacked chunk boundary; the `block_ids` slice
  comes up short). The patcher's clamp is required and load-bearing here.
- **0.27.0 / 0.28.0 / 0.29.0** — `storable_chunks` clamps `num_chunks` to
  `num_allocated_chunks`, which is exactly the block_ids-shortfall guard, so the
  **observed crash is already fixed upstream** from 0.27 on. The patcher still
  applies (anchor unchanged) and adds the `len(offload_keys)` guard on top; that
  term is identity except in an offload_keys-shortfall, which was not the crash
  we saw — so on these versions the patch is defensive, not a known fix.
- **0.30.0+** — `storable_chunks` clamps to **both** `num_allocated_chunks` and
  `num_keyed_chunks` before `num_chunks` is ever used, so `assert len(offload_keys)
  == len(offload_block_ids)` (still present at ~line 1569 as an invariant check)
  **cannot fire**, and the downstream second loop's `block_ids[gpu_block_idx + i]`
  is in bounds. **fix#3 is fully upstream — there is nothing to port.** vLLM 0.30
  also rewrote `_build_store_jobs` into two passes (a `group_store_ranges` list),
  so the 4-line anchor is absent anyway; the patcher detects the upstream clamp
  (`"num_keyed_chunks" in src`) and no-ops with an accurate message rather than
  the anchor-mismatch WARNING. Running 0.30 with `VLLM_FIX3=1` is correct and a
  no-op; `VLLM_FIX3=0` on 0.30 is equally safe (stock 0.30 is not crash-prone).

## Why fix#3 here is shmq-only

The canonical fix#3 lives in
`benchmarks/kv-offload-replay/vllm-fix2/scheduler.py` (fork `dwaddington/vllm`
branch `fix/tiering-deferred-finalize-v0.26.0`, commit `83571bcb`). That file
also carries **fix#2** — three `self.manager.mark_stores_submitted(...)` calls (a
deferred-finalize handshake). That handshake requires
`TieringOffloadingManager.mark_stores_submitted()`; `CertusShmqOffloadingSpec`'s
manager does **not** implement it, so applying the full fix2 file to the shmq
image would `AttributeError`. `apply_fix3.py` inserts **only** the fix#3 clamp —
no fix#2 handshake. The shmq crash is the fix#3 assert, not the fix#2
`_req_state` KeyError, so fix#2 is not needed here.

## `scheduler.orig.py` / `scheduler.fix3.py` (0.26.0 reference baselines)

These two files are the **superseded** whole-file artifacts, kept only as a
0.26.0 reference: `scheduler.orig.py` is the pristine 0.26.0 scheduler and
`scheduler.fix3.py` is the same file with the clamp added, so
`diff scheduler.orig.py scheduler.fix3.py` shows exactly the one clamp block.
The run script no longer mounts them — `apply_fix3.py` is the live mechanism.

To refresh the 0.26.0 baseline:

```bash
STORE=(--root /mnt/certus1/podman/storage --runroot /mnt/certus1/podman/run)
TGT=$(podman "${STORE[@]}" run --rm --entrypoint python3 localhost/certus-shmq-connector:vllm0.26.0 -c \
  'import vllm.distributed.kv_transfer.kv_connector.v1.offloading.scheduler as s; print(s.__file__)')
podman "${STORE[@]}" run --rm --entrypoint cat localhost/certus-shmq-connector:vllm0.26.0 "$TGT" > scheduler.orig.py
# then re-apply the one clamp block (see the diff) to produce scheduler.fix3.py
```
