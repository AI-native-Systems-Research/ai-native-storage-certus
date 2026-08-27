# Selecting the stock vs. fixed vLLM 0.26 tiering image

The vLLM 0.26.0 native tiering plugin (`OffloadingConnector` →
`TieringOffloadingSpec`: CPU primary tier + `fs` disk-spill secondary) crashes at
scale — `state = self._req_state[req_id]` KeyError → `EngineDeadError` —
completing **0/10** runs at 450 convs. Our fork carries a pure-Python fix
(deferred finished-request finalization + a `mark_stores_submitted()`
store-submission handshake, 3 files) that takes the tiering arm to **10/10**.

Fork: `github.com/dwaddington/vllm`, branch
`fix/tiering-deferred-finalize-v0.26.0` @ `5e20aeb5` (cut from the `v0.26.0`
tag). See [`vllm-fix2/PROVENANCE.md`](vllm-fix2/PROVENANCE.md) for the vendored
source of truth and how to re-sync it.

## The selection is by image tag, not a runtime flag

Stock and fixed are **two separately-built images**. The choice is baked in at
**build time** via the `VLLM_FIX_TIERING` build-arg; there is no runtime switch
that flips one into the other.

| Tag | Build-arg | vLLM tiering behavior |
|---|---|---|
| `certus-offload-bench` | `VLLM_FIX_TIERING=0` (default) | stock 0.26.0 — reproduces the `_req_state` KeyError crash |
| `certus-offload-bench-fix026` | `VLLM_FIX_TIERING=1` | forked fix baked in — completes cleanly |

Only `Dockerfile.offload` (the 0.26.0 tiering image) is affected. The
sharedstorage and shmq images are vLLM 0.23.0, lack the tiering framework, and
are not touched by this build-arg.

## Building

Both tags in one go:

```bash
bash benchmarks/kv-offload-replay/build_026.sh
```

Or build one directly (context = repo root):

```bash
# stock (as-shipped / crashing arm)
podman build --build-arg VLLM_VERSION=0.26.0 \
  -f benchmarks/kv-offload-replay/Dockerfile.offload -t certus-offload-bench .

# fixed (forked tiering fix baked in)
podman build --build-arg VLLM_VERSION=0.26.0 --build-arg VLLM_FIX_TIERING=1 \
  -f benchmarks/kv-offload-replay/Dockerfile.offload -t certus-offload-bench-fix026 .
```

The overlay is a `COPY` of the 3 vendored files over the prebuilt base image —
the base already ships vLLM 0.26.0's compiled extensions, so **no vLLM source
rebuild** happens and the build is fast. `VLLM_FIX_TIERING=1` also asserts the
base is `0.26.x` and fails loudly on any other version, so the patch can never
be silently applied to the wrong base.

## Running

Point the harness at the tag you want. The stability scripts already default to
the correct one for each arm:

- `run_cputier_stability.sh` → `certus-offload-bench` (stock / crashing arm)
- `run_cputier_patched_stability.sh` → `certus-offload-bench-fix026` (fixed arm)

Override on either with the `IMAGE` env var:

```bash
# run the fixed image through the (stock-default) harness
IMAGE=certus-offload-bench-fix026 bash run_cputier_stability.sh out/

# run the stock image through the (patched-default) harness
IMAGE=certus-offload-bench bash run_cputier_patched_stability.sh out/
```

Because the fix is baked into `certus-offload-bench-fix026`, there is **no
runtime patch bind-mount** — the only difference between the two arms is which
tag is run, so any change in reliability/throughput is attributable to the
patch alone.

## Telling a built image apart

```bash
# label: 1 = fixed, 0 = stock
podman image inspect certus-offload-bench-fix026 \
  --format '{{index .Config.Labels "org.certus.vllm-fix-tiering"}}'

# handshake symbol: non-zero = fixed, 0 = stock
podman run --rm --entrypoint sh certus-offload-bench-fix026 \
  -c 'grep -c mark_stores_submitted \
      /usr/local/lib/python3.12/dist-packages/vllm/v1/kv_offload/tiering/manager.py'
```

The build also prints a sentinel line: stock →
`stock vLLM 0.26.0 (tiering fix NOT applied)`; fixed →
`baked tiering fix (fork fix/tiering-deferred-finalize-v0.26.0 @5e20aeb5)`.

---

**In short:** stock = `certus-offload-bench`, fixed =
`certus-offload-bench-fix026`. Choose by which tag you build (`VLLM_FIX_TIERING`)
and which tag you run (`IMAGE`).
