# Upstream proposal (draft): `pullableTiers` on llm-d's `p2p-source-producer`

**Status**: draft, not filed. **Date**: 2026-10-26. For `llm-d/llm-d-router`
(`pkg/epp/framework/plugins/requestcontrol/dataproducer/p2psource`), at `1a7e2ade`.

This is the text for an upstream issue, followed by the patch it proposes. Filing it is for
the maintainers of this repository to decide; nothing here has been sent.

---

## Issue

### Title

`p2p-source-producer`: make the pullable device tier(s) configurable (`pullableTiers`)

### Summary

`p2p-source-producer` selects a pull source by the contiguous cached prefix it holds in the
**`cpu`** device tier only (`producer.go`, `cpuDeviceTier = "cpu"`, in
`sourceCachedTokens`). That matches vLLM's own P2P secondary tier, which serves pulls from
the CPU offload tier. But the source header the plugin emits is consumed by whatever the
deployment's offloading connector does with `remote_kv_source`, and other connectors can
serve pulls from other tiers. For those, the hard-coded tier means a source is never chosen.

We propose a `pullableTiers` parameter: the device tiers whose blocks a pull can be served
from, default `["cpu"]`, so existing configurations behave exactly as today.

### Motivation

We run llm-d (P/D disaggregation, `MultiConnector[NixlConnector, OffloadingConnector]`) with
an out-of-tree offloading spec that stores KV in an external per-node cache service. Each
engine's offloading connector reports that service's blocks in vLLM KV events with medium
`STORAGE` (the only other value vLLM's offloading-events tracker accepts besides `CPU`). Its
connector resolves the sidecar's `remote_kv_source` to the source pod's cache service and
fetches the prefix from it over RDMA.

Everything else in the path works with the existing plugins unchanged: the
`precise-prefix-cache-producer` indexes the `storage` tier from the events, the routing
sidecar's `--enable-p2p-pull` injects `remote_kv_source`, and the connector acts on it. Only
the source choice ignores the tier the blocks are in. Today we work around it by reporting
the external cache as `CPU` in the events, which is false about the medium and would collide
with a real CPU tier configured beside it.

The case that matters most under P/D: a decode pod that generated a turn's answer stores it
in its cache; on the next turn the prompt contains that answer, and the router should name
the decode pod as the source so prefill fetches the answer instead of recomputing it. With
the workaround, on two A30 nodes, this works end to end: for a 1 600-token answer the EPP
logs `outcome=pulled` naming the decode pod (4 672 cached tokens vs prefill's 3 072) and
prefill fetches all 100 answer blocks; with the decode pod's events off, `outcome=self` and
no fetch.

### Proposal

- New optional parameter, `pullableTiers: [string]` (device-tier names, lower-cased like
  the index's tier names). Default `["cpu"]`.
- A source's pullable cached tokens become the longest contiguous chain among the listed
  tiers. Each per-tier count in `CachedBlocksByTier` is already a contiguous chain from the
  first block, so taking the maximum over the listed tiers is exact; summing would not be.
- The reusable-prefix floor (`ReusablePrefixTokens`) uses the same count, so floor and
  source choice keep agreeing.
- Validation: non-empty if set; reject `gpu` and `speculative` (a peer cannot pull from
  another pod's GPU, and speculative entries are routing guesses, not cached blocks).
- No change when the producer has no tier data: the existing "trusted as-is" fallback
  stays.

Mixed chains (blocks of one prefix split across two pullable tiers, e.g. the first half in
CPU and the rest on disk) are not counted as one chain; that is what the index reports today
and the same limitation the `cpu`-only rule has. A follow-up could count them if a connector
can serve a split chain.

### Alternatives considered

- **Report the external cache as `CPU`.** What we do now. It works, but it is false, it
  mislabels the cache in vLLM's own metrics, and it breaks if a real CPU tier sits beside it.
- **Count every non-GPU, non-speculative tier.** No configuration, but it would name sources
  whose tier the deployment's connector cannot pull from (a disk tier only the local engine
  reads, for example), turning a correct "no source" into a fetch that misses.
- **A separate plugin.** Duplicates the whole selection and cost model for one constant.

### Compatibility

Default `["cpu"]` reproduces current behaviour exactly; existing configurations and tests
are unaffected.

---

## Patch (proposed)

`upstream-pullable-tiers.patch` beside this file: a `git diff` against llm-d-router
`1a7e2ade`, +99/-13 in `p2psource/producer.go` and `producer_test.go`.

- `Config.PullableTiers []string` (`pullableTiers`), lower-cased; empty defaults to
  `["cpu"]`.
- `sourceCachedTokens` takes the longest per-tier chain among them (each count is already
  a contiguous chain; summing would count a gap).
- `PluginFactory` rejects `gpu`, `speculative` and the empty string.
- Three tests: a `storage` holder is a source only when `storage` is pullable (and is not
  by default: today's behaviour, pinned); two pullable tiers give the longer chain, not the
  sum; the factory rejects unpullable tiers. The existing tests are unchanged apart from the
  constant's new name.

Checked (2026-10-26, `golang:1.27` container): the package's tests pass, old and new; a
planted "sum the tiers" and a planted "allow gpu" each fail a new test.

README: one parameter line, and "Pulls are served from the source's CPU tier" becomes "from
the source's pullable tiers (`pullableTiers`, default CPU)" -- not in the patch, left to the
maintainers' wording.

---

## On our side, once it lands

- Drop the bridge: `CERTUS_EVENT_MEDIUM` back to its default (`storage`), remove it from the
  pd-certus patch, and set `pullableTiers: [storage]` on the p2p-source-producer in
  `pd-certus/router-values.yaml`; add `storage` to the precise producer's
  `kvCacheBackendConfigs` weights.
- Bump `deploy/llm-d/llm-d.ref` past the change and re-check `router-values.yaml` against
  upstream's `pd-config.yaml`.
