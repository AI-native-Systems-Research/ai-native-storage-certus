# Design: llm-d chooses the peer a request fetches from

**Feature**: `001-rdma-cache-transfer` | **Date**: 2026-10-23 | **Status**: milestones 1-3 built and verified in k8s (2026-10-26; M3 with the `CERTUS_EVENT_MEDIUM=cpu` bridge); milestone 4 not built

How a request's `FETCH` gets its peer without anyone setting `certus_fetch_from` by hand,
by riding llm-d's own peer-pull selection. Milestone 3 of
[contracts/shmq-fetch.md](./contracts/shmq-fetch.md). Covers P/D and aggregated serving.

## The problem

The connector fetches only when a request names a peer, `kv_transfer_params
["certus_fetch_from"] = "ip:port"`. On node5 that works when a client talks to the
prefill pod directly (367/367 blocks fetched and served, 457 ms vs 1115 ms computed). It
does not work through the router: llm-d's routing sidecar **rebuilds `kv_transfer_params`
from scratch** on every request it forwards (llm-d-router `1a7e2ade`,
`pkg/sidecar/proxy/connector_nixlv2.go:164` for P/D, `connector_p2p.go:372` for
decode-only: "the sidecar owns this field, so client-supplied keys are dropped"). Nothing a
client puts there reaches vLLM. And a client is the wrong party to choose the peer anyway:
it does not know which instance holds the prefix.

## What llm-d already does

llm-d has a pull path for exactly this, built for vLLM's own pod-to-pod offload tier:

1. **EPP chooses a source.** The `p2p-source-producer` plugin
   (`pkg/epp/.../dataproducer/p2psource`) reads per-endpoint prefix-cache match data from
   a prefix-cache producer, samples among the endpoints within one block of the longest
   cached prefix (weighted by queue depth; or by an optional cost model), and, after
   scheduling, sets the header **`x-llm-d-kv-cache-source-host-port: <pod-ip>:<port>`**
   when the source out-caches the pod that will compute by at least
   `minCachedTokenDelta` tokens. Any inbound value of the header is stripped.
2. **The sidecar turns it into transfer params.** With `--kv-connector=nixlv2
   --enable-p2p-pull` (or `--kv-connector=offloading`), the sidecar adds to the request it
   forwards:
   - P/D, on the prefill hop (`addP2PPullToPrefill`, `connector_p2p.go:258`);
   - no prefill (aggregated, or decode-only), on the one hop (`decodeWithP2PSource`,
     `connector_p2p.go:351`);

   ```json
   "remote_kv_source": {"kv_request_id": "<uuid>", "remote_host": "<pod-ip>", "remote_port": 7777}
   ```

   `remote_host` is the **source pod's IP**; `remote_port` is the sidecar's
   `--p2p-connector-port` (default 7777, plus the DP rank), the port vLLM's p2p tier
   listens on. It is skipped when the source is the computing pod itself.
3. **vLLM's p2p tier consumes it** (`vllm/v1/kv_offload/tiering/p2p/manager.py`
   `_parse_source`): the consumer fetches from that pod.

So llm-d already answers "which instance holds this prefix", at the scheduler, with the
whole fleet's cache state; and already delivers the answer into `kv_transfer_params`. What
it names is a **vLLM pod**, not a certus instance.

## Proposal

**The connector accepts `remote_kv_source` as a peer hint, and maps the pod to the certus
instance that pod uses.** `certus_fetch_from` stays, as an explicit override for clients
that bypass the router, and takes precedence.

### Pod → certus instance

A pod's KV lives in the certus instance on **its node and its NUMA domain** (the certus
entrypoint chooses the mailbox from the GPU's NUMA node). That instance is named
`<node RoCE IP>:<7481 + numa>` (`deploy/k8s/certus-server-numa.yaml.tpl`). Two ways to
resolve a source pod IP to it:

