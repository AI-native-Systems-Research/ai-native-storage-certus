# vllm-fix-kvload-hol — vLLM 0.30.0 scheduler deadlock fix (provenance)

`scheduler.py` here is stock vLLM 0.30.0 `vllm/v1/core/sched/scheduler.py` with one
change, shown in full in `scheduler.patch`. The vLLM image Dockerfiles copy this
directory in and run `apply.sh "${VLLM_VERSION}"`, which replaces the installed file
only on a 0.30.0 base whose file is byte-identical to stock, and fails the build
otherwise. Build with `--build-arg VLLM_FIX_KVLOAD_HOL=0` to opt out.

| | |
|---|---|
| Upstream file | `vllm/v1/core/sched/scheduler.py`, vLLM 0.30.0 (`docker.io/vllm/vllm-openai:v0.30.0`) |
| Stock md5 | `849cc72e032b672f310e32cfc42eaef8` |
| Patched md5 | `a01271366fcb7ec89237c795b8565151` |
| Applies to | vLLM 0.30.0 only; other versions build unpatched with a notice |
| Images | `certus-offload:vllm0.30.0` (`Dockerfile.offload`), `certus-shmq-connector:vllm0.30.0` (`certus-shmq-connector/Dockerfile`) |
| Upstream status | not reported or merged upstream yet |

## The bug

Under sustained async KV loading (any KV connector that loads asynchronously; seen
with both the vLLM-native `OffloadingConnector` tiering backend and the certus SHMQ
connector) the engine can stop forever with **0 running, N waiting**, most of them
reported as "deferred", while the engine sleep-polls in `_process_engine_step`. It is
a circular wait inside the scheduler:

1. The waiting loop is FCFS and reads `skipped_waiting` first. When `allocate_slots()`
   returns `None` for the request at the head, the loop `break`s; it never skips
   ahead.
2. The head request is an async-load admission, so `allocate_slots()` is passed
   `reserved_blocks = _inflight_prefill_reserved_blocks()`. That reservation still
   counts the remaining blocks of requests whose loads have **already finished**.
   The head does not fit and the loop breaks.
3. Those finished-load requests (`WAITING_FOR_REMOTE_KVS`, ids already in
   `finished_recving_kv_req_ids`) sit behind the head, so the loop never reaches
   them and never promotes them. They never run, so their reservation and the KV
   blocks they hold are never released.

Evidence (2026-11-20, cputier arm, 16 in-flight, CC-weka think-time replay): two
engine state dumps taken 20 s apart were identical. The offload path was idle (no
connector or tier jobs, no keys being loaded, all lookups resolved, GPU copy queues
empty), 3 requests were `WAITING_FOR_REMOTE_KVS` with their ids in
`finished_recving_kv_req_ids`, and GPU KV usage was 0.5299 — exactly the 190,720
loaded tokens those 3 requests held out of a 359,952-token pool. Preemptions were 0.

## The fix

Before the waiting loop runs, `Scheduler._certus_front_finished_kv_loads()` moves
every `WAITING_FOR_REMOTE_KVS` request whose id is in `finished_recving_kv_req_ids`
to the front of `skipped_waiting`, keeping their relative order. They are then
promoted and scheduled by the normal path (`_try_promote_blocked_waiting_request`)
before anything new is admitted. It only reorders the queue; it is a no-op without a
KV connector, without finished loads, when they are already at the front, and under
the `PRIORITY` policy (where position does not decide selection). Each real reorder
logs one `[certus-fix]` INFO line.

## Validation

Mounted over stock 0.30.0 images (same file), CC-weka think-time replay, Qwen2.5-7B-1M
dense @ 262k, 64 GB DRAM tier, 3600 s per arm:

- Unpatched: both backends wedged at 8 and 16 in-flight, cputier also at 32 and 64.
- Patched: 20 of 20 arms (certus and cputier at 4, 8, 16, 32, 64 in-flight, plus
  reruns) ran the full hour with no stall; the fix fired 246–589 times per arm.
- Control: certus at 32 in-flight never wedged unpatched; patched it is unchanged
  (0.57 req/s and TTFT p50 49.9 s, vs 0.57 req/s and 50.3 s), so the fix does not
  cost performance when there is no deadlock.

Results: `/mnt/certus1/vllm-serve-matrix/ccweka-lowconc-patched-combined.md`.

## Re-vendoring

The copy is a whole-file replacement for exact 0.30.0 source. To move to another
vLLM version, extract that version's `scheduler.py` from its base image, apply the
same change (see `scheduler.patch`), and update `STOCK_MD5`, `PATCHED_MD5` and the
version check in `apply.sh` plus the table above.
