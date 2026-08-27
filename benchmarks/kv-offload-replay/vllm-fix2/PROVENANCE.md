# vllm-fix2 — vendored vLLM tiering fix (provenance)

The three `.py` files in this directory are a **verbatim, byte-identical copy**
of the patched vLLM sources from our fork. They are vendored here so
`Dockerfile.offload` can `COPY` them over the prebuilt `vllm/vllm-openai:v0.26.0`
base image (build with `--build-arg VLLM_FIX_TIERING=1`), baking the fix into a
built image instead of relying on runtime bind-mounts.

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
