#!/usr/bin/env python3
"""Model KV-cache reuse under a tiered (HBM -> DRAM -> SSD), block-level LRU cache
driven by *sampled* turn-interval and turn-count distributions.

One **shared** three-tier hierarchy holds fixed-size KV blocks: HBM
(``--hbm-gb``, default 4 GiB), DRAM (``--dram-gb``, default 32 GiB) and SSD
(``--ssd-gb``, default unbounded storage). Tiers are exclusive: a full tier evicts
its **least-recently-used** blocks by **demoting** them to the next tier down
(blocks evicted from the bottom tier are lost), and a turn **promotes** the
lower-tier blocks it reuses back into HBM. A size of 0 disables DRAM or SSD;
``--dram-gb 0 --ssd-gb 0`` is a single HBM-only LRU cache. A closed-loop population of
``--concurrent-sessions`` conversations runs over a single wall-clock timeline;
when one finishes, a fresh one starts, holding concurrency constant until
``--sessions`` total conversations complete. Two trace-derived distributions
shape each conversation:

  * **Turn interval** (required positional arg): the think-time gap between a
    session's turns, from ``extract_interval_distribution.py``. LRU recency is
    wall-clock, so a brief pause finds blocks still in HBM while a long tail
    pause returns to find foreign churn demoted them to DRAM/SSD (promote) or
    dropped them (miss -> prefill).
  * **Turn count** (``--turn-distribution``, optional): how many turns each
    conversation runs, from ``extract_turn_distribution.py``. Without it, every
    conversation runs a fixed ``--num-turns``.

Block model (prefix caching):
  * With ``--prefix-tokens`` set, every conversation opens with one common
    prefix (system prompt, tool definitions). Its full blocks are cached once
    and **shared** by all sessions; every turn touches them. Reuse follows the
    hash chain: a session can only reuse its private blocks if the whole shared
    prefix ahead of them is still resident. A partial trailing prefix block is
    private (only full blocks are shareable).
  * Beyond the shared prefix there is no cross-session sharing.
  * Context grows per turn; with ``--max-model-len`` set, the live context is a
    **rolling window** of the trailing ``max_model_len`` tokens — older blocks
    slide out, bounding a conversation's KV at the window (how the 131K replay
    keeps deep, million-token conversations servable). The shared prefix stays
    pinned at the head of the window; only the private context slides. Unset =>
    unbounded growth.
  * A turn touches every resident block of its session, so they share one
    recency (its last turn time). Eviction drops blocks of the globally
    least-recently-used session first; on return a session reuses whatever live
    blocks survived and recomputes the rest plus the new tokens.

Caveat: a true rolling window matches sliding-window attention; a full-attention
model (e.g. Qwen2.5) instead rejects/truncates over-window turns, so the window
here is best read as a per-conversation KV-footprint bound rather than that
model's exact reuse mechanics.

Reported: reuse rate across return turns (full/partial/prefix-only/cold mix),
shared-prefix reuse on first turns, recompute token volume, per-tier hit rates,
per-tier mean/peak block occupancy, promotion/demotion/drop counts, and mean
simulated turns/conv.

Distributions are produced in benchmarks/synthetic-gen-by-llm/:

    ./extract_interval_distribution.py <trace>.jsonl -o intervals.yaml
    ./extract_turn_distribution.py     <trace>.jsonl -o turns.yaml

Examples
--------
    # Fixed 5 turns, unbounded context, 4G HBM / 32G DRAM / unbounded SSD
    ./kv_cache_dynamics.py example-intervals.yaml

    # Single-tier 8 GiB HBM LRU cache (no offload)
    ./kv_cache_dynamics.py example-intervals.yaml --hbm-gb 8 --dram-gb 0 --ssd-gb 0

    # Sampled turn counts + 131K rolling window
    ./kv_cache_dynamics.py cc-trace-weka-062126-intervals.yaml \
        --turn-distribution cc-trace-weka-062126-turns.yaml --max-model-len 131072
"""

from __future__ import annotations

import argparse
import bisect
import heapq
import json
import math
import random
import sys

import yaml


def load_distribution(path):
    """Load an extract_interval_distribution.py YAML doc and validate it."""
    with open(path) as f:
        doc = yaml.safe_load(f)
    if not isinstance(doc, dict) or "buckets" not in doc:
        raise ValueError(
            f"{path}: not an interval-distribution YAML (no 'buckets' key)")
    buckets = doc["buckets"]
    if not buckets:
        raise ValueError(f"{path}: distribution has no buckets")
    return doc


def build_sampler(buckets, rng):
    """Return a zero-arg callable that draws one interval (seconds).

    Buckets are selected by cumulative ``fraction`` (or ``count`` as a fallback);
    the value within a bucket is drawn log-uniformly, or linear-uniformly when
    the lower edge is non-positive.
    """
    weights = []
    for b in buckets:
        w = b.get("fraction")
        if w is None:
            w = b.get("count", 0)
        weights.append(float(w))

    total = sum(weights)
    if total <= 0:
        raise ValueError("distribution has zero total weight (all buckets empty)")

    # Precompute the cumulative distribution for O(log n) sampling via bisect.
    cum = []
    running = 0.0
    for w in weights:
        running += w / total
        cum.append(running)
    cum[-1] = 1.0  # guard against float drift so bisect never falls off the end

    def _edge(b, *names):
        for name in names:
            if name in b:
                return float(b[name])
        raise ValueError(f"bucket missing edge key (looked for {names})")

    # Interval YAML uses lower_s/upper_s; turn YAML uses lower/upper.
    lowers = [_edge(b, "lower_s", "lower") for b in buckets]
    uppers = [_edge(b, "upper_s", "upper") for b in buckets]

    def sample():
        i = bisect.bisect_left(cum, rng.random())
        if i >= len(cum):  # pragma: no cover - defensive
            i = len(cum) - 1
        lo, hi = lowers[i], uppers[i]
        if hi <= lo:
            return lo
        u = rng.random()
        if lo > 0:
            return 10 ** (math.log10(lo) + u * (math.log10(hi) - math.log10(lo)))
        return lo + u * (hi - lo)

    return sample


