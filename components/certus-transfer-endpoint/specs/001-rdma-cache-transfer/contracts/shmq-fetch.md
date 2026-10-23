# Contract: the connector's FETCH message (T067)

**Feature**: `001-rdma-cache-transfer` | **Date**: 2026-10-22

How the vLLM connector asks its local Certus to fetch a run of blocks from a named peer
into its memory tier. One new shared-memory-queue op, `FETCH`, in
`lib/shmq-dispatcher`'s wire protocol, served by the transfer endpoint. Interface contract
of the transfer itself: [itransfer_endpoint.md](./itransfer_endpoint.md).

## Where it fits

vLLM reaches an offloading connector only through **lookup**, and lookup may answer
`HIT_PENDING`: the request is set aside and all its lookups re-run on a later scheduler
step, without blocking (vLLM ≥0.25; `knowledge/contracts/vllm-offloading-interface-v030.yaml`
`lookup_semantics`). A fetch is therefore started from lookup and invisible to vLLM
otherwise (research D2): once fetched, blocks are ordinary memory-tier entries, loaded by
the existing CHECK → PIN → LOOKUP → UNPIN path.

```text
vLLM lookup(keys)            manager                              certus (local)          peer
  ── keys k0..kN ──────────▶ CHECK k0..kN ──────────────────────▶ RESIDENT×h, MISS×…
                             leading miss run = k_h..kN
                             FETCH(peer, k_h..kN) ────────────────▶ window: reserve, fetch ─▶
  ◀── HIT_PENDING ─────────  (answers PENDING for the run)         ◀─ writes, reply ─────────
  … next scheduler step …                                          publish hits, release misses,
  ── keys again ───────────▶ CHECK ──────────────────────────────▶   advance the window
                             RESIDENT up to the first MISS
  ◀── HIT / MISS ──────────  (load as today)
```

### With P/D disaggregation

`MultiConnector` assigns each request to **one** connector — the first with a hit
(`multi_connector.py:457-470`) — and every connector sees only tokens past the GPU
prefix-cache hit. So: on a **prefill** pod a fresh request draws nothing from NIXL, and
Certus (local, or fetched from a peer) supplies what it can before prefill computes the
rest; on a **decode** pod NIXL is listed first and pulls the prompt GPU→GPU, and Certus only
stores. A peer fetch is therefore a prefill-side (or non-disaggregated) action; it never
competes with NIXL's pull.

## The peer hint

`kv_transfer_params["certus_fetch_from"] = "host:port"` — one Certus transfer-endpoint
instance, named exactly as it names itself (D16, D24). Absent: no fetch, behaviour as
today. llm-d's routing sets it in the end; until a scorer exists, a client or proxy sets it
by hand. The manager reads it from `req_context.kv_transfer_params`, as it already reads
`session_id`.

## Wire

Opcode **17, `FETCH`**. Little-endian, unaligned, like every op.

```text
request  { peer_len: u16, peer: [u8; peer_len] (UTF-8 "host:port"),
           n: u32, [key: u64, size: u32] * n }
response { status: u8 }  then  [started: u8] * n
```

- `keys` are in prefix order: the order vLLM would use them. Duplicates are refused
  (`STATUS_ERROR`), as for every op.
- `status`: `0` accepted; `1` no endpoint in this server; `2` the peer does not parse, or
  names this instance; `3` the request names more keys than `max_fetch_keys`.
- `started[i]`: `1` the key is now part of the fetch (it reads `PENDING` until it ends);
  `0` it is not — already resident, or already pending (a store or another fetch). The run
  is the keys answered `1`, in order; a `0` inside it ends the run there, since what follows
  is not a usable prefix without it.
- The op **never blocks** on the transfer: it returns once the first window is issued.
  Each key's fate is read with CHECK — `RESIDENT` (fetched, or already there) or `MISS`.

## The window (server side)

One FETCH names the whole leading miss run; the server paces it, so unpublished memory is
bounded and the pipe stays full without a scheduler step between batches.

