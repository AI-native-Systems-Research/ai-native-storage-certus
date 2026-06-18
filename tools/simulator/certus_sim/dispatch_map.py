"""Dispatch map: tracks cache entry state and reference counts.

States per spec:
  - MemoryTier: data is in DRAM pool (has optional ssd_offset when write-through done)
  - BlockDevice: data is only on SSD (evicted from memory tier)
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum, auto
from typing import Optional


class EntryState(Enum):
    MEMORY_TIER = auto()
    BLOCK_DEVICE = auto()


@dataclass
class DispatchEntry:
    key: int
    size: int
    state: EntryState
    ssd_offset: Optional[int] = None
    drive_idx: Optional[int] = None
    read_ref: int = 0
    write_ref: int = 0
    timestamp: float = 0.0  # last access time for LRU


class DispatchMap:
    """In-memory key→state map with reference counting."""

    def __init__(self):
        self._entries: dict[int, DispatchEntry] = {}

    def create_memory_tier_entry(self, key: int, size: int, timestamp: float) -> bool:
        if key in self._entries:
            return False
        self._entries[key] = DispatchEntry(
            key=key, size=size, state=EntryState.MEMORY_TIER,
            write_ref=1, timestamp=timestamp,
        )
        return True

    def lookup(self, key: int) -> Optional[DispatchEntry]:
        return self._entries.get(key)

    def exists(self, key: int) -> bool:
        return key in self._entries

    def touch(self, key: int, timestamp: float) -> bool:
        entry = self._entries.get(key)
        if entry is None:
            return False
        entry.timestamp = timestamp
        return True

    def convert_to_storage(self, key: int, drive_idx: int, offset: int) -> bool:
        """Sets ssd_offset on a MemoryTier entry (write-through complete)."""
        entry = self._entries.get(key)
        if entry is None:
            return False
        if entry.state != EntryState.MEMORY_TIER:
            return False
        entry.ssd_offset = offset
        entry.drive_idx = drive_idx
        if entry.read_ref > 0:
            entry.read_ref -= 1
        return True

    def convert_memory_tier_to_block(self, key: int) -> bool:
        """Transitions MemoryTier → BlockDevice using stored ssd_offset."""
        entry = self._entries.get(key)
        if entry is None:
            return False
        if entry.state != EntryState.MEMORY_TIER:
            return False
        if entry.ssd_offset is None:
            return False
        entry.state = EntryState.BLOCK_DEVICE
        return True

    def is_evictable(self, key: int) -> bool:
        """True if MemoryTier + ssd_offset set + no active refs."""
        entry = self._entries.get(key)
        if entry is None:
            return False
        return (
            entry.state == EntryState.MEMORY_TIER
            and entry.ssd_offset is not None
            and entry.read_ref == 0
            and entry.write_ref == 0
        )

    def remove(self, key: int) -> Optional[DispatchEntry]:
        return self._entries.pop(key, None)

    def take_read(self, key: int) -> bool:
        entry = self._entries.get(key)
        if entry is None:
            return False
        if entry.write_ref > 0:
            return False
        entry.read_ref += 1
        return True

    def release_read(self, key: int) -> bool:
        entry = self._entries.get(key)
        if entry is None or entry.read_ref == 0:
            return False
        entry.read_ref -= 1
        return True

    def take_write(self, key: int) -> bool:
        entry = self._entries.get(key)
        if entry is None:
            return False
        if entry.read_ref > 0 or entry.write_ref > 0:
            return False
        entry.write_ref += 1
        return True

    def release_write(self, key: int) -> bool:
        entry = self._entries.get(key)
        if entry is None or entry.write_ref == 0:
            return False
        entry.write_ref -= 1
        return True

    def downgrade_reference(self, key: int) -> bool:
        """Write ref → read ref atomically."""
        entry = self._entries.get(key)
        if entry is None or entry.write_ref == 0:
            return False
        entry.write_ref -= 1
        entry.read_ref += 1
        return True

    def oldest_keys(self, n: int) -> list[int]:
        """Return up to n keys sorted by ascending timestamp."""
        sorted_entries = sorted(self._entries.values(), key=lambda e: e.timestamp)
        return [e.key for e in sorted_entries[:n]]

    def entry_count(self) -> int:
        return len(self._entries)

    def entry_size(self, key: int) -> Optional[int]:
        entry = self._entries.get(key)
        return entry.size if entry else None
