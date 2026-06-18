"""Per-drive SSD storage model.

Models N independent NVMe drives with simple extent allocation.
Drive selection uses key % num_drives (matching spec).
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class Extent:
    key: int
    offset: int
    size: int
    drive_idx: int


class SsdDrive:
    """Single drive with bump-pointer allocation and free list."""

    def __init__(self, capacity_bytes: int, drive_idx: int):
        self.capacity_bytes = capacity_bytes
        self.drive_idx = drive_idx
        self.used_bytes: int = 0
        self._next_offset: int = 0
        self._extents: dict[int, Extent] = {}  # offset → Extent
        self._free_offsets: list[int] = []

    def allocate(self, key: int, size: int) -> int | None:
        if self.used_bytes + size > self.capacity_bytes:
            return None
        if self._free_offsets:
            offset = self._free_offsets.pop()
        else:
            offset = self._next_offset
            self._next_offset += size
        extent = Extent(key=key, offset=offset, size=size, drive_idx=self.drive_idx)
        self._extents[offset] = extent
        self.used_bytes += size
        return offset

    def free(self, offset: int) -> bool:
        extent = self._extents.pop(offset, None)
        if extent is None:
            return False
        self.used_bytes -= extent.size
        self._free_offsets.append(offset)
        return True

    def utilization(self) -> float:
        if self.capacity_bytes == 0:
            return 0.0
        return self.used_bytes / self.capacity_bytes

    def extent_count(self) -> int:
        return len(self._extents)


class SsdTier:
    """Multi-drive SSD tier with key-based sharding."""

    def __init__(self, num_drives: int, drive_capacity_bytes: int):
        self.num_drives = num_drives
        self.drives = [
            SsdDrive(drive_capacity_bytes, i) for i in range(num_drives)
        ]

    def drive_for_key(self, key: int) -> int:
        return key % self.num_drives

    def allocate(self, key: int, size: int) -> tuple[int, int] | None:
        """Returns (drive_idx, offset) or None if full."""
        drive_idx = self.drive_for_key(key)
        drive = self.drives[drive_idx]
        offset = drive.allocate(key, size)
        if offset is None:
            return None
        return (drive_idx, offset)

    def free(self, drive_idx: int, offset: int) -> bool:
        return self.drives[drive_idx].free(offset)

    def total_used_bytes(self) -> int:
        return sum(d.used_bytes for d in self.drives)

    def total_capacity_bytes(self) -> int:
        return sum(d.capacity_bytes for d in self.drives)

    def combined_utilization(self) -> float:
        total_cap = self.total_capacity_bytes()
        if total_cap == 0:
            return 0.0
        return self.total_used_bytes() / total_cap