def percentile(sorted_vals, q):
    """Linear-interpolated percentile q in [0,100] over a sorted list."""
    if not sorted_vals:
        return float("nan")
    if len(sorted_vals) == 1:
        return sorted_vals[0]
    pos = (q / 100.0) * (len(sorted_vals) - 1)
    lo = math.floor(pos)
    hi = math.ceil(pos)
    if lo == hi:
        return sorted_vals[lo]
    frac = pos - lo
    return sorted_vals[lo] * (1 - frac) + sorted_vals[hi] * frac


def fmt_bytes(n):
    """Human-readable MB/GB."""
    mb = n / (1024 ** 2)
    if mb < 1024:
        return f"{mb:.2f} MB"
    return f"{mb / 1024:.2f} GB"


PREFIX_SID = -1  # cache key for the shared prefix (session ids are >= 0)


class _Tier:
    """One capacity-bounded tier: per-session block counts with LRU eviction.

    A session's blocks in a tier all carry one recency (its last access time).
    A lazy-deletion min-heap keyed on recency makes eviction amortized O(log n).
    ``capacity`` may be ``math.inf`` (never evicts).
    """

    def __init__(self, name, capacity):
        self.name = name
        self.capacity = capacity
        self.resident = {}            # sid -> blocks held in this tier
        self.recency = {}             # sid -> last access time (seconds)
        self.total = 0                # sum of resident blocks
        self._heap = []               # (recency, seq, sid) lazy LRU entries
        self._seq = 0

    def add(self, sid, blocks, rec):
        """Add ``blocks`` for ``sid`` with recency ``rec``."""
        if blocks <= 0:
            return
        self.resident[sid] = self.resident.get(sid, 0) + blocks
        self.total += blocks
        if self.recency.get(sid) != rec:
            self.recency[sid] = rec
            self._seq += 1
            heapq.heappush(self._heap, (rec, self._seq, sid))

    def take(self, sid, blocks):
        """Remove ``blocks`` of ``sid``'s blocks from this tier (recency kept)."""
        n = self.resident.get(sid, 0)
        k = min(n, blocks)
        if k == n:
            self.remove(sid)
        else:
            self.resident[sid] = n - k
            self.total -= k
        return k

    def remove(self, sid):
        """Drop all of ``sid``'s blocks from this tier; return how many."""
        n = self.resident.pop(sid, 0)
        self.recency.pop(sid, None)
        self.total -= n
        return n

    def evict_over(self):
        """Evict tail blocks (LRU session first) until within capacity.

        Returns ``[(sid, blocks, recency), ...]`` evicted, for demotion below.
        """
        out = []
        while self.total > self.capacity and self._heap:
            rec, _, sid = self._heap[0]
            # Skip stale entries (recency moved on, or session fully evicted).
            if self.recency.get(sid) != rec or self.resident.get(sid, 0) == 0:
                heapq.heappop(self._heap)
                continue
            n = min(self.resident[sid], self.total - self.capacity)
            self.resident[sid] -= n
            self.total -= n
            out.append((sid, n, rec))
            if self.resident[sid] == 0:
                heapq.heappop(self._heap)
                del self.resident[sid]
                del self.recency[sid]
            # else: capacity is met; the session keeps its leading blocks.
        return out


class TieredBlockCache:
    """Exclusive multi-tier KV-block hierarchy (e.g. HBM -> DRAM -> SSD).

    Each block lives in exactly one tier. Blocks evicted from a tier are
    **demoted** to the next one down; those evicted from the last tier are
    dropped. A session's blocks form one hash chain laid out contiguously as
    ``[top-tier head][next-tier middle]...[bottom-tier tail]`` (tail blocks are
    always the ones demoted), so per-session per-tier counts describe it fully.
    Accessing a session **promotes** its reused lower-tier blocks to the top.
    """

    def __init__(self, tiers):
        self.tiers = [_Tier(name, cap) for name, cap in tiers]
        self.promoted = [0] * len(self.tiers)   # blocks moved tier i -> top
        self.demoted = [0] * len(self.tiers)    # blocks moved tier i -> i+1
        self.dropped_blocks = 0                 # blocks lost off the bottom

    def chain(self, sid):
        """Per-tier block counts of ``sid``'s resident chain, top tier first."""
        return [t.resident.get(sid, 0) for t in self.tiers]

    def touch(self, sid, blocks, now, reused):
        """Make ``sid`` hold ``blocks`` live blocks in the top tier at ``now``.

        ``reused`` is the per-tier block count that this access read; those
        lower-tier blocks are promoted. Every other lower-tier block of ``sid``
        is outside the live window and is discarded.
        """
        for i, tier in enumerate(self.tiers[1:], start=1):
            tier.remove(sid)
            self.promoted[i] += reused[i]
        top = self.tiers[0]
        top.remove(sid)
        top.add(sid, blocks, now)

    def touch_head(self, sid, blocks, now):
        """Bring the first ``blocks`` of ``sid``'s chain into the top tier at ``now``.

        For a chain shared by several sessions (a share group): head blocks
        held lower down are promoted, missing ones are newly computed, and any
        blocks beyond ``blocks`` — needed by deeper peers — stay where they are.
        """
        top = self.tiers[0]
        have = top.resident.get(sid, 0)
        need = blocks - have
        for i, tier in enumerate(self.tiers[1:], start=1):
            if need <= 0:
                break
            k = tier.take(sid, need)
            self.promoted[i] += k
            need -= k
        top.remove(sid)
        top.add(sid, max(have, blocks), now)

    def enforce(self):
        """Cascade evictions down the hierarchy until every tier fits."""
        last = len(self.tiers) - 1
        for i, tier in enumerate(self.tiers):
            for sid, n, rec in tier.evict_over():
                if i < last:
                    self.tiers[i + 1].add(sid, n, rec)
                    self.demoted[i] += n
                else:
                    self.dropped_blocks += n


