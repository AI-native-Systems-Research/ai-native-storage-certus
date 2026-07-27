# SPDX-License-Identifier: Apache-2.0
"""OffloadingManager backed by a remote certus-server over gRPC.

All index/allocation/eviction state lives in the server. This class adapts
between vLLM's Python types and the server's u64-keyed batch RPCs, per the
mapping:

    lookup        -> Check
    prepare_store -> Check (filter) + Reserve (best-effort: store the subset
                     that fits, drop blocks the saturated tier can't reserve)
    complete_store-> CommitStore (success) / AbortStore (failure)
    prepare_load  -> Pin(promote=true)
    complete_load -> Unpin
    touch         -> Touch
    take_events   -> TakeEvents(max_events=0)
"""

from __future__ import annotations

import collections
import os
import sys
from collections.abc import Iterable

from vllm.v1.kv_offload.abstract import (
    LoadStoreSpec,
    OffloadingEvent,
    OffloadingManager,
    OffloadKey,
    PrepareStoreOutput,
)

from . import dispatcher_pb2 as pb
from .mediums import BlockLocation, CertusLoadStoreSpec


def _key_to_u64(key: OffloadKey) -> int:
    """Convert an OffloadKey (bytes) to a u64 for the server."""
    if isinstance(key, int):
        return key
    return int.from_bytes(key[:8], "big")


def _keys_to_u64s(keys: Iterable[OffloadKey]) -> list[int]:
    return [_key_to_u64(k) for k in keys]


