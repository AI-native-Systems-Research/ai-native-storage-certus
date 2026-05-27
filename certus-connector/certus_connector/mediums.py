# SPDX-License-Identifier: Apache-2.0
"""LoadStoreSpec for Certus tiered storage (DRAM + NVMe via SPDK)."""

from __future__ import annotations

from dataclasses import dataclass

from vllm.v1.kv_offload.abstract import LoadStoreSpec


@dataclass
class BlockLocation:
    nvme_slab: int | None = None
    dram_slot: int | None = None


class CertusLoadStoreSpec(LoadStoreSpec):
    """Spec carrying block locations for the Certus handler.

    Each location identifies a block by its NVMe slab index and optional
    DRAM slot index. No file paths — raw device addressing only.
    """

    def __init__(self, locations: list[BlockLocation]):
        self.locations = locations

    @staticmethod
    def medium() -> str:
        return "Certus"

    def __repr__(self) -> str:
        return f"CertusLoadStoreSpec(n_blocks={len(self.locations)})"