def simulate(
    interval_sample,
    *,
    total_sessions,
    concurrency,
    num_turns_fixed,
    turn_sample,
    turn_tokens,
    block_size,
    tiers,
    window_blocks,
    prefix_tokens=0,
    admission="interval",
    rng=None,
    shared_fraction=0.0,
    share_groups=1,
    decode_tokens=0,
    decode_step_s=0.03,
    decode_batch=1,
):
    """Closed-loop event simulation over a shared, tiered LRU block cache.

    ``tiers`` is ``[(name, capacity_blocks), ...]`` top (fastest) first; a
    capacity may be ``math.inf``.
    ``turn_sample`` (or None -> ``num_turns_fixed``) sets each conversation's turn
    count; ``window_blocks`` (or None -> unbounded) caps a conversation's live KV
    to a trailing rolling window. ``prefix_tokens`` opens every conversation
    with a common prefix whose full blocks are shared across sessions (pinned at
    the head of the window). ``admission`` picks whose turn runs next:
    ``interval`` (sampled think-time gaps on a wall-clock timeline),
    ``round-robin`` (cycle through the active sessions in slot order) or
    ``random`` (uniform over active sessions, drawn from ``rng``).
    ``shared_fraction`` makes the leading fraction of each conversation's live
    context (after the prefix) identical across the sessions of its share group
    (``share_groups`` groups, sessions dealt round-robin), cached once per group.
    ``decode_tokens`` > 0 splits each turn into a prefill (prior context +
    ``turn_tokens`` prompt) followed by that many one-token decode steps,
    ``decode_step_s`` apart; every step re-reads the session's whole live chain
    (interval admission only). ``decode_batch`` simulates that many steps per
    event, weighting their lookups accordingly, to bound event count.
    Returns aggregate reuse/occupancy/eviction stats; reuse stats exclude a
    warmup of one full concurrency wave.
    """
    cache = TieredBlockCache(tiers)
    ntiers = len(cache.tiers)
    warmup_sessions = concurrency

    def ceil_blocks(tokens):
        return (tokens + block_size - 1) // block_size

    # Shared prefix: full blocks live once in the cache under PREFIX_SID; the
    # partial remainder is private, added to each session's first turn.
    prefix_blocks = prefix_tokens // block_size
    prefix_rem_tokens = prefix_tokens - prefix_blocks * block_size
    priv_window = window_blocks - prefix_blocks if window_blocks else None

    def cap_live(blocks):
        return min(blocks, priv_window) if priv_window else blocks

    def shared_part(live):
        """Leading blocks of a ``live``-block context that are group-shared."""
        return int(shared_fraction * live)

    def group_sid(sid):
        # Share-group chains are keyed below PREFIX_SID: -2, -3, ...
        return -2 - (sid % share_groups)

    def take(chain, limit):
        """Per-tier blocks read walking ``chain`` top-down, up to ``limit``."""
        out = []
        for n in chain:
            k = min(n, limit)
            out.append(k)
            limit -= k
        return out

    # Per-session mutable state kept only while the session is active.
    accum_tokens = {}   # sid -> accumulated private context tokens so far
    turn_idx = {}       # sid -> next turn index to execute
    turns_total = {}    # sid -> this conversation's sampled turn count
    born = {}           # sid -> session ordinal (for warmup filtering)
    decode_left = {}    # sid -> decode steps remaining in the current turn

    started = 0

    # Scheduler: admit(sid, now) makes a session runnable, next_turn() returns
    # (now, sid) or None when drained, reschedule(sid, now) queues a session's
    # next turn, retire(sid) removes a finished one.
    if admission == "interval":
        # Event heap: (event_time, seq, sid). seq keeps ties deterministic.
        events = []
        seq = 0

        def admit(sid, now):
            nonlocal seq
            seq += 1
            heapq.heappush(events, (now, seq, sid))

        def next_turn():
            if not events:
                return None
            now, _, sid = heapq.heappop(events)
            return now, sid

        def reschedule(sid, now):
            # Next turn after a sampled think-time gap.
            admit(sid, now + interval_sample())

        def retire(sid):
            pass
    else:
        # Policy-ordered: one turn per step on a logical clock (LRU recency only
        # needs an order). A finished session's slot goes to its replacement, so
        # round-robin keeps a fixed cyclic order over slots.
        slots = []          # active sids in slot order
        free_slot = None    # slot vacated by the last retire, if not yet refilled
        rr_next = 0
        clock = 0

        def admit(sid, now):
            nonlocal free_slot
            if free_slot is not None:
                slots[free_slot] = sid
                free_slot = None
            else:
                slots.append(sid)

        def next_turn():
            nonlocal rr_next, clock, free_slot
            if free_slot is not None:   # vacated and never refilled: close it
                del slots[free_slot]
                if free_slot < rr_next:
                    rr_next -= 1
                free_slot = None
            if not slots:
                return None
            if admission == "round-robin":
                i = rr_next % len(slots)
                rr_next = i + 1
            else:
                i = rng.randrange(len(slots))
            clock += 1
            return float(clock), slots[i]

        def reschedule(sid, now):
            pass

        def retire(sid):
            nonlocal free_slot
            free_slot = slots.index(sid)

    def spawn(now):
        nonlocal started
        sid = started
        accum_tokens[sid] = 0
        turn_idx[sid] = 0
        turns_total[sid] = (max(1, round(turn_sample())) if turn_sample
                            else num_turns_fixed)
        born[sid] = started
        started += 1
        admit(sid, now)

    for _ in range(min(concurrency, total_sessions)):
        spawn(0.0)

    completed = 0
    measured_convs = 0
    # Reuse accounting over return turns (turn index > 0), in blocks * block_size.
    reused_tokens = 0
    reusable_tokens = 0   # in-window prior context that COULD be reused
    recompute_tokens = 0
    full_hits = partial_hits = prefix_only_hits = cold_misses = 0
    return_turns = 0
    # First turns (turn index 0) can reuse only the shared prefix.
    first_turns = 0
    first_reused_tokens = 0
    first_recompute_tokens = 0
    # Per-tier hits over all measured turns (first + return), in tokens.
    tier_hit_tokens = [0] * ntiers
    lookup_tokens = 0     # in-window prior context looked up (incl. shared prefix)
    # Share-group blocks looked up / found cached (measured turns), in tokens.
    shared_lookup_tokens = 0
    shared_hit_tokens = 0
    # Decode-stage accounting (measured sessions): each one-token step reads the
    # whole live chain; a block missing from every tier must be recomputed.
    decode_steps = 0
    decode_lookup_tokens = 0
    decode_tier_hit_tokens = [0] * ntiers
    decode_recompute_tokens = 0

    # Occupancy sampled per processed turn. (Time-weighting is avoided: the
    # closed-loop drain tail — a lone session drawing a huge tail-interval gap
    # after everything else has finished — would stretch wall-clock time over a
    # near-empty cache and wash the mean to zero.)
    occ_sum = [0.0] * ntiers
    occ_count = 0
    peak_blocks = [0] * ntiers

    while (nxt := next_turn()) is not None:
        now, sid = nxt

        t = turn_idx[sid]
        decoding = decode_left.get(sid, 0) > 0
        steps = min(decode_batch, decode_left[sid]) if decoding else 0
        prior_tokens = accum_tokens[sid]
        prior_blocks = ceil_blocks(prior_tokens)
        if decoding:
            added = steps        # one token per decode step
        else:
            added = turn_tokens + (prefix_rem_tokens if t == 0 else 0)
        new_tokens = prior_tokens + added
        new_blocks_total = ceil_blocks(new_tokens)

        # Rolling window: a conversation's live private KV is its trailing
        # priv_window blocks (the shared prefix stays pinned ahead of it).
        new_live = cap_live(new_blocks_total)
        fresh_blocks = min(new_blocks_total - prior_blocks, new_live)  # new this turn
        # Share group: the leading shared_new live blocks are the group's chain
        # (identical across members), looked up in full each turn — blocks this
        # session never computed still hit if a deeper peer cached them. The
        # rest of the live window is private.
        shared_prior = shared_part(cap_live(prior_blocks))
        shared_new = shared_part(new_live)
        fresh_priv = max(0, fresh_blocks - (shared_new - shared_prior))
        carried_priv = max(0, new_live - shared_new - fresh_priv)  # own prior context
        # Hash-chain reuse, read top tier first: shared-group blocks sit behind
        # the prefix, and private blocks are reachable only behind an intact
        # prefix and the session's own prior shared run (in whichever tiers).
        prefix_hit = take(cache.chain(PREFIX_SID), prefix_blocks)
        prefix_reused = sum(prefix_hit)
        gsid = group_sid(sid)
        if shared_new and prefix_reused == prefix_blocks:
            shared_hit = take(cache.chain(gsid), shared_new)
        else:
            shared_hit = [0] * ntiers
        shared_reused = sum(shared_hit)
        if prefix_reused == prefix_blocks and shared_reused >= shared_prior:
            priv_hit = take(cache.chain(sid), carried_priv)
        else:
            priv_hit = [0] * ntiers
        priv_reused = sum(priv_hit)
        carried = prefix_blocks + shared_new + carried_priv
        reused = prefix_reused + shared_reused + priv_reused
        fresh_blocks = fresh_priv   # shared fresh blocks are counted in `carried`

        measured = born[sid] >= warmup_sessions
        if decoding:
            if measured:
                # The first of this event's one-token steps finds what eviction
                # left since the last event; the rest are counted after the touch.
                decode_steps += steps
                decode_lookup_tokens += carried * block_size * steps
                for i in range(ntiers):
                    decode_tier_hit_tokens[i] += ((prefix_hit[i] + shared_hit[i]
                                                   + priv_hit[i]) * block_size)
                decode_recompute_tokens += (carried - reused) * block_size
        elif measured:
            lookup_tokens += carried * block_size
            shared_lookup_tokens += shared_new * block_size
            shared_hit_tokens += shared_reused * block_size
            for i in range(ntiers):
                tier_hit_tokens[i] += ((prefix_hit[i] + shared_hit[i] + priv_hit[i])
                                       * block_size)
        if decoding:
            pass
        elif t == 0 and measured:
            first_turns += 1
            first_reused_tokens += prefix_reused * block_size
            first_recompute_tokens += (carried - reused + fresh_blocks) * block_size
        elif measured:
            return_turns += 1
            reused_tokens += reused * block_size
            reusable_tokens += carried * block_size
            recompute_tokens += (carried - reused + fresh_blocks) * block_size
            if carried == 0 or reused == 0:
                cold_misses += 1
            elif reused >= carried:
                full_hits += 1
            elif priv_reused == 0 and carried_priv > 0:
                prefix_only_hits += 1
            else:
                partial_hits += 1

        # Prefill this turn: reused lower-tier blocks are promoted and the live
        # window is fully resident in the top tier, accessed at `now`. Touch the
        # shared prefix first so the session ties newer than it.
        accum_tokens[sid] = new_tokens
        if prefix_blocks:
            cache.touch(PREFIX_SID, prefix_blocks, now, prefix_hit)
        if shared_new:
            cache.touch_head(gsid, shared_new, now)
        cache.touch(sid, new_live - shared_new, now, priv_hit)
        cache.enforce()
        if decoding and measured and steps > 1:
            # Steps 2..K of a batched event re-read the chain as eviction just
            # left it: a context larger than HBM keeps spilling its tail, so
            # every step pays the lower-tier reads again.
            post = [a + b + c for a, b, c in zip(
                take(cache.chain(PREFIX_SID), prefix_blocks),
                take(cache.chain(gsid), shared_new),
                take(cache.chain(sid), carried - prefix_blocks - shared_new))]
            for i in range(ntiers):
                decode_tier_hit_tokens[i] += post[i] * block_size * (steps - 1)
            decode_recompute_tokens += ((carried - sum(post)) * block_size
                                        * (steps - 1))
        for i, tier in enumerate(cache.tiers):
            peak_blocks[i] = max(peak_blocks[i], tier.total)
        if measured:
            for i, tier in enumerate(cache.tiers):
                occ_sum[i] += tier.total
            occ_count += 1

        # Decode: queue the next step (or the turn's first) after one step time;
        # the turn ends, and think-time starts, when the last step is done.
        if decoding:
            decode_left[sid] -= steps
        elif decode_tokens:
            decode_left[sid] = decode_tokens
        if decode_left.get(sid, 0) > 0:
            admit(sid, now + decode_step_s * min(decode_batch, decode_left[sid]))
            continue

        turn_idx[sid] = t + 1
        if turn_idx[sid] >= turns_total[sid]:
            # Conversation done; its blocks linger until LRU reclaims them.
            completed += 1
            if measured:
                measured_convs += 1
            del accum_tokens[sid]
            del turn_idx[sid]
            del turns_total[sid]
            del born[sid]
            decode_left.pop(sid, None)
            retire(sid)
            if started < total_sessions:
                spawn(now)
        else:
            reschedule(sid, now)

    reuse_rate = (reused_tokens / reusable_tokens) if reusable_tokens else 0.0
    mean_turns = (return_turns / measured_convs + 1) if measured_convs else 0.0
    return {
        "completed": completed,
        "measured_convs": measured_convs,
        "mean_turns": mean_turns,
        "return_turns": return_turns,
        "reuse_rate": reuse_rate,
        "reused_tokens": reused_tokens,
        "reusable_tokens": reusable_tokens,
        "recompute_tokens": recompute_tokens,
        "full_hits": full_hits,
        "partial_hits": partial_hits,
        "prefix_only_hits": prefix_only_hits,
        "first_turns": first_turns,
        "first_reused_tokens": first_reused_tokens,
        "first_recompute_tokens": first_recompute_tokens,
        "prefix_blocks": prefix_blocks,
        "shared_lookup_tokens": shared_lookup_tokens,
        "shared_hit_tokens": shared_hit_tokens,
        "cold_misses": cold_misses,
        "tier_hit_tokens": tier_hit_tokens,
        "lookup_tokens": lookup_tokens,
        "promoted": cache.promoted,
        "demoted": cache.demoted,
        "dropped_blocks": cache.dropped_blocks,
        "decode_steps": decode_steps,
        "decode_lookup_tokens": decode_lookup_tokens,
        "decode_tier_hit_tokens": decode_tier_hit_tokens,
        "decode_recompute_tokens": decode_recompute_tokens,
        "mean_blocks": [(o / occ_count) if occ_count else 0.0 for o in occ_sum],
        "peak_blocks": peak_blocks,
    }


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("distribution",
                    help="interval-distribution YAML from extract_interval_distribution.py")
    ap.add_argument("--turn-distribution", default=None,
                    help="optional turn-count YAML from extract_turn_distribution.py; "
                         "sample a per-conversation turn count instead of --num-turns")

    # Model configuration (defaults: Llama-3-8B class).
    ap.add_argument("--num-layers", type=int, default=32)
    ap.add_argument("--num-kv-heads", type=int, default=8)
    ap.add_argument("--head-dim", type=int, default=128)
    ap.add_argument("--bytes-per-element", type=int, default=2,
                    help="2 for BF16/FP16, 1 for FP8 (default: 2)")
    ap.add_argument("--block-size", type=int, default=64,
                    help="KV cache block size in tokens (default: 64)")

    # Workload configuration.
    ap.add_argument("--avg-prompt-tokens", type=int, default=512,
                    help="new prompt tokens added per turn (default: 512)")
    ap.add_argument("--avg-gen-tokens", type=int, default=256,
                    help="generated response tokens per turn (default: 256)")
    ap.add_argument("--decode", action="store_true",
                    help="emulate decode: each turn prefills prior context + the "
                         "prompt, then generates --avg-gen-tokens one token per step, "
                         "each step re-reading the whole live context (interval "
                         "admission only)")
    ap.add_argument("--decode-step-s", type=float, default=0.03,
                    help="seconds per decode step (time per output token) with "
                         "--decode (default: 0.03)")
    ap.add_argument("--decode-batch", type=int, default=1,
                    help="decode steps simulated per event with --decode; >1 trades "
                         "recency resolution for speed, lookups still counted per "
                         "step (default: 1 = exact)")
    ap.add_argument("--num-turns", type=int, default=5,
                    help="turns per conversation when --turn-distribution is unset (default: 5)")
    ap.add_argument("--prefix-tokens", type=int, default=0,
                    help="tokens of a common prefix (system prompt, tools) every "
                         "conversation opens with; its full blocks are shared across "
                         "sessions in the cache. 0 = no shared prefix (default: 0)")
    ap.add_argument("--shared-fraction", type=float, default=0.0,
                    help="fraction (0-1) of each conversation's live context, after "
                         "the prefix, that is identical across its share group (e.g. "
                         "agents reading the same repository); cached once per group. "
                         "0 = fully private (default: 0)")
    ap.add_argument("--share-groups", type=int, default=1,
                    help="number of share groups; sessions are dealt round-robin "
                         "into groups and share only within one (default: 1)")
    ap.add_argument("--max-model-len", type=int, default=0,
                    help="context window in tokens; beyond it a conversation's live KV "
                         "becomes a trailing rolling window. 0 = unbounded (default: 0)")

    # Cache tiers: HBM -> DRAM -> SSD, exclusive, demote-on-evict.
    ap.add_argument("--hbm-gb", "--cache-capacity-gb", dest="hbm_gb", type=float,
                    default=4.0,
                    help="HBM (top-tier) KV cache capacity in GiB (default: 4)")
    ap.add_argument("--dram-gb", type=float, default=32.0,
                    help="DRAM tier capacity in GiB; 0 disables the tier (default: 32)")
    ap.add_argument("--ssd-gb", type=float, default=math.inf,
                    help="SSD tier capacity in GiB; 'inf' = unbounded storage, "
                         "0 disables the tier (default: inf)")

    # Load configuration.
    ap.add_argument("--admission", choices=("interval", "random", "round-robin"),
                    default="interval",
                    help="which active session runs its next turn: 'interval' = "
                         "sampled think-time gaps decide (wall-clock timeline); "
                         "'round-robin' = cycle through active sessions in fixed "
                         "order (LRU worst case); 'random' = uniform pick "
                         "(default: interval)")
    ap.add_argument("--concurrent-sessions", type=int, default=32,
                    help="conversations active at once, held constant (default: 32)")
    ap.add_argument("--sessions", type=int, default=2000,
                    help="total conversations to run through the cache (default: 2000)")
    ap.add_argument("--seed", type=int, default=None,
                    help="RNG seed for reproducible sampling (default: nondeterministic)")
    ap.add_argument("--json", default=None, metavar="PATH",
                    help="also write the configuration and raw stats as JSON to PATH")

    args = ap.parse_args()

    for name, val in (("--num-turns", args.num_turns),
                      ("--sessions", args.sessions),
                      ("--concurrent-sessions", args.concurrent_sessions),
                      ("--block-size", args.block_size)):
        if val < 1:
            ap.error(f"{name} must be >= 1")
    if not 0 < args.hbm_gb < math.inf:
        ap.error("--hbm-gb must be > 0 and finite")
    for name, val in (("--dram-gb", args.dram_gb), ("--ssd-gb", args.ssd_gb)):
        if not val >= 0:
            ap.error(f"{name} must be >= 0")
    if args.max_model_len < 0:
        ap.error("--max-model-len must be >= 0")
    if args.prefix_tokens < 0:
        ap.error("--prefix-tokens must be >= 0")
    if not 0.0 <= args.shared_fraction <= 1.0:
        ap.error("--shared-fraction must be in [0, 1]")
    if args.share_groups < 1:
        ap.error("--share-groups must be >= 1")
    if args.decode:
        if args.admission != "interval":
            ap.error("--decode needs --admission interval (steps are timed events)")
        if args.decode_step_s <= 0:
            ap.error("--decode-step-s must be > 0")
        if args.decode_batch < 1:
            ap.error("--decode-batch must be >= 1")
    if args.max_model_len and args.prefix_tokens >= args.max_model_len:
        ap.error("--prefix-tokens must be < --max-model-len")

    rng = random.Random(args.seed)
    try:
        doc = load_distribution(args.distribution)
        interval_sample = build_sampler(doc["buckets"], rng)
        turn_doc = None
        turn_sample = None
        if args.turn_distribution:
            turn_doc = load_distribution(args.turn_distribution)
            turn_sample = build_sampler(turn_doc["buckets"], rng)
    except (OSError, ValueError, yaml.YAMLError) as e:
        print(f"error: {e}", file=sys.stderr)
        return 1

    bytes_per_token = (2 * args.num_layers * args.num_kv_heads
                       * args.head_dim * args.bytes_per_element)
    bytes_per_block = args.block_size * bytes_per_token

    def gb_to_blocks(gb):
        if gb == math.inf:
            return math.inf
        return max(1, int(gb * (1024 ** 3) // bytes_per_block))

    # Enabled tiers, top first: (name, GiB, capacity blocks).
    tier_cfg = [("HBM", args.hbm_gb, gb_to_blocks(args.hbm_gb))]
    for name, gb in (("DRAM", args.dram_gb), ("SSD", args.ssd_gb)):
        if gb > 0:
            tier_cfg.append((name, gb, gb_to_blocks(gb)))
    capacity_blocks = tier_cfg[0][2]   # HBM
    turn_tokens = args.avg_prompt_tokens + args.avg_gen_tokens
    window_blocks = ((args.max_model_len + args.block_size - 1) // args.block_size
                     if args.max_model_len else None)
    prefix_blocks = args.prefix_tokens // args.block_size
    priv_window = window_blocks - prefix_blocks if window_blocks else None

    # Per-conversation footprint cap for the oversubscription estimate: a full
    # conversation, but no larger than the rolling window if one is set.
    if turn_sample:
        turns_for_est = turn_doc.get("summary", {}).get("mean") or args.num_turns
    else:
        turns_for_est = args.num_turns
    # Private blocks only; the shared prefix counts once across all sessions.
    priv_tokens_est = (int(turns_for_est) * turn_tokens
                       + args.prefix_tokens - prefix_blocks * args.block_size)
    conv_blocks = (priv_tokens_est + args.block_size - 1) // args.block_size
    if priv_window:
        conv_blocks = min(conv_blocks, priv_window)

    print("--- MODEL / CACHE CONFIGURATION ---")
    print(f"KV footprint: {bytes_per_token / 1024:.2f} KB/token, "
          f"{fmt_bytes(bytes_per_block)}/block ({args.block_size} tokens)")
    print(f"Tokens added per turn: {turn_tokens} "
          f"({args.avg_prompt_tokens} prompt + {args.avg_gen_tokens} gen)")
    if args.decode:
        print(f"Decode: {args.avg_gen_tokens} one-token steps/turn at "
              f"{args.decode_step_s * 1000:g} ms/step "
              f"({args.avg_gen_tokens * args.decode_step_s:.1f} s/turn), each re-reading "
              f"the live context" + (f"; {args.decode_batch} steps/event"
                                     if args.decode_batch > 1 else ""))
    for name, gb, blocks in tier_cfg:
        if blocks == math.inf:
            print(f"{name + ' tier:':10s} unbounded")
        else:
            print(f"{name + ' tier:':10s} {gb:.2f} GiB = {blocks} blocks")
    disabled = [n for n in ("DRAM", "SSD") if n not in {t[0] for t in tier_cfg}]
    if disabled:
        print(f"Disabled tiers: {', '.join(disabled)}")
    if window_blocks:
        print(f"Context window: {args.max_model_len} tokens = {window_blocks} blocks "
              f"({fmt_bytes(window_blocks * bytes_per_block)} per-conversation KV cap)")
    else:
        print("Context window: unbounded (monotonic growth)")
    if args.prefix_tokens:
        print(f"Shared prefix: {args.prefix_tokens} tokens = {prefix_blocks} shared blocks "
              f"({fmt_bytes(prefix_blocks * bytes_per_block)}, held once)"
              + (f" + {args.prefix_tokens - prefix_blocks * args.block_size} private "
                 f"remainder tokens" if args.prefix_tokens % args.block_size else ""))
    else:
        print("Shared prefix: none")
    if turn_sample:
        ts = turn_doc.get("summary", {})
        print(f"Turns/conversation: sampled from {args.turn_distribution} "
              f"(median={ts.get('median')} mean={ts.get('mean')} max={ts.get('max')})")
    else:
        print(f"Turns/conversation: {args.num_turns} (fixed)")
    # Shared-group blocks count once per group (bounded by the group's members).
    shared_blocks = int(args.shared_fraction * conv_blocks)
    groups_live = min(args.share_groups, args.concurrent_sessions)
    working_blocks = (args.concurrent_sessions * (conv_blocks - shared_blocks)
                      + groups_live * shared_blocks + prefix_blocks)
    oversub = working_blocks / capacity_blocks
    prefix_note = (f" + {fmt_bytes(prefix_blocks * bytes_per_block)} shared prefix"
                   if prefix_blocks else "")
    if shared_blocks:
        print(f"Shared fraction: {args.shared_fraction:g} of each live context "
              f"({fmt_bytes(shared_blocks * bytes_per_block)} at full depth) shared "
              f"within {args.share_groups} group(s)")
    else:
        print("Shared fraction: 0 (private contexts)")
    share_note = (f" (of which {fmt_bytes(shared_blocks * bytes_per_block)} shared, "
                  f"held once per group)" if shared_blocks else "")
    print(f"Working set vs capacity: {args.concurrent_sessions} concurrent x "
          f"{fmt_bytes(conv_blocks * bytes_per_block)}{share_note}{prefix_note} = "
          f"{oversub:.2f}x HBM "
          f"({'oversubscribed' if oversub > 1 else 'fits'})\n")

    print("--- INTERVAL DISTRIBUTION ---")
    print(f"Source: {doc.get('source', args.distribution)}")
    summary = doc.get("summary", {})
    if summary:
        print(f"median={summary.get('median_s')}s  mean={summary.get('mean_s')}s  "
              f"p90={summary.get('p90_s')}s  p99={summary.get('p99_s')}s")
    if args.admission == "interval":
        print("Admission: interval — longer sampled gaps => more foreign churn "
              "during the pause => eviction.\n")
    else:
        print(f"Admission: {args.admission} — the policy orders turns; sampled "
              f"intervals are not used.\n")

    stats = simulate(
        interval_sample,
        total_sessions=args.sessions,
        concurrency=args.concurrent_sessions,
        num_turns_fixed=args.num_turns,
        turn_sample=turn_sample,
        # With --decode the prefill adds only the prompt; generation is stepped.
        turn_tokens=args.avg_prompt_tokens if args.decode else turn_tokens,
        block_size=args.block_size,
        tiers=[(name, blocks) for name, _, blocks in tier_cfg],
        window_blocks=window_blocks,
        prefix_tokens=args.prefix_tokens,
        admission=args.admission,
        rng=rng,
        shared_fraction=args.shared_fraction,
        share_groups=args.share_groups,
        decode_tokens=args.avg_gen_tokens if args.decode else 0,
        decode_step_s=args.decode_step_s,
        decode_batch=args.decode_batch,
    )

    rt = stats["return_turns"]
    print("--- CACHE REUSE (LRU) ---")
    print(f"Conversations completed: {stats['completed']} "
          f"(warmup of {args.concurrent_sessions} excluded from reuse stats)")
    print(f"Mean turns/conversation (simulated): {stats['mean_turns']:.1f}")
    print(f"Return turns measured: {rt}")
    if rt:
        print(f"  full live-window hit : {stats['full_hits']} "
              f"({stats['full_hits'] / rt * 100:.1f}%)")
        print(f"  partial hit          : {stats['partial_hits']} "
              f"({stats['partial_hits'] / rt * 100:.1f}%)")
        if stats["prefix_blocks"]:
            print(f"  shared-prefix-only   : {stats['prefix_only_hits']} "
                  f"({stats['prefix_only_hits'] / rt * 100:.1f}%)")
        print(f"  cold miss            : {stats['cold_misses']} "
              f"({stats['cold_misses'] / rt * 100:.1f}%)")
    print(f"Token reuse rate (any tier): {stats['reuse_rate'] * 100:.1f}% "
          f"({stats['reused_tokens']:,} of {stats['reusable_tokens']:,} "
          f"in-window prior-context tokens)")
    print(f"Recompute (prefill) volume: {stats['recompute_tokens']:,} tokens "
          f"across measured return turns")
    ft = stats["first_turns"]
    if stats["prefix_blocks"] and ft:
        possible = ft * stats["prefix_blocks"] * args.block_size
        print(f"First turns measured: {ft}; shared-prefix reuse "
              f"{stats['first_reused_tokens'] / possible * 100:.1f}% "
              f"({stats['first_reused_tokens']:,} of {possible:,} tokens), "
              f"recompute {stats['first_recompute_tokens']:,} tokens")
    if stats["shared_lookup_tokens"]:
        sl = stats["shared_lookup_tokens"]
        print(f"Share-group blocks: {stats['shared_hit_tokens'] / sl * 100:.1f}% found cached "
              f"({stats['shared_hit_tokens']:,} of {sl:,} shared-context tokens looked up)")
    print()

    lookup = stats["lookup_tokens"]
    print("--- TIER HIT RATES (all measured turns) ---")
    print(f"Prior-context tokens looked up: {lookup:,} (incl. shared prefix)")
    print(f"{'tier':6s} {'hit tokens':>18s} {'of lookups':>11s} {'local hit':>10s}")
    remaining = lookup
    for (name, _, _), hits in zip(tier_cfg, stats["tier_hit_tokens"]):
        # "local hit" = fraction of lookups reaching this tier that it served.
        share = hits / lookup * 100 if lookup else 0.0
        local = hits / remaining * 100 if remaining else 0.0
        print(f"{name:6s} {hits:>18,} {share:>10.1f}% {local:>9.1f}%")
        remaining -= hits
    miss_share = remaining / lookup * 100 if lookup else 0.0
    print(f"{'miss':6s} {remaining:>18,} {miss_share:>10.1f}%   (recomputed)\n")

    dl = stats["decode_lookup_tokens"]
    if dl:
        print(f"--- DECODE-STAGE LOOKUPS ({stats['decode_steps']:,} measured steps) ---")
        print(f"Context tokens read by decode steps: {dl:,}")
        print(f"{'tier':6s} {'hit tokens':>18s} {'of reads':>11s}")
        for (name, _, _), hits in zip(tier_cfg, stats["decode_tier_hit_tokens"]):
            print(f"{name:6s} {hits:>18,} {hits / dl * 100:>10.3f}%")
        rc = stats["decode_recompute_tokens"]
        print(f"{'miss':6s} {rc:>18,} {rc / dl * 100:>10.3f}%   (evicted mid-decode, "
              f"recomputed)\n")

    print("--- TIER OCCUPANCY & MOVEMENT ---")
    print(f"{'tier':6s} {'mean blocks':>12s} {'mean size':>11s} {'% cap':>6s} "
          f"{'peak blocks':>12s} {'promoted->HBM':>14s} {'demoted down':>13s}")
    for i, (name, _, cap) in enumerate(tier_cfg):
        mean = stats["mean_blocks"][i]
        pct = f"{mean / cap * 100:5.0f}%" if cap != math.inf else "    -"
        promo = f"{stats['promoted'][i]:,}" if i else "-"
        demo = f"{stats['demoted'][i]:,}" if i < len(tier_cfg) - 1 else "-"
        print(f"{name:6s} {mean:>12.0f} {fmt_bytes(mean * bytes_per_block):>11s} {pct:>6s} "
              f"{stats['peak_blocks'][i]:>12,} {promo:>14s} {demo:>13s}")
    print(f"Promotions total: {sum(stats['promoted']):,} blocks; "
          f"demotions total: {sum(stats['demoted']):,} blocks")
    print(f"Blocks dropped off the bottom tier ({tier_cfg[-1][0]}): "
          f"{stats['dropped_blocks']:,}")

    if args.json:
        def finite(x):   # JSON has no infinity; unbounded tiers become null
            return None if x == math.inf else x
        out = {
            "args": {k: finite(v) for k, v in vars(args).items()},
            "bytes_per_token": bytes_per_token,
            "bytes_per_block": bytes_per_block,
            "turn_tokens": turn_tokens,
            "window_blocks": window_blocks,
            "prefix_blocks": prefix_blocks,
            "conv_blocks_est": conv_blocks,
            "oversubscription_vs_hbm": oversub,
            "working_blocks_est": working_blocks,
            "tiers": [{"name": n, "gb": finite(gb), "capacity_blocks": finite(b)}
                      for n, gb, b in tier_cfg],
            "stats": stats,
        }
        with open(args.json, "w") as f:
            json.dump(out, f, indent=2)
    return 0


if __name__ == "__main__":
    sys.exit(main())