- **(a) The source pod says (recommended).** The certus entrypoint already computes, at
  start, everything needed: the NUMA node it chose (so the port, 7481 + NUMA) and, from the
  node's RDMA netdev, the RoCE IP (as `first_active_rdma_ipv4` does in the server). It
  writes them onto **its own pod** as an annotation,
  `certus.ai/transfer-endpoint: 10.0.0.105:7482`. The reading connector finds the source
  pod by IP (`get pods --field-selector status.podIP=<ip>`) and reads it. Both sides need
  a Role in the namespace -- `get`/`list` pods to read, `patch` its own pod to write --
  which the overlay adds with a ServiceAccount. (The modelserver container has the pod IP
  as `VLLM_NIXL_SIDE_CHANNEL_HOST`, from `status.podIP`.) A per-node file instead of an
  annotation does not work: the source pod is on another node.
- **(b) The reader derives it.** The connector looks up the source pod's node
  (`spec.nodeName`) and the node's RoCE IP from a node annotation set once at provisioning
  (`certus.ai/roce-ip`, as `certus-nodes.sh` already annotates claims). The gap is the
  NUMA domain, which the API does not show: on a two-NUMA node there are two candidate
  instances, and trying both (:7481, then :7482 on a total miss) costs a round trip per
  miss on the wrong one.

**Recommendation: (a).** It is exact (the entrypoint knows the NUMA node it chose), it
is one record with the pod's own lifetime, and the reader needs one `list` per new
source. The connector caches the map per source IP
for the life of the pod (a restarted pod has a new IP).

### Whose cache view the EPP ranks by

The plugin ranks sources by a prefix-cache producer's per-endpoint match data. Two kinds:

