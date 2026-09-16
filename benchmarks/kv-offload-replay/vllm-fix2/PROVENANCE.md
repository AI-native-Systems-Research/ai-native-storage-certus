# vllm-fix2 — vendored vLLM tiering fix (provenance)

The `.py` files in this directory are a copy of the patched vLLM sources from
our fork, plus one additional local fix (**fix#3**, see below) not yet on the
fork. They are vendored here so `Dockerfile.offload` can `COPY` them over the
prebuilt `vllm/vllm-openai:v0.26.0` base image (build with
`--build-arg VLLM_FIX_TIERING=1`), baking the fix into a built image instead of
relying on runtime bind-mounts.

> **`scheduler.py` is NOT byte-identical to the fork commit below** — it carries
> fix#3 on top. `manager.py` and `base.py` are still verbatim. See "fix#3" and
> the re-sync warning before refreshing from the fork.

## Source

| | |
|---|---|
| Fork repo | `github.com/dwaddington/vllm` (fork of `vllm-project/vllm`) |
| Branch | `fix/tiering-deferred-finalize-v0.26.0` |
| Commit | `5e20aeb5` |
| Cut from tag | `v0.26.0` (exactly the version shipped by `vllm/vllm-openai:v0.26.0`) |

## What the fix does

Defers finished-request finalization in `TieringOffloadingManager` until the
connector scheduler has submitted (or determined unnecessary) the request's
**trailing-block store**, via a `mark_stores_submitted()` handshake. This closes
the lifecycle race where `prepare_store()` indexed `_req_state[req_id]` after the
manager had already deleted that state — the `KeyError → EngineDeadError` crash
that made the as-shipped tiering plugin complete 0/10 runs at 450 convs.

Validated locally: patched build completes 10/10 (450 convs × 12 turns) vs 0/10
for the stock base image.

## fix#3 — `scheduler.py` `_build_store_jobs` length clamp (local, not yet on fork)

A **second, independent** tiering bug, surfaced under a guidellm shared-prefix
sweep against `run-serve-cputier.sh` (reproduced on the fix026 image, so the
handshake above does not cover it):

```
offloading/scheduler.py  _build_store_jobs()
    assert len(offload_keys) == len(offload_block_ids)   # AssertionError → EngineDeadError
```

`offload_keys` advances every step in `_update_req_states` (`update_offload_keys`
is unconditional) but `block_ids` only grows when the scheduler reports newly
allocated blocks. On the finished path `num_offloadable_tokens` jumps to
`req.num_tokens`, so `num_chunks` can cross a chunk boundary whose
`blocks_per_chunk` GPU blocks were never appended to `block_ids` (the finishing
decode reused an already-allocated block). The strided `block_ids` slice then
comes up short and the `assert` fatally kills the engine. Load-dependent: fired
after ~1500 requests in a saturating sweep.

**Fix:** clamp `num_chunks` to `min(num_chunks, len(offload_keys),
len(block_ids)//blocks_per_chunk)` before the slice/assert. Correctness-
preserving — only chunks with both a stable hash and backing GPU blocks are
stored; an unbacked trailing chunk (on a finishing request, with no future step
to flush it) is dropped, costing at most a future prefix-cache hit, matching the
tradeoff the fix2 PR already accepted for its "defensive guard" alternative.

**Not yet upstreamed to the fork.** When fix#3 lands on
`fix/tiering-deferred-finalize-v0.26.0`, update the commit hash above and this
note; until then the re-sync command below WILL DROP fix#3 from `scheduler.py`.

## Files and their in-image destinations

The base image's Python root is `/usr/local/lib/python3.12/dist-packages`.

| Local file | In-image destination |
|---|---|
| `manager.py` | `.../vllm/v1/kv_offload/tiering/manager.py` |
| `scheduler.py` | `.../vllm/distributed/kv_transfer/kv_connector/v1/offloading/scheduler.py` |
| `base.py` | `.../vllm/v1/kv_offload/base.py` |

**These files are valid for vLLM 0.26.0 only.** They must not be applied to any
other base version (e.g. the 0.23.0 sharedstorage/shmq images).

## Re-syncing from the fork

**Warning:** until fix#3 is upstreamed to the fork, this overwrites `scheduler.py`
and drops fix#3. Re-apply the `_build_store_jobs` `num_chunks` clamp afterward.

To refresh these copies from the fork branch head (run from this directory):

```bash
BR=fix/tiering-deferred-finalize-v0.26.0
declare -A P=(
  [manager.py]=vllm/v1/kv_offload/tiering/manager.py
  [scheduler.py]=vllm/distributed/kv_transfer/kv_connector/v1/offloading/scheduler.py
  [base.py]=vllm/v1/kv_offload/base.py
)
for f in manager.py scheduler.py base.py; do
  gh api "repos/dwaddington/vllm/contents/${P[$f]}?ref=$BR" -q '.content' | base64 -d > "$f"
done
```
