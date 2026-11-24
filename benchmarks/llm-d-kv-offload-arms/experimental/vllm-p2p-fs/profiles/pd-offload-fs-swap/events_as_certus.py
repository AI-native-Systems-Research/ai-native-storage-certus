"""EXPERIMENT ONLY (arm 5 + fs, not committed): make vLLM's offloading KV events
look to llm-d's router the way certus's do.

Certus reports its blocks as the CPU tier (CERTUS_EVENT_MEDIUM=cpu) and reports
no removal when it demotes a block to SSD, so the router's p2p-source-producer,
which counts only CPU-tier blocks, treats certus's SSD as pullable. vLLM's
TieringOffloadingSpec reports its filesystem tier as STORAGE and reports a CPU
eviction as removed, so under overflow the router sees a decode pod's answers
vanish and names no source, although the pod's P2P tier could serve them from
disk (the P2P producer looks up through every tier).

This wraps OffloadingEventsTracker.take_events, where every tier's events become
router events: removals are dropped, STORAGE is relabelled CPU. Applied by
importing this module before vLLM builds the connector (sitecustomize).
"""
from vllm.distributed.kv_transfer.kv_connector.v1.offloading import events as _ev
from vllm.v1.kv_offload.base import Medium, OffloadingEvent

_orig = _ev.OffloadingEventsTracker.take_events


def _take_events(self, events):
    kept = []
    for e in events:
        if e.removed:
            continue
        if e.medium is Medium.STORAGE:
            e = OffloadingEvent(keys=e.keys, medium=Medium.CPU, removed=False,
                                locality=e.locality, ownership=e.ownership,
                                removal_expected=e.removal_expected)
        kept.append(e)
    return _orig(self, kept)


_ev.OffloadingEventsTracker.take_events = _take_events
# stderr: this module loads in every Python process, including the ones the
# start command captures stdout from to build --kv-transfer-config.
import sys as _sys
print("[arm5-fs] KV events as certus: no removals, STORAGE reported as CPU", file=_sys.stderr, flush=True)