- **Approximate** (`approx-prefix-cache-producer`) -- **the one this deployment already
  runs.** It needs no KV events: it hashes each prompt and remembers which endpoint it
  routed it to, with an LRU per server. Its data has no tiers, and the plugin "trusts it
  as-is" (`producer.go:514`). For certus that is close to right: a pod that served a
  prefix stored it in its certus, and certus keeps it longer than the GPU does (DRAM,
  then SSD). Its error is the other way -- it forgets after its LRU capacity
  (`lruCapacityPerServer`, auto-tuned from the pod's GPU KV capacity), long before certus
  does -- which errs toward computing, never toward a fetch that misses.
- **Precise** (`precise-prefix-cache-producer`) -- built from the KV events every pod
  publishes, with a per-tier breakdown. Two gaps for certus:
  1. **Certus stores are not reported.** The connector's events carry removals only
     (`manager.py` `take_events`: "Only REMOVED means the key is no longer accessible"),
     medium `"Certus"`. The EPP can only know a pod's certus holds a prefix if the
     connector emits `removed=False` events for the blocks it stores.
  2. **The tier name.** With per-tier data the plugin counts only the `"cpu"` tier
     (`producer.go:509`), the one vLLM's p2p tier can serve, so certus's blocks would be
     ignored. Either the plugin learns other pull-servable tiers (an llm-d change: a
     `pullableTiers` parameter), or certus reports itself as `"cpu"` -- simple, but false
     about the medium, and confusing beside a real CPU tier.

**Recommendation: start with the approximate producer, which needs nothing from certus.**
Move to the precise one -- store events, certus's own tier name, `pullableTiers`
upstream -- only if measurement shows the approximate view misses fetches worth having.

### Sidecar configuration

- P/D: `--enable-p2p-pull` on the decode sidecar (`--kv-connector=nixlv2` stays). Today
  it is off, so no source header is consumed.
- Aggregated: the aggregated overlay has **no sidecar** at all, so the header has nowhere
  to go. Adding one (llm-d's `routing-sidecar` component with `--kv-connector=offloading`)
  puts a proxy hop in front of every aggregated request. The alternative is for vLLM to
  read the header itself -- not supported. So aggregated serving gains this only with the
  sidecar.
- `--p2p-connector-port` can stay at its default (7777). The sidecar only copies it into
  `remote_port`, never dials it, and the connector ignores it: the certus port comes from
  the annotation.

### What `remote_kv_source` also means to vLLM

If a pod ran vLLM's own p2p tier beside certus, it would also act on `remote_kv_source`.
Ours does not: the certus profiles configure `OffloadingConnector` with
`CertusShmqOffloadingSpec`, which has no p2p tier, and `_parse_source` lives in the p2p
tier's manager. So the field reaches only the certus connector. A deployment that wanted
both would need the two to agree who acts on it -- out of scope.

## Aggregated serving and session moves

The same path covers the case of a session moved between pods at a turn boundary: the EPP
schedules every turn independently, scoring prefix affinity against load, and when it moves
a session the old pod holds the prefix. The plugin names it as the source, the sidecar
(with one in front of aggregated pods) adds `remote_kv_source`, and the connector fetches
from the old pod's certus. The old pod may be on the same node, the other NUMA domain: then
the peer is the sibling instance on the same host, which the `host:port` naming handles; the
transfer crosses the NIC twice. On the a30 profile such moves are rare -- upstream's
`peakPrefillThroughput` is far above an A30's, so affinity effectively never yields to load.

## Changes, by component

| where | change | size |
|---|---|---|
| certus entrypoint (`deploy/llm-d/.../entrypoint-certus.sh`) | annotate its own pod `certus.ai/transfer-endpoint=<roce-ip>:<7481+numa>`; the RoCE IP from the node's RDMA netdev | small |
| llm-d overlays | ServiceAccount + Role + RoleBinding (get/list pods, patch own pod); `--enable-p2p-pull` + `--p2p-connector-port` on the decode sidecar | small |
| EPP config (router values) | add `p2p-source-producer`, fed by the `approx-prefix-cache-producer` already there | small |
| connector (`manager.py`) | read `remote_kv_source.remote_host`, resolve via the annotation (cached by IP), `certus_fetch_from` overrides | medium |
| later, only if measured worthwhile | store events from the connector; `precise-prefix-cache-producer` with vLLM KV events; `pullableTiers` upstream in llm-d | medium, partly theirs |

## Risks

- **Selection quality.** The approximate view is a guess from routing history: it may name
  a source whose certus has since evicted the prefix -- a fetch that misses fast and is
  computed instead, bounded by the budget and counted (`fetch_misses`). It cannot name a
  source it never routed to. The precise view would fix both, at the cost above.
- **API reads on the request path.** A `get pod` per new source IP; cached for the pod's
  life, so at most once per peer per pod restart.
- **The plugin only sets the header when the source out-caches the computing pod.** A
  computing pod with a short local prefix and a peer with a long one is exactly the case;
  a computing pod that already holds the prefix gets no header and needs none.
- **Upstream dependency**, only for the precise view: `pullableTiers` in llm-d, or
  reporting certus as `"cpu"`.

## Not proposed

- A sidecar patch that forwards `certus_fetch_from` (option 2): a fork of llm-d, and it
  still leaves the client choosing the peer.
- Teaching the EPP about certus directly (a certus-specific scorer): duplicates what the
  p2p producer does, and llm-d already has the plumbing.

## Milestone 1, as built

The pod-to-certus mapping changed from the proposal above: the entrypoint cannot read the
node's RoCE address from inside the pod (no host network; the pod's RoCE GIDs read zero,
see `deploy/k8s/rdma-secondary-network.md`). Instead **the certus server publishes its own
name beside its mailbox**, `<mailbox>.endpoint`, and the entrypoint annotates the pod with
what it reads there -- exact by construction, since the mailbox is the one the pod uses.

Verified on node2/node5 (certus `b9a8af52`, connector `vllm0.30.0-b9a8af52`), the header set
by hand at decode1's sidecar, `x-llm-d-kv-cache-source-host-port`, prefill on node5:

| source named | outcome |
|---|---|
| decode0's pod (its certus, node2 NUMA 0, holds the prompt) | 367/367 blocks fetched into node5 and served as fetched |
| decode1's pod (its certus lacks the prompt) | fetch sent, 56 misses, computed |
| an IP with no pod | no fetch; logged "has no certus.ai/transfer-endpoint" |

All three pods annotated themselves at start. The request through decode's sidecar still
spends ~8 s in the NIXL pull over TCP; that is the secondary-network proposal's subject, not
this one's.

## Milestone 2, as built

`deploy/llm-d/profiles/a30/pd-certus/router-values.yaml` adds `p2p-source-producer` to the
P/D router's EPP config, on the approximate prefix producer already there, with
`minCachedTokenDelta: 1024` and `--allow-experimental-plugins` (the plugin is Alpha).
`install.sh` layers a profile's router values over the guide's. Rolled out: the EPP loads the
plugin and logs a decision per request.

**Not exercised on this cluster, by construction.** The plugin names a source only when one
pod holds at least `minCachedTokenDelta` more of the prompt than *the pod that will compute
it*. Under P/D the computing pod is the prefill pod, and the approximate producer records
every request against both the decode pod and the prefill pod it was routed to
(`approximateprefix` `PreRequest`). With **one** prefill pod -- the node2/node5 testbed --
that pod has served every prompt, so it always holds at least as much as any decode pod:
three requests with a 7 858-token shared prefix logged `below-floor`, delta 0, each time.

The case that matters is **decode -> prefill**, not prefill -> prefill. A turn's prompt is
the previous turn's prompt, plus the model's answer, plus the new message. The previous
prompt is already in the prefill pod's own cache; the answer's KV was generated on the
decode pod and exists nowhere else. Prefill -> prefill arises only when a session's turns
land on different prefill pods, which the affinity filter works against.

Three things stood in its way; the first is fixed:

1. **Decode stored only the prompt.** vLLM's offloading connector defaults to
   `offload_prompt_only: true` (`vllm/v1/kv_offload/base.py`), so decode's certus held the
   same blocks as prefill's and the answer's KV lived only in decode's GPU. **Fixed**:
   `CERTUS_OFFLOAD_PROMPT_ONLY=false` on the decode role (pd-certus patch, read by the
   certus entrypoint).
2. **The router cannot see what decode generated.** The approximate producer hashes the
   prompts it routes; generated tokens never reach it. Naming decode as the source needs the
   precise producer, fed by KV events, and certus store events from the connector:
   milestone 3.
3. **Re-tokenization.** Block keys are chained hashes of token ids, so turn N+1 matches
   decode's blocks only if it carries exactly the tokens decode generated. Qwen3's chat
   template strips earlier `<think>` blocks, so a chat-API conversation with thinking
   diverges after the answer starts; the completions API with the answer's tokens appended
   does not.

### Decode -> prefill, by hand (2026-10-26)

Turn 1 through decode0's sidecar (prefill on node5): a 3 081-token prompt, a 1 600-token
answer -- 100 whole blocks generated on decode. Turn 2: the same prompt + the answer's
token ids + a new question, 4 687 tokens, through the same sidecar.

| turn 2 | prefill node's certus |
|---|---|
| source header naming decode0 | 1 FETCH; **100/100 blocks fetched and served as fetched** -- exactly the answer; the prompt's 192 blocks were local |
| no source header (control) | no fetch: prefill computed the answer's 100 blocks |

The fetched run starts at the first block prefill lacked, which is the first block of the
answer. Latency is not a clean comparison on this testbed: turn 2's end-to-end time also
includes decode's NIXL pull over TCP and its own prefix-cache hits; the counters are the
evidence.

## Milestone 3: design -- the router names decode by itself

**Status**: built and verified (2026-10-26); see *Milestone 3, as built* below.

### Goal

The router names, as the KV-cache source of a turn, the pod whose certus holds the longest
prefix of the turn's prompt -- in particular the decode pod that generated the previous
turn's answer -- with no header set by hand. Measured by: on a multi-turn workload through
the router, turn N >= 2 prefill fetches the previous answer's blocks
(`lookup_hits_fetched` on the prefill node), with no client hint.

### Why the approximate producer cannot

It hashes the prompts the router routes and records each against the pods it routed them to.
A turn's answer is generated on decode and never passes through the router as a prompt, so
the producer never learns that decode holds it. Only the engines know what they cached; they
say so in **KV events**.

### What already exists

- **vLLM KV events.** `--kv-events-config '{"enable_kv_cache_events":true,"publisher":"zmq",
  "endpoint":...,"topic":"kv@<pod-ip>:<port>@<model>"}'` publishes `BlockStored` /
  `BlockRemoved` per block, with `token_ids`, `parent_block_hash` and a `medium`. llm-d's
  `p2p-kv-cache-sharing` guide runs exactly this.
- **Events from an offloading connector.** vLLM's `OffloadingEventsTracker`
  (`offloading/events.py`) turns a connector's raw `OffloadingEvent`s (keys only) into
  self-describing `BlockStored`s -- it snapshots each chunk's token ids and parent hash at
  store time, while the request is alive -- and fans removals out to the same block hashes.
  Opt-in: `kv_connector_extra_config["self_describing_kv_events"]: true`, and inert unless KV
  events are enabled.
- **The certus connector already emits removals** (`manager.take_events`), with medium
  `STORAGE` (`compat.offload_event_medium`; vLLM's tracker accepts only `CPU` and `STORAGE`),
  gated on KV events being enabled. It does **not** emit stores.
- **The precise producer** (`precise-prefix-cache-producer`) subscribes to every pod's
  events (`kvEventsConfig.discoverPods`), re-hashes block keys from the event token ids
  (so the engine's hash seed need not match), and indexes each block per pod per **device
  tier** -- the lower-cased event medium (`gpu`, `cpu`, `storage`, ...).
- **Tokenization in the EPP.** The precise producer hashes the request's tokens, so the EPP
  needs a `token-producer`. llm-d's guide uses a small CPU-only vLLM `render` service for
  exact tokenization; the approximate setup today uses the tokenizer-free `estimate`
  backend.

### What is missing

1. **Certus store events.** On `complete_store(success=True)`, emit
   `OffloadingEvent(keys=<logical keys>, medium=STORAGE, removed=False)` once per logical
   block (under TP=W, after all W shards: the commit is already per logical key), only when
   events are enabled. The tracker then publishes a `BlockStored` with the chunk's tokens.
   This is the connector half; small.

   A store's *eviction* is already published as `BlockRemoved` -- but only `REMOVED`, not
   `DEMOTED` (DRAM -> SSD), which keeps it readable. Correct for this purpose: the block is
   still fetchable from certus.

2. **The tier the p2p plugin counts.** With per-tier data, `p2p-source-producer` counts
   only the **`cpu`** tier (`producer.go`: `cpuDeviceTier = "cpu"`) -- the tier vLLM's own
   P2P can serve. Certus blocks arrive as `storage`; the plugin would see none. Options:

   | option | for | against |
   |---|---|---|
   | **a. report certus as `CPU`** (`offload_event_medium`) | no llm-d change; works today | false about the medium; collides with a real CPU tier beside certus (none here); and changes what *vLLM's own* KV metrics call certus |
   | **b. llm-d: a `pullableTiers` parameter** on `p2p-source-producer`, default `[cpu]` | honest; small and general (any tier a deployment's pull path can serve) | an upstream change, and a wait |
   | **c. a certus-local fork of the plugin** | no wait | a fork to carry |

   **Recommendation: (b), with (a) as the bridge.** Propose `pullableTiers` upstream; until
   it lands, a per-deployment switch in the certus connector, `CERTUS_EVENT_MEDIUM=cpu`
   (default `storage`), so the testbed can run M3 now and drop the switch later. On this
   testbed no real CPU tier sits beside certus, so (a)'s collision cannot happen here.

3. **GPU vs certus.** The engines also publish their **GPU** prefix cache (`gpu` tier). The
   plugin uses `cpu` only for the *source* choice, so GPU-only blocks never name a source --
   correct, since a peer cannot fetch from another pod's GPU. The `prefix-cache-scorer` and
   affinity filter may use all tiers for *placement*; their weights (`kvCacheBackendConfigs`)
   decide how much a certus block counts there. Start with llm-d's guide's weights (gpu 1.0,
   cpu 0.4).

4. **Decode as a source under P/D.** The producer's index is per pod; decode pods publish
   events like any other, so a decode pod that stored the answer is indexed as holding it.
   `p2p-source-producer` considers every endpoint as a source (its `Produce` iterates all of
   them), and compares against the **prefill** pod (`prefillProfileName: prefill`). So decode
   is named when its certus holds >= `minCachedTokenDelta` more of the prompt than prefill's
   -- which is the previous answer. No change needed; to be confirmed in the test.

5. **Token-exact turns.** As above: completions-API traffic (or a chat template that keeps
   earlier answers verbatim) matches; a template that strips `<think>` does not, past the
   answer's start. A workload property, not a design choice; the workload generator's
   multi-turn mode sends token-exact turns.

### Changes

| where | change |
|---|---|
| connector `manager.py` | store events on successful commit (gated; TP-aware) |
| connector `compat.py` | `CERTUS_EVENT_MEDIUM` (`storage` default, `cpu` for the bridge) |
| certus entrypoint | `--kv-events-config` (zmq publisher, the topic llm-d expects: `kv@$(POD_IP):<port>@<model>`), `self_describing_kv_events: true` in the certus connector's extra config |
| pd-certus patch | `POD_IP`, the events endpoint, `CERTUS_EVENT_MEDIUM=cpu` (bridge) on both roles |
| pd-certus router values | `token-producer` (the `render` service) + `endpoint-notification-source` + `precise-prefix-cache-producer` (`discoverPods`, socket port), and `p2p-source-producer` pointed at it; `dataLayer` wiring as in llm-d's guide |
| pd-certus resources | the `render` Deployment + Service (CPU-only vLLM, 4 CPU / 12 GiB as in the guide), on a non-GPU node |
| llm-d (upstream, later) | `pullableTiers` on `p2p-source-producer` |

### Risks

- **Event volume.** Every stored block is an event, on every pod. llm-d's guide notes events
  arrive "at many times the inference request rate"; the precise producer is built for it,
  but the EPP's CPU and memory should be watched (single EPP replica here).
- **A stale index.** Events are lossy under load (`take_events` already warns when the
  server drops some). A missed removal names a source whose certus has evicted the block: a
  fetch that misses and is computed, bounded by the budget and counted (`fetch_misses`). A
  missed store just loses a fetch opportunity. Neither is a correctness problem.
- **Tokenizer drift.** The `render` service must run the same model's tokenizer as the
  engines, or the EPP's block hashes match nothing. Pin it to the same model id.
- **The bridge's lie.** With `CERTUS_EVENT_MEDIUM=cpu`, vLLM's own metrics and anything else
  reading the events call certus "CPU". Confined to the testbed by the switch; removed once
  `pullableTiers` exists.

### Verification

1. Events: on a decode pod, after a turn, the EPP's index holds its stored blocks (EPP debug
   log, or a router request whose `p2p decision` shows `bestCachedTokens` > prefill's).
2. End to end: a two-turn session through the router (completions API, turn 2 = turn 1 +
   answer + question): the EPP logs `outcome=pulled` naming the decode pod; node5's certus
   counts the answer's blocks in `lookup_hits_fetched`; no header from the client.
3. Control: the same with `self_describing_kv_events` off -- `below-floor`, no fetch.

## Milestone 3, as built

As designed above, with the bridge (`CERTUS_EVENT_MEDIUM=cpu`), and two findings:

- **The router's block size must not exceed the size of certus's store events.** llm-d's
  guide sets `tokenProcessorConfig.blockSize: 64` and calls it independent of the engines',
  but the index drops a stored event with fewer tokens than that
  (`incomplete_canonical_block`, `pkg/kvevents/pool.go`), and certus's stores arrive one
  16-token block per event. At 64, every certus store was dropped and the router saw only the
  engines' GPU cache. Set to 16, the engines' block size.
- **The render service needs no image of its own**: `vllm launch render` from the vLLM image
  runs without a GPU given `VLLM_TARGET_DEVICE=cpu`.

Verified through the router, node2/node5, connector `vllm0.30.0-f47230bd`, no header from
the client. Turn 1: 3 081-token prompt, 1 600-token answer (100 blocks) on decode. Turn 2:
prompt + answer token ids + a question.

| decode's KV events | router's decision (turn 2) | node5's certus |
|---|---|---|
| on | `pulled`: source the decode pod, it holds 4 672 tokens vs prefill's 3 072, delta 1 600 | 1 FETCH, **100/100 fetched and served as fetched** |
| off (control) | `self`: only prefill's 3 072 known, delta 0 | no fetch |

## Milestones

1. Connector accepts `remote_kv_source` with the resolver, `certus_fetch_from`
   overriding; entrypoint annotation + Role. Verified by setting the header by hand at
   the decode sidecar (`curl -H 'x-llm-d-kv-cache-source-host-port: <pod-ip>:<port>'`
   to its :8000) with `--enable-p2p-pull` on, so selection is not yet in the loop.
2. `p2p-source-producer` in the EPP, on the approximate producer. Verified end to end: a
   turn whose prefix another pod served is fetched, counted by `lookup_hits_fetched` on
   the computing node, with no hint from the client.
3. Measure what the approximate view misses; only then store events + the precise
   producer + `pullableTiers`.
4. Aggregated: a sidecar in front of aggregated pods, if session moves prove frequent
   enough to be worth a proxy hop on every request.
