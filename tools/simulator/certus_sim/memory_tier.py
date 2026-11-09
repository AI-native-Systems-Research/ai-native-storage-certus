"""Sharded LRU memory-tier model.

Models the DRAM cache pool with 16 shards (key % num_shards).
Each shard maintains an LRU ordered dict. Capacity-based eviction.
"""

from __future__ import annotations

from collections import OrderedDict
from dataclasses import dataclass
from typing import Optional


@dataclass
class MemoryTierEntry:
    key: int
    size: int
    shard: int
    insert_time: float  # simulation time of insertion


class MemoryTierShard:
    """Single shard with LRU ordering (MRU at tail)."""

    def __init__(self):
        self._entries: OrderedDict[int, MemoryTierEntry] = OrderedDict()
        self.used_bytes: int = 0

    def insert(self, entry: MemoryTierEntry) -> None:
        self._entries[entry.key] = entry
        self._entries.move_to_end(entry.key)
        self.used_bytes += entry.size

    def get(self, key: int) -> Optional[MemoryTierEntry]:
        if key in self._entries:
            self._entries.move_to_end(key)
            return self._entries[key]
        return None

    def peek(self, key: int) -> Optional[MemoryTierEntry]:
        return self._entries.get(key)

    def touch(self, key: int) -> bool:
        if key in self._entries:
            self._entries.move_to_end(key)
            return True
        return False

    def remove(self, key: int) -> Optional[MemoryTierEntry]:
        entry = self._entries.pop(key, None)
        if entry is not None:
            self.used_bytes -= entry.size
        return entry

    def evict_lru(self) -> Optional[MemoryTierEntry]:
        if not self._entries:
            return None
        key, entry = next(iter(self._entries.items()))
        del self._entries[key]
        self.used_bytes -= entry.size
        return entry

    def oldest_keys(self, n: int) -> list[int]:
        return list(self._entries.keys())[:n]

    def __len__(self) -> int:
        return len(self._entries)

    def __contains__(self, key: int) -> bool:
        return key in self._entries


class MemoryTier:
    """Sharded memory tier matching the spec's 16-shard design."""

    def __init__(self, capacity_bytes: int, num_shards: int = 16):
        self.capacity_bytes = capacity_bytes
        self.num_shards = num_shards
        self._shards = [MemoryTierShard() for _ in range(num_shards)]

    @property
    def used_bytes(self) -> int:
        return sum(s.used_bytes for s in self._shards)

    @property
    def free_bytes(self) -> int:
        return self.capacity_bytes - self.used_bytes

    def _shard_for(self, key: int) -> MemoryTierShard:
        return self._shards[key % self.num_shards]

    def insert(self, key: int, size: int, sim_time: float) -> bool:
        if self.used_bytes + size > self.capacity_bytes:
            return False
        shard = self._shard_for(key)
        entry = MemoryTierEntry(key=key, size=size, shard=key % self.num_shards,
                                insert_time=sim_time)
        shard.insert(entry)
        return True

    def get(self, key: int) -> Optional[MemoryTierEntry]:
        return self._shard_for(key).get(key)

    def peek(self, key: int) -> Optional[MemoryTierEntry]:
        return self._shard_for(key).peek(key)

    def touch(self, key: int) -> bool:
        return self._shard_for(key).touch(key)

    def remove(self, key: int) -> Optional[MemoryTierEntry]:
        return self._shard_for(key).remove(key)

    def contains(self, key: int) -> bool:
        return key in self._shard_for(key)

    def evict_lru_for_key(self, target_key: int) -> Optional[MemoryTierEntry]:
        """Evict LRU from the same shard as target_key (spec: shard-targeted eviction)."""
        shard = self._shard_for(target_key)
        return shard.evict_lru()

    def evict_lru(self) -> Optional[MemoryTierEntry]:
        """Evict globally oldest entry across all shards."""
        oldest_entry = None
        oldest_shard_idx = -1
        for i, shard in enumerate(self._shards):
            if not shard._entries:
                continue
            _, entry = next(iter(shard._entries.items()))
            if oldest_entry is None or entry.insert_time < oldest_entry.insert_time:
                oldest_entry = entry
                oldest_shard_idx = i
        if oldest_entry is not None:
            return self._shards[oldest_shard_idx].remove(oldest_entry.key)
        return None

    def oldest_keys(self, n: int) -> list[int]:
        """Return up to n keys in LRU order across all shards."""
        all_entries: list[tuple[float, int]] = []
        for shard in self._shards:
            for key in shard._entries:
                entry = shard._entries[key]
                all_entries.append((entry.insert_time, key))
        all_entries.sort()
        return [k for _, k in all_entries[:n]]

    def entry_count(self) -> int:
        return sum(len(s) for s in self._shards)
