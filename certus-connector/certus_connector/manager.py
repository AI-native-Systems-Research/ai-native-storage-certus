# SPDX-License-Identifier: Apache-2.0
"""Tiered OffloadingManager for Certus: pinned DRAM (Tier 1) + NVMe via SPDK (Tier 2).

Blocks are identified by OffloadKey (opaque bytes). The manager maintains an
in-memory hash index mapping keys to BlockAddress (nvme_slab, dram_slot).

Tiering policy:
- Store: allocate NVMe slab + DRAM slot (block lands in both tiers after store)
- Promotion: blocks touched frequently get DRAM slot if not already resident
- Demotion: DRAM slots released for idle blocks (data remains on NVMe)
- Eviction: when NVMe is full, LRU blocks are evicted (slab freed)
"""

from __future__ import annotations

import time
from collections import OrderedDict
from collections.abc import Iterable
from dataclasses import dataclass, field

from vllm.v1.kv_offload.abstract import (
    LoadStoreSpec,
    OffloadingEvent,
    OffloadingManager,
    OffloadKey,
    PrepareStoreOutput,
)

from certus_connector.mediums import BlockLocation, CertusLoadStoreSpec


@dataclass
class BlockState:
    nvme_slab: int
    dram_slot: int | None = None
    ref_cnt: int = 0
    is_ready: bool = False
    last_touch: float = 0.0
    touch_count: int = 0


@dataclass
class TieringConfig:
    max_nvme_slabs: int = 0
    max_dram_slots: int = 0
    promotion_touch_threshold: int = 3
    promotion_window_seconds: float = 10.0
    demotion_idle_seconds: float = 30.0