- **Window**: at most `fetch_window_bytes` (default 128 MiB) of unpublished slots per
  fetch. The server reserves slots (`reserve_memory`, the store path's unpublished
  reservation) for the next keys of the run until the window is full, and passes them to
  `ITransferEndpoint::fetch(peer, [(key, LocalSlot{addr, length})])`.
- **Completion**: each hit is published (`copy_gpu_to_memory_completed`, which also
  schedules write-through, so the entry is demotable); each miss or failure is released
  (`release_memory`). Then the next part of the run is reserved and fetched, keeping the
  window full.
- **Stop** at the first miss: blocks after it are not a usable prefix (vLLM's scan stops at
  the first MISS), so the rest of the run is dropped and reads `MISS`. Also stop when a
  reservation is refused (the tier is full) or the global cap is reached: what was fetched
  stands, the rest reads `MISS` and is recomputed.
- **Global cap**: at most `fetch_inflight_bytes` (default 1 GiB) unpublished across all
  fetches. A window that cannot be filled issues what fits; one that gets nothing waits for
  the next completion rather than spin.
- A slot is reclaimed **only** by its fetch's completion — never by the stale-reservation
  reaper, since the peer may still be writing it (FR-017). The endpoint guarantees the
  completion fires, after its deadline teardown at worst, so no slot is stranded.
- Per-key state lives with the fetch, not in `pending_stores`, so the reaper cannot see it;
  CHECK and TOUCH_CHECK answer `PENDING` for a key in either map.

## Attribution: a fetched hit is `FETCHED`, not `DRAM`

A fetched block is an ordinary memory-tier entry by the time vLLM loads it, so the
dispatcher's `served_by` for it is `Dram`, and `certus_lookup_hits_dram_total` would silently
absorb every remote fetch. Remote fetch and remote lookup are also kept apart: they are
different mechanisms, decided by different parties, with different costs.

