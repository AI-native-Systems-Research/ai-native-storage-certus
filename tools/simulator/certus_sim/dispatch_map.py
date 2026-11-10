"""Dispatch map: tracks cache entry state and reference counts.

States per Rust implementation (components/dispatch-map/src/entry.rs):
  - Staging: data is in an in-memory DMA staging buffer (pending SSD write)
  - MemoryTier: data is in DRAM pool (has optional ssd_offset when write-through done)
  - BlockDevice: data is only on SSD (evicted from memory tier, or direct-write committed)
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum, auto
from typing import Optional


class EntryState(Enum):
    STAGING = auto()
    MEMORY_TIER = auto()
    BLOCK_DEVICE = auto()


class LookupResult(Enum):
    NOT_EXIST = auto()
    STAGING = auto()
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
    timestamp: float = 0.0


class DispatchMap:
    """In-memory key->state map with reference counting.

    Models the Rust DispatchMapComponent with blocking wait-for semantics
    (simplified for discrete event simulation — no real blocking needed).
    """

    def __init__(self):
        self._entries: dict[int, DispatchEntry] = {}

    def create_staging(self, key: int, size: int) -> bool:
        """Create a new staging entry (write_ref=1). Returns False if key exists."""
        if key in self._entries:
            return False
        self._entries[key] = DispatchEntry(
            key=key, size=size, state=EntryState.STAGING,
            write_ref=1, timestamp=0.0,
        )
        return True

    def create_memory_tier_entry(self, key: int, size: int, timestamp: float) -> bool:
        """Create a new memory-tier entry (write_ref=1)."""
        if key in self._entries:
            return False
        self._entries[key] = DispatchEntry(
            key=key, size=size, state=EntryState.MEMORY_TIER,
            write_ref=1, timestamp=timestamp,
        )
        return True

    def lookup(self, key: int) -> Optional[DispatchEntry]:
        return self._entries.get(key)

    def lookup_result(self, key: int) -> LookupResult:
        """Return the lookup result enum matching Rust's LookupResult."""
        entry = self._entries.get(key)
        if entry is None:
            return LookupResult.NOT_EXIST
        if entry.state == EntryState.STAGING:
            return LookupResult.STAGING
        if entry.state == EntryState.MEMORY_TIER:
            return LookupResult.MEMORY_TIER
        return LookupResult.BLOCK_DEVICE

    def exists(self, key: int) -> bool:
        return key in self._entries

    def touch(self, key: int, timestamp: float) -> bool:
        entry = self._entries.get(key)
        if entry is None:
            return False
        entry.timestamp = timestamp
        return True

    def convert_to_storage(self, key: int, drive_idx: int, offset: int) -> bool:
        """For Staging: transitions to BlockDevice. For MemoryTier: sets ssd_offset.
        Decrements read_ref (matches Rust behavior)."""
        entry = self._entries.get(key)
        if entry is None:
            return False
        if entry.state == EntryState.STAGING:
            entry.state = EntryState.BLOCK_DEVICE
            entry.ssd_offset = offset
            entry.drive_idx = drive_idx
        elif entry.state == EntryState.MEMORY_TIER:
            entry.ssd_offset = offset
            entry.drive_idx = drive_idx
        else:
            return False
        if entry.read_ref > 0:
            entry.read_ref -= 1
        return True

    def convert_memory_tier_to_block(self, key: int) -> bool:
        """Transitions MemoryTier -> BlockDevice using stored ssd_offset."""
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
        """Write ref -> read ref atomically."""
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

    def entries_in_state(self, state: EntryState) -> list[DispatchEntry]:
        """Return all entries in the given state."""
        return [e for e in self._entries.values() if e.state == state]