class CertusOffloadingManager(OffloadingManager):
    """Tiered manager: DRAM (fast) + NVMe (capacity).

    On store, blocks get both an NVMe slab and a DRAM slot (store goes
    through CPU, so DRAM residency is free). On demotion, DRAM slot is
    released but NVMe slab remains. On eviction, NVMe slab is freed.
    """

    def __init__(self, config: TieringConfig, enable_events: bool = False):
        self._config = config

        # Primary index: key → state
        self._blocks: OrderedDict[OffloadKey, BlockState] = OrderedDict()

        # NVMe slab allocator (free-list)
        self._nvme_free: list[int] = []
        self._nvme_next_slab: int = 0

        # DRAM slot allocator (free-list)
        self._dram_free: list[int] = []
        self._dram_next_slot: int = 0
        self._dram_used: int = 0

        # Touch tracking for promotion decisions
        self._touch_window: dict[OffloadKey, list[float]] = {}

        self._events: list[OffloadingEvent] | None = (
            [] if enable_events else None
        )

    # ── Slab/slot allocation ──

    def _alloc_nvme_slab(self) -> int | None:
        if self._nvme_free:
            return self._nvme_free.pop()
        if self._config.max_nvme_slabs <= 0 or self._nvme_next_slab < self._config.max_nvme_slabs:
            slab = self._nvme_next_slab
            self._nvme_next_slab += 1
            return slab
        return None

    def _free_nvme_slab(self, slab: int) -> None:
        self._nvme_free.append(slab)

    def _alloc_dram_slot(self) -> int | None:
        if self._dram_free:
            self._dram_used += 1
            return self._dram_free.pop()
        if self._config.max_dram_slots <= 0 or self._dram_next_slot < self._config.max_dram_slots:
            slot = self._dram_next_slot
            self._dram_next_slot += 1
            self._dram_used += 1
            return slot
        return None

    def _free_dram_slot(self, slot: int) -> None:
        self._dram_free.append(slot)
        self._dram_used -= 1

    def _nvme_capacity_remaining(self) -> int:
        if self._config.max_nvme_slabs <= 0:
            return 2**31
        allocated = self._nvme_next_slab - len(self._nvme_free)
        return self._config.max_nvme_slabs - allocated

    # ── Promotion / Demotion ──

    def _should_promote(self, key: OffloadKey) -> bool:
        """Check if block should be promoted to DRAM based on touch frequency."""
        now = time.monotonic()
        window = self._config.promotion_window_seconds
        touches = self._touch_window.get(key, [])
        recent = [t for t in touches if now - t <= window]
        self._touch_window[key] = recent
        return len(recent) >= self._config.promotion_touch_threshold

    def _try_promote(self, key: OffloadKey) -> None:
        """Promote block to DRAM if eligible and slots available."""
        block = self._blocks.get(key)
        if block is None or block.dram_slot is not None:
            return
        if not self._should_promote(key):
            return
        slot = self._alloc_dram_slot()
        if slot is None:
            self._demote_coldest()
            slot = self._alloc_dram_slot()
        if slot is not None:
            block.dram_slot = slot

    def _demote_coldest(self) -> None:
        """Demote the least-recently-touched DRAM-resident block."""
        coldest_key: OffloadKey | None = None
        coldest_time = float("inf")
        for key, block in self._blocks.items():
            if block.dram_slot is not None and block.ref_cnt == 0:
                if block.last_touch < coldest_time:
                    coldest_time = block.last_touch
                    coldest_key = key
        if coldest_key is not None:
            block = self._blocks[coldest_key]
            self._free_dram_slot(block.dram_slot)
            block.dram_slot = None

    def run_demotion_pass(self) -> list[OffloadKey]:
        """Demote idle DRAM blocks. Called periodically by background thread."""
        now = time.monotonic()
        demoted: list[OffloadKey] = []
        for key, block in list(self._blocks.items()):
            if block.dram_slot is None or block.ref_cnt > 0:
                continue
            if now - block.last_touch > self._config.demotion_idle_seconds:
                self._free_dram_slot(block.dram_slot)
                block.dram_slot = None
                demoted.append(key)
        return demoted

    # ── Eviction (NVMe tier) ──

    def _evict(self, count: int, protected: set[OffloadKey]) -> list[OffloadKey]:
        """Evict LRU blocks from NVMe. Also frees DRAM slot if present."""
        evicted: list[OffloadKey] = []
        for key in list(self._blocks.keys()):
            if len(evicted) >= count:
                break
            if key in protected:
                continue
            block = self._blocks[key]
            if block.ref_cnt > 0:
                continue
            if block.dram_slot is not None:
                self._free_dram_slot(block.dram_slot)
            self._free_nvme_slab(block.nvme_slab)
            del self._blocks[key]
            self._touch_window.pop(key, None)
            evicted.append(key)
        return evicted

    # ── OffloadingManager interface ──

    def lookup(self, keys: Iterable[OffloadKey]) -> int | None:
        hit_count = 0
        for key in keys:
            block = self._blocks.get(key)
            if block is None or not block.is_ready:
                break
            hit_count += 1
        return hit_count

    def prepare_load(self, keys: Iterable[OffloadKey]) -> LoadStoreSpec:
        locations: list[BlockLocation] = []
        for key in keys:
            block = self._blocks.get(key)
            assert block is not None, f"Block {key!r} not in index"
            assert block.is_ready, f"Block {key!r} not ready"
            block.ref_cnt += 1
            locations.append(BlockLocation(
                nvme_slab=block.nvme_slab,
                dram_slot=block.dram_slot,
            ))
        return CertusLoadStoreSpec(locations)

    def touch(self, keys: Iterable[OffloadKey]) -> None:
        now = time.monotonic()
        for key in keys:
            block = self._blocks.get(key)
            if block is None:
                continue
            self._blocks.move_to_end(key)
            block.last_touch = now
            block.touch_count += 1
            touches = self._touch_window.setdefault(key, [])
            touches.append(now)
            self._try_promote(key)

    def complete_load(self, keys: Iterable[OffloadKey]) -> None:
        for key in keys:
            block = self._blocks.get(key)
            assert block is not None
            assert block.ref_cnt > 0
            block.ref_cnt -= 1

    def prepare_store(self, keys: Iterable[OffloadKey]) -> PrepareStoreOutput | None:
        keys_list = list(keys)

        # Filter already-stored
        to_store = [k for k in keys_list if k not in self._blocks]

        if not to_store:
            return PrepareStoreOutput(
                keys_to_store=[],
                store_spec=CertusLoadStoreSpec([]),
                evicted_keys=[],
            )

        # Evict if needed
        evicted_keys: list[OffloadKey] = []
        needed = len(to_store) - self._nvme_capacity_remaining()
        if needed > 0:
            protected = set(keys_list)
            evicted_keys = self._evict(needed, protected)
            if len(evicted_keys) < needed:
                return None

        # Allocate slabs and slots for new blocks
        locations: list[BlockLocation] = []
        now = time.monotonic()
        for key in to_store:
            nvme_slab = self._alloc_nvme_slab()
            assert nvme_slab is not None
            # Store goes through CPU — block lands in DRAM for free
            dram_slot = self._alloc_dram_slot()
            block = BlockState(
                nvme_slab=nvme_slab,
                dram_slot=dram_slot,
                ref_cnt=1,  # protected until complete_store
                is_ready=False,
                last_touch=now,
            )
            self._blocks[key] = block
            locations.append(BlockLocation(
                nvme_slab=nvme_slab,
                dram_slot=dram_slot,
            ))

        if evicted_keys and self._events is not None:
            self._events.append(OffloadingEvent(
                keys=evicted_keys,
                block_size=0,
                medium=CertusLoadStoreSpec.medium(),
                removed=True,
            ))

        return PrepareStoreOutput(
            keys_to_store=to_store,
            store_spec=CertusLoadStoreSpec(locations),
            evicted_keys=evicted_keys,
        )

    def complete_store(self, keys: Iterable[OffloadKey], success: bool = True) -> None:
        stored_keys: list[OffloadKey] = []
        for key in keys:
            block = self._blocks.get(key)
            if block is None:
                continue
            if success and not block.is_ready:
                block.is_ready = True
                block.ref_cnt = 0
                stored_keys.append(key)
            elif not success and not block.is_ready:
                if block.dram_slot is not None:
                    self._free_dram_slot(block.dram_slot)
                self._free_nvme_slab(block.nvme_slab)
                del self._blocks[key]
                self._touch_window.pop(key, None)

        if stored_keys and self._events is not None:
            self._events.append(OffloadingEvent(
                keys=stored_keys,
                block_size=0,
                medium=CertusLoadStoreSpec.medium(),
                removed=False,
            ))

    def take_events(self) -> Iterable[OffloadingEvent]:
        if self._events is not None:
            yield from self._events
            self._events.clear()

    def shutdown(self) -> None:
        self._blocks.clear()
        self._touch_window.clear()