class GrpcCertusOffloadingManager(OffloadingManager):
    """Manager delegating to a remote certus-server via a DispatcherStub."""

    def __init__(self, stub, block_size_bytes: int):
        self._stub = stub
        self._block_size_bytes = int(block_size_bytes)
        # Cumulative count of blocks we could not offload because the server's
        # memory tier was saturated (Reserve failed). Logged in throttled
        # summaries rather than per-request, so a persistently-full tier does
        # not produce a warning storm.
        self._store_dropped_blocks = 0
        self._store_drop_log_next = 1000

        # Distribution of the `keys` batch length, tracked separately for
        # prepare_store and prepare_load. Keyed by exact length so percentiles
        # are exact. The distribution is flushed (printed + reset) per benchmark
        # *conversation round*: this manager runs scheduler-side in vLLM's
        # EngineCore process, a different process from the benchmark driver that
        # loops llm.generate(), so the driver signals the current round by
        # writing an integer to the file named by CERTUS_ROUND_FILE (inherited
        # across the EngineCore spawn). When that value advances, we print the
        # round that just ended and start fresh. Without the env var the samples
        # simply accumulate and are printed once at shutdown().
        self._round_signal_path = os.environ.get("CERTUS_ROUND_FILE")
        self._current_round = 0
        self._key_len_counts = {
            "prepare_store": collections.Counter(),
            "prepare_load": collections.Counter(),
        }
        self._key_dist_total = {"prepare_store": 0, "prepare_load": 0}

    def set_block_size_bytes(self, block_size_bytes: int) -> None:
        """Update the per-block Reserve size once the true KV-cache tensor
        stride is known (the manager is constructed before get_handlers can
        resolve it). Reserve sizes are per-call, so changing this affects only
        subsequent stores."""
        self._block_size_bytes = int(block_size_bytes)

    # ── instrumentation ──

    def _read_round(self) -> int:
        """Read the current benchmark round from the signal file. Returns the
        last known round on any error (path unset, file missing, or an empty
        read caught mid-write by the driver)."""
        if not self._round_signal_path:
            return self._current_round
        try:
            with open(self._round_signal_path) as f:
                return int(f.read().strip() or self._current_round)
        except (OSError, ValueError):
            return self._current_round

    def _flush_distributions(self) -> None:
        """Print and reset both ops' accumulated distributions."""
        for op in ("prepare_store", "prepare_load"):
            self._print_key_distribution(op)
            self._key_len_counts[op].clear()
            self._key_dist_total[op] = 0

    def _maybe_roll_round(self) -> None:
        """If the driver has advanced to a new conversation round, print the
        distribution of the round that just ended and reset for the new one."""
        r = self._read_round()
        if r != self._current_round:
            self._flush_distributions()
            self._current_round = r

    def _record_key_count(self, op: str, n: int) -> None:
        """Record one observation of the `keys` batch length for ``op``
        (``"prepare_store"`` or ``"prepare_load"``), first flushing the prior
        round's distribution if the benchmark has moved on to a new round."""
        self._maybe_roll_round()
        self._key_len_counts[op][n] += 1
        self._key_dist_total[op] += 1

    def _print_key_distribution(self, op: str) -> None:
        """Print a histogram (power-of-two buckets) plus summary stats for the
        distribution of ``op``'s `keys` batch lengths in the current round."""
        counts = self._key_len_counts[op]
        total = self._key_dist_total[op]
        if not total:
            return
        rnd = self._current_round

        lengths = sorted(counts)
        min_len, max_len = lengths[0], lengths[-1]
        total_keys = sum(n * c for n, c in counts.items())
        mean = total_keys / total

        def percentile(p: float) -> int:
            target = p * total
            cum = 0
            for n in lengths:
                cum += counts[n]
                if cum >= target:
                    return n
            return max_len

        # Aggregate exact lengths into power-of-two buckets for display:
        # {0}, {1}, [2,3], [4,7], [8,15], ...
        buckets: dict = {}
        for n in lengths:
            if n == 0:
                lo, label = 0, "0"
            else:
                e = n.bit_length() - 1
                lo, hi = 1 << e, (1 << (e + 1)) - 1
                label = str(lo) if lo == hi else f"{lo}-{hi}"
            slot = buckets.setdefault(lo, [label, 0])
            slot[1] += counts[n]

        peak = max(c for _, c in buckets.values())
        bar_width = 40

        lines = [
            f"[certus-grpc] {op} keys-length distribution — round {rnd} "
            f"({total} calls)",
            f"  calls={total} keys_total={total_keys} min={min_len} "
            f"max={max_len} mean={mean:.1f} p50={percentile(0.50)} "
            f"p90={percentile(0.90)} p99={percentile(0.99)}",
        ]
        for lo in sorted(buckets):
            label, c = buckets[lo]
            bar = "#" * max(1, round(bar_width * c / peak))
            pct = 100.0 * c / total
            lines.append(f"  {label:>10} | {bar:<{bar_width}} {c:>10}  {pct:5.1f}%")
        print("\n".join(lines), flush=True)

    # ── lookup / touch ──

    def lookup(self, key: OffloadKey, req_context=None) -> bool | None:
        int_key = _key_to_u64(key)
        resp = self._stub.Check(pb.BatchCheckRequest(keys=[int_key]))
        return bool(resp.results and resp.results[0].exists)

    def touch(self, keys: Iterable[OffloadKey]) -> None:
        int_keys = _keys_to_u64s(keys)
        if int_keys:
            self._stub.Touch(pb.BatchTouchRequest(keys=int_keys, promote=False))

    # ── store ──

    def prepare_store(
        self, keys: Iterable[OffloadKey], req_context=None
    ) -> PrepareStoreOutput | None:
        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)
        self._record_key_count("prepare_store", len(keys_list))

        # Filter out keys already cached (consecutive dedup is vLLM's concern;
        # here we just avoid re-storing existing entries).
        check = self._stub.Check(pb.BatchCheckRequest(keys=int_keys))
        exists = {r.key: r.exists for r in check.results}
        to_store_pairs = [
            (orig, k)
            for orig, k in zip(keys_list, int_keys)
            if not exists.get(k, False)
        ]

        if not to_store_pairs:
            return PrepareStoreOutput(
                keys_to_store=[],
                store_spec=CertusLoadStoreSpec([]),
                evicted_keys=[],
            )

        to_store_ints = [k for _, k in to_store_pairs]

        # Reserve DRAM slots (server evicts LRU internally to make room). A
        # per-key failure means the server could not free enough space for that
        # block — the memory tier is saturated (e.g. capped at its max size with
        # a working set larger than it, or the evictable set is small because
        # entries are pinned by in-flight loads or not yet written through to
        # SSD). Reserve is per-key independent, so we treat this as best-effort:
        # keep the keys that reserved and drop the rest, rather than rejecting
        # the whole request. A whole-request reject (return None) does NOT
        # advance vLLM's stored-block index, so the same blocks are retried
        # every scheduler step, each logging "cannot store blocks" — a warning
        # storm under sustained pressure. Storing the subset that fits advances
        # the index and offloads what it can.
        reserve = self._stub.Reserve(
            pb.BatchReserveRequest(
                entries=[
                    pb.ReserveEntry(key=k, size=self._block_size_bytes)
                    for k in to_store_ints
                ]
            )
        )
        reserved_ok = {r.key for r in reserve.results if r.success}

        # Keep reserved keys in the original offload order (the scheduler
        # positionally zips src GPU block ids with dst keys, deriving src order
        # from offload_keys filtered by keys_to_store — so store_spec must stay
        # in offload order for a partial subset to line up).
        stored_pairs = [(orig, k) for orig, k in to_store_pairs if k in reserved_ok]
        dropped = len(to_store_pairs) - len(stored_pairs)
        if dropped:
            self._note_store_drops(dropped)

        # Keys that reserved but that we are not going to store this round would
        # leak a reservation; here every reserved key IS kept (we only drop keys
        # whose Reserve failed, which allocated nothing), so no rollback needed.

        if not stored_pairs:
            # Nothing fit. Return an empty (non-None) result so vLLM advances
            # past these tokens quietly instead of retrying + warning every step.
            return PrepareStoreOutput(
                keys_to_store=[],
                store_spec=CertusLoadStoreSpec([]),
                evicted_keys=[],
            )

        stored_orig = [orig for orig, _ in stored_pairs]
        stored_ints = [k for _, k in stored_pairs]
        locations = [BlockLocation(key=k) for k in stored_ints]
        return PrepareStoreOutput(
            keys_to_store=stored_orig,
            store_spec=CertusLoadStoreSpec(locations),
            # Evictions are surfaced asynchronously via take_events()/TakeEvents.
            evicted_keys=[],
        )

    def _note_store_drops(self, dropped: int) -> None:
        """Account for blocks skipped due to a saturated memory tier and emit a
        throttled summary (never per-request, to avoid a warning storm)."""
        self._store_dropped_blocks += dropped
        if self._store_dropped_blocks >= self._store_drop_log_next:
            print(
                f"[certus-grpc] memory tier saturated: skipped offloading "
                f"{self._store_dropped_blocks} blocks so far (best-effort store; "
                f"blocks stay in GPU). Consider a larger --memory-tier-size or "
                f"lower concurrency.",
                file=sys.stderr,
                flush=True,
            )
            # Back off the log cadence so a persistently-full tier stays quiet.
            self._store_drop_log_next = self._store_dropped_blocks * 2

    def complete_store(self, keys: Iterable[OffloadKey], success: bool = True) -> None:
        int_keys = _keys_to_u64s(keys)
        if not int_keys:
            return
        if success:
            self._stub.CommitStore(pb.BatchCommitStoreRequest(keys=int_keys))
        else:
            self._stub.AbortStore(pb.BatchAbortStoreRequest(keys=int_keys))

    # ── load ──

    def prepare_load(
        self, keys: Iterable[OffloadKey], req_context=None
    ) -> LoadStoreSpec:
        int_keys = _keys_to_u64s(keys)
        self._record_key_count("prepare_load", len(int_keys))
        # Pin (promote=FALSE) only takes the eviction-protecting read-ref. We must
        # NOT ask Pin to promote: Pin's promote is async/fire-and-forget, and the
        # Lookup that immediately follows (in the load handler) already promotes
        # cold (BlockDevice) entries itself. Two promotes race on the same key —
        # both do mt.insert() — and the loser hits MemoryTierError::AlreadyExists,
        # surfaced as ALLOCATION_FAILED, which fails the load and crashes vLLM
        # (worker asserts transfer success). Lookup is self-sufficient: it serves
        # MemoryTier hits directly and promotes BlockDevice misses in one path.
        resp = self._stub.Pin(pb.BatchPinRequest(keys=int_keys, promote=False))
        # Diagnostic: vLLM only reaches here for keys lookup()/Check reported as
        # present, and cannot drop keys from the returned spec (dst block ids are
        # positionally zipped). So a Pin failure here is the earliest signal that
        # a Check-hit entry vanished — log which key + why.
        for r in resp.results:
            if not r.success:
                print(
                    f"[certus-grpc] PIN FAILURE in prepare_load key={r.key} "
                    f"error_code={r.error_code} msg={r.error_message!r} "
                    f"(Check said present, Pin says gone — eviction race)",
                    flush=True,
                )
        return CertusLoadStoreSpec([BlockLocation(key=k) for k in int_keys])

    def complete_load(self, keys: Iterable[OffloadKey]) -> None:
        int_keys = _keys_to_u64s(keys)
        if int_keys:
            self._stub.Unpin(pb.BatchUnpinRequest(keys=int_keys))

    # ── events / shutdown ──

    def take_events(self) -> Iterable[OffloadingEvent]:
        resp = self._stub.TakeEvents(pb.TakeEventsRequest(max_events=0))
        if resp.dropped_count:
            print(
                f"[certus-grpc] WARNING: {resp.dropped_count} eviction events "
                "dropped by server (event view is lossy)",
                file=sys.stderr,
                flush=True,
            )
        # Only REMOVED means the key is no longer accessible. DEMOTED entries
        # stay on SSD and remain loadable, so they are not eviction events for
        # vLLM's accounting.
        removed = [
            e.key.to_bytes(8, "big")
            for e in resp.events
            if e.reason == pb.EVICTION_REASON_REMOVED
        ]
        if removed:
            yield OffloadingEvent(
                keys=removed,
                medium=CertusLoadStoreSpec.medium(),
                removed=True,
            )

    def shutdown(self) -> None:
        # Flush the final (in-progress) round, which no later round-advance will
        # trigger. Channel is owned by the spec singleton; nothing to close.
        self._flush_distributions()
