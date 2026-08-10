# SPDX-License-Identifier: Apache-2.0
"""LoadStoreSpec for Certus tiered storage accessed over gRPC.

The Certus side of a transfer is identified purely by cache key (u64). The GPU
side (block ids) arrives in the paired ``GPULoadStoreSpec`` at transfer time, so
this spec only needs to carry the ordered keys; the handler zips them with the
GPU block ids to build per-block IPC handles with offsets.
"""

from __future__ import annotations

from dataclasses import dataclass

from .compat import LoadStoreSpec


@dataclass
class BlockLocation:
    """A single Certus block, identified by its u64 cache key."""

    key: int


class CertusLoadStoreSpec(LoadStoreSpec):
    """Spec carrying the ordered Certus cache keys for a transfer.

    No file paths or device pointers — the GPU addressing is reconstructed by
    the handler from the paired GPULoadStoreSpec's block ids plus the shared
    KV-cache IPC handle.
    """

    def __init__(self, locations: list[BlockLocation]):
        self.locations = locations

    @property
    def keys(self) -> list[int]:
        return [loc.key for loc in self.locations]

    @staticmethod
    def medium() -> str:
        return "Certus"

    def __repr__(self) -> str:
        return f"CertusLoadStoreSpec(n_blocks={len(self.locations)})"