- **A new attribution value, `ServedBy::Fetched`** (interfaces), a hit, distinct from
  `Remote` (remote lookup's in-lookup delivery). Wire byte **4** on `LOOKUP`: readers test
  `byte != 0` and an unknown non-zero value already means *served, tier unknown*
  (dispatcher spec 002 `contracts/served-by.md`), so no reader breaks.
- **Decided by the translator, at the first serve.** When FETCH publishes a hit, the key is
  marked `fetched` (with its peer and landing time). The first `LOOKUP` that serves a marked
  key reports `Fetched` in place of the dispatcher's `Dram`, and drops the mark: from then on
  the block is a local entry and its hits are honestly `Dram`. A mark is also dropped when the
  entry is evicted (the eviction events the translator already drains) or removed, and after
  `fetch_mark_ttl` (default 30 s) — so a stale mark can never re-attribute a later, unrelated
  entry under the same key.
- **Counted, not inferred**, beside the tier counters. Prometheus names are
  `certus_<counter>_total` (`certus_fetch_keys_pending` and `certus_fetch_marks_live` are
  gauges), OTel `certus.fetch.<name>`; exported only by a server that fetches, so a profile
  without an endpoint shows no fetch series rather than zeros. The exported
  `certus_lookup_hits_dram_total` is **net** of `lookup_hits_fetched`: the dispatcher counted
  each fetched first serve as `Dram`, and the translator re-attributed it, so the route
  partition `dram + ssd + fetched + remote = lookup_hits` holds:

| counter | meaning |
|---|---|
| `fetch_requests`, `fetch_keys_requested` | FETCH ops received; keys they named |
| `fetch_keys_started` | keys accepted into a fetch's run (`started = 1` on the wire) |
| `fetch_not_started` | keys refused at once: already resident, already pending |
| `fetch_hits`, `fetch_bytes` | blocks published from a peer; their bytes |
| `fetch_misses` | the peer does not hold them |
| `fetch_failed` | lost to a fault: transport, deadline, refused |
| `fetch_truncated` | started keys never fetched: after the first miss, or a refused reservation |
| `fetch_window_waits` | windows the global cap held back |
| `lookup_hits_fetched` | fetched blocks a LOOKUP then delivered (attribution `Fetched`) |
| `fetched_unused` | fetched blocks evicted or aged out before any read — the waste |

- **Identities** (checked by test, and by the multinode run): once quiescent,
  `fetch_keys_requested = fetch_keys_started + fetch_not_started`,
  `fetch_keys_started = fetch_hits + fetch_misses + fetch_failed + fetch_truncated` (a started
  key is either fetched, or ends with its run cut short), and `fetch_hits = lookup_hits_fetched + fetched_unused + marks_live`. The endpoint's own
  `TransferStats` counts the same fetches from the transport side, a second, independent
  tally to reconcile against.
- **Known imprecision, accepted:** two requests sharing a prefix — whichever reads first is
  the one counted `Fetched`. Per block that is exactly one fetched serve per fetched copy,
  which is the number that measures what fetching saved.

## The manager (connector side)

vLLM 0.30's scan (`offloading/scheduler.py` `_maximal_prefix_lookup`) calls `lookup` once per
key in prefix order from the first not-yet-computed chunk, counts `HIT` and `HIT_PENDING` as
hits, and **stops at the first `MISS`**. The manager therefore sees a request's keys one at a
time, and learns the run by answering:

- **Gathering (first scan).** For a request whose `kv_transfer_params` names a peer, each key
  that reads `MISS` locally answers `HIT_PENDING` and joins the run, so the scan continues to
  the end of the leading misses. A key already here (`RESIDENT`, or someone else's store
  `PENDING`) closes the run; later misses do not join it.
- **Send.** `on_schedule_end`, called once per scheduler step after scheduling, sends one
  FETCH per gathered run — all W per-rank keys of each block. A block is in the run only if
  all its shards started. A refused FETCH (no endpoint, bad peer, too many keys) is not
  retried; that request then behaves as if it named no peer.
- **Re-scans.** A run key reads `RESIDENT` (`HIT`), `PENDING` (`HIT_PENDING`), or `MISS` (the
  fetch ended there); a key outside the run answers from residency alone. A block with any
  shard missing is not a hit.
- `on_request_finished` forgets the request.

- **Counts** its side too: `HIT_PENDING` answers, budgets spent, blocks answered `HIT` after
  a fetch — tying vLLM's view (which only ever sees `HIT`) to Certus's counters.
- **Time budget**: `CERTUS_FETCH_BUDGET_MS` (default 200; 0 disables fetching) per request,
  from its first hinted lookup. Once spent, answer from the
  current state — resident keys `HIT`, the rest `MISS` — so vLLM computes the remainder
  rather than wait on a slow peer. The fetch continues and lands in the cache for later.
- TP: a logical block is W per-rank keys (`ns_key`); FETCH carries all of them, to the one
  peer. A peer with another TP layout simply misses.

## Configuration

| name | where | default |
|---|---|---|
| `certus_fetch_from` | request `kv_transfer_params` | none (no fetch) |
| `CERTUS_FETCH_BUDGET_MS` | connector environment | 200 (0: never fetch) |
| `CERTUS_FETCH_WINDOW_BYTES` | server | 128 MiB |
| `CERTUS_FETCH_INFLIGHT_BYTES` | server | 1 GiB |
| `CERTUS_FETCH_MAX_KEYS` | server | 65 536 (a sanity bound on one message) |
| `CERTUS_RDMA_BIND_IP`, `CERTUS_RDMA_PORT` | server | auto-detect, 7481 |

## Milestones

1. **FETCH op + window** in `shmq-dispatcher`, the endpoint wired into a new
   `full-transfer` profile of `certus-server-yaml` (refused without `rdma`), the manager's
   hint path; tested over a mock endpoint, then two real servers node2 ↔ node5 driven over
   the ring, then vLLM on a non-disaggregated pod with the hint set by hand.
2. **P/D with both**: `MultiConnector[NixlConnector, Offloading(certus)]` on the
   `pd-disaggregation` deployment, NIXL first. Check that D stores, and that NIXL's lease
   still covers P's blocks; measure turn 2 (Certus fetch from D's cache vs NIXL's TTL
   readback).
3. **llm-d names the peer**: Certus's stored-block events into llm-d's KV-events index, and
   a routing plugin that sets `certus_fetch_from`. Its own design step.
