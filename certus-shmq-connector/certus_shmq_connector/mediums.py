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

# High-bit width reserved for the TP shard rank when namespacing keys (8 bits →
# up to 256 ranks). The low 56 bits carry the (masked) logical block key.
_RANK_SHIFT = 56
_KEY_MASK = (1 << _RANK_SHIFT) - 1


def ns_key(key: int, rank: int, world_size: int) -> int:
    """Namespace a logical block key by its TP shard rank.

    Under tensor parallelism (world_size>1) each rank's worker holds a DIFFERENT
    head-shard of every KV block but computes the SAME content-hash u64 key. In a
    single shared certus-server tier those identical keys would collide → one
    rank's shard silently overwrites/serves another's → wrong-shard loads and
    garbage output that completes rc=0 (not a crash). Folding the rank into the
    high 8 bits gives each rank a disjoint keyspace in the one server.

    IDENTITY when world_size<=1, so the single-GPU path is byte-for-byte the
    historical baseline (no masking, no collision surface). The 8-bit fold costs
    8 bits of a 64-bit content hash — negligible collision risk for random keys.
    """
    if world_size <= 1:
        return key

    rank_bits = 64 - _RANK_SHIFT
    max_ranks = 1 << rank_bits
    if not (0 <= rank < max_ranks):
        raise ValueError(f"tp rank {rank} out of range (expected 0..{max_ranks - 1})")
    if world_size > max_ranks:
        raise ValueError(f"world_size {world_size} exceeds max supported {max_ranks}")

    return (key & _KEY_MASK) | ((rank & (max_ranks - 1)) << _RANK_SHIFT)


def denamespace_key(key: int, world_size: int) -> int:
    """Recover the logical key from a namespaced one (mask off the rank bits).

    Identity when world_size<=1 (keys were never namespaced). Used by
    ``take_events`` to collapse the W per-rank eviction events for one logical
    block back to a single logical eviction for vLLM's accounting.
    """
    if world_size <= 1:
        return key
    return key & _KEY_MASK


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
