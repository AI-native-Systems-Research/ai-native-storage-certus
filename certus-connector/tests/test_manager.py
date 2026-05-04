# SPDX-License-Identifier: Apache-2.0
"""Unit tests for the tiered CertusOffloadingManager.

Runs WITHOUT vllm/torch — mocks the vllm imports.
"""

import sys
import time
import types
from unittest.mock import patch

import pytest

# ── Mock vllm modules ──
_mock_modules = {}
for mod_name in [
    "vllm", "vllm.v1", "vllm.v1.core", "vllm.v1.core.kv_cache_utils",
    "vllm.v1.kv_offload", "vllm.v1.kv_offload.abstract",
    "vllm.v1.kv_offload.mediums", "vllm.v1.kv_offload.worker",
    "vllm.v1.kv_offload.worker.worker", "vllm.v1.kv_offload.spec",
    "vllm.v1.kv_cache_interface", "vllm.v1.attention",
    "vllm.v1.attention.backend", "vllm.config", "vllm.logger",
]:
    _mock_modules[mod_name] = types.ModuleType(mod_name)
    sys.modules[mod_name] = _mock_modules[mod_name]

from abc import ABC, abstractmethod
from collections.abc import Iterable
from dataclasses import dataclass
from typing import NewType

# OffloadKey stub
OffloadKey = NewType("OffloadKey", bytes)


def make_offload_key(block_hash: bytes, group_idx: int) -> OffloadKey:
    return OffloadKey(block_hash + group_idx.to_bytes(4, "big", signed=False))


def get_offload_block_hash(key: OffloadKey) -> bytes:
    return key[:-4]


# LoadStoreSpec stub
class LoadStoreSpec(ABC):
    @staticmethod
    @abstractmethod
    def medium() -> str: ...


@dataclass
class PrepareStoreOutput:
    keys_to_store: list
    store_spec: object
    evicted_keys: list


@dataclass
class OffloadingEvent:
    keys: list
    block_size: int
    medium: str
    removed: bool


class OffloadingManager(ABC):
    @abstractmethod
    def lookup(self, keys): ...
    @abstractmethod
    def prepare_load(self, keys): ...
    def touch(self, keys): return
    def complete_load(self, keys): return
    @abstractmethod
    def prepare_store(self, keys): ...
    def complete_store(self, keys, success=True): return
    def take_events(self): return ()
    def shutdown(self): return


sys.modules["vllm.v1.kv_offload.abstract"].LoadStoreSpec = LoadStoreSpec
sys.modules["vllm.v1.kv_offload.abstract"].OffloadingManager = OffloadingManager
sys.modules["vllm.v1.kv_offload.abstract"].PrepareStoreOutput = PrepareStoreOutput
sys.modules["vllm.v1.kv_offload.abstract"].OffloadingEvent = OffloadingEvent
sys.modules["vllm.v1.kv_offload.abstract"].OffloadKey = OffloadKey
sys.modules["vllm.v1.kv_offload.abstract"].make_offload_key = make_offload_key
sys.modules["vllm.v1.kv_offload.abstract"].get_offload_block_hash = get_offload_block_hash

# Logger stub
def init_logger(name):
    import logging
    return logging.getLogger(name)

sys.modules["vllm.logger"].init_logger = init_logger

# Now import our code
from certus_connector.manager import CertusOffloadingManager, TieringConfig
from certus_connector.mediums import CertusLoadStoreSpec, BlockLocation


def make_key(i: int) -> OffloadKey:
    return OffloadKey(i.to_bytes(8, "big") + (0).to_bytes(4, "big"))


class TestTieredManager:
    @pytest.fixture
    def config(self):
        return TieringConfig(
            max_nvme_slabs=10,
            max_dram_slots=5,
            promotion_touch_threshold=3,
            promotion_window_seconds=10.0,
            demotion_idle_seconds=2.0,
        )

    @pytest.fixture
    def manager(self, config):
        return CertusOffloadingManager(config=config, enable_events=True)

    def test_lookup_empty(self, manager):
        keys = [make_key(i) for i in range(5)]
        assert manager.lookup(keys) == 0

    def test_store_and_lookup(self, manager):
        keys = [make_key(i) for i in range(3)]
        output = manager.prepare_store(keys)
        assert output is not None
        assert len(output.keys_to_store) == 3
        assert isinstance(output.store_spec, CertusLoadStoreSpec)
        assert len(output.store_spec.locations) == 3

        # Not ready before complete_store
        assert manager.lookup(keys) == 0

        manager.complete_store(keys, success=True)
        assert manager.lookup(keys) == 3

    def test_store_allocates_both_tiers(self, manager):
        """Store through CPU gives both NVMe slab and DRAM slot."""
        keys = [make_key(0)]
        output = manager.prepare_store(keys)
        manager.complete_store(keys, success=True)

        spec = manager.prepare_load(keys)
        loc = spec.locations[0]
        assert loc.nvme_slab is not None
        assert loc.dram_slot is not None
        manager.complete_load(keys)

    def test_dram_slots_limited(self, config, manager):
        """Only max_dram_slots blocks get DRAM slots."""
        # Store 7 blocks (max_dram_slots=5)
        keys = [make_key(i) for i in range(7)]
        output = manager.prepare_store(keys)
        manager.complete_store(keys, success=True)

        spec = manager.prepare_load(keys)
        dram_count = sum(1 for loc in spec.locations if loc.dram_slot is not None)
        # First 5 get DRAM, last 2 don't (or some get demoted to make room)
        assert dram_count <= config.max_dram_slots
        manager.complete_load(keys)

    def test_lookup_partial_prefix(self, manager):
        k0, k1, k2 = make_key(0), make_key(1), make_key(2)
        output = manager.prepare_store([k0, k1])
        manager.complete_store([k0, k1], success=True)

        assert manager.lookup([k0, k1, k2]) == 2
        assert manager.lookup([k2, k0]) == 0

    def test_eviction_frees_both_tiers(self, manager):
        """Eviction frees both NVMe slab and DRAM slot."""
        # Fill NVMe capacity (10 slabs)
        keys = [make_key(i) for i in range(10)]
        output = manager.prepare_store(keys)
        manager.complete_store(keys, success=True)

        # Touch recent ones
        manager.touch([make_key(8), make_key(9)])

        # Store 2 more — should evict LRU
        new_keys = [make_key(100), make_key(101)]
        output = manager.prepare_store(new_keys)
        assert output is not None
        assert len(output.evicted_keys) == 2
        manager.complete_store(new_keys, success=True)

        # Evicted blocks gone
        assert manager.lookup([make_key(0)]) == 0

    def test_eviction_skips_pinned(self, manager):
        """Pinned blocks (ref_cnt > 0) are not evicted."""
        keys = [make_key(i) for i in range(10)]
        manager.prepare_store(keys)
        manager.complete_store(keys, success=True)

        # Pin block 0
        manager.prepare_load([make_key(0)])

        # Store 1 more — should evict something else
        new_key = [make_key(200)]
        output = manager.prepare_store(new_key)
        assert output is not None
        assert make_key(0) not in output.evicted_keys

        manager.complete_load([make_key(0)])

    def test_store_already_stored_noop(self, manager):
        keys = [make_key(0)]
        manager.prepare_store(keys)
        manager.complete_store(keys, success=True)

        output = manager.prepare_store(keys)
        assert output is not None
        assert len(output.keys_to_store) == 0

    def test_complete_store_failure(self, manager):
        keys = [make_key(0)]
        manager.prepare_store(keys)
        manager.complete_store(keys, success=False)
        assert manager.lookup(keys) == 0

    def test_touch_updates_lru(self, manager):
        """Touched blocks are not evicted first."""
        keys = [make_key(i) for i in range(10)]
        manager.prepare_store(keys)
        manager.complete_store(keys, success=True)

        # Touch block 0 repeatedly — should be safe from eviction
        for _ in range(5):
            manager.touch([make_key(0)])

        # Evict 2
        new_keys = [make_key(50), make_key(51)]
        output = manager.prepare_store(new_keys)
        assert output is not None
        assert make_key(0) not in output.evicted_keys

    @patch("certus_connector.manager.time.monotonic")
    def test_promotion_via_touch(self, mock_time, config):
        """Block gets promoted to DRAM after enough touches."""
        config.max_dram_slots = 2
        config.max_nvme_slabs = 10
        config.promotion_touch_threshold = 3
        mgr = CertusOffloadingManager(config=config)

        # Store a block — it gets DRAM slot on store (free promotion)
        mock_time.return_value = 0.0
        keys = [make_key(0), make_key(1), make_key(2)]
        mgr.prepare_store(keys)
        mgr.complete_store(keys, success=True)

        # Block 2 may not have DRAM (only 2 slots for 3 blocks)
        spec = mgr.prepare_load([make_key(2)])
        had_dram = spec.locations[0].dram_slot is not None
        mgr.complete_load([make_key(2)])

        if not had_dram:
            # Touch block 2 enough times to trigger promotion
            mock_time.return_value = 1.0
            mgr.touch([make_key(2)])
            mock_time.return_value = 2.0
            mgr.touch([make_key(2)])
            mock_time.return_value = 3.0
            mgr.touch([make_key(2)])

            spec = mgr.prepare_load([make_key(2)])
            assert spec.locations[0].dram_slot is not None
            mgr.complete_load([make_key(2)])

    @patch("certus_connector.manager.time.monotonic")
    def test_demotion_idle_blocks(self, mock_time, config):
        """Idle blocks get demoted from DRAM."""
        config.max_dram_slots = 5
        config.demotion_idle_seconds = 2.0
        mgr = CertusOffloadingManager(config=config)

        mock_time.return_value = 0.0
        keys = [make_key(0)]
        mgr.prepare_store(keys)
        mgr.complete_store(keys, success=True)

        # Verify has DRAM
        spec = mgr.prepare_load(keys)
        assert spec.locations[0].dram_slot is not None
        mgr.complete_load(keys)

        # Advance time past demotion threshold
        mock_time.return_value = 5.0
        demoted = mgr.run_demotion_pass()
        assert make_key(0) in demoted

        # Block still on NVMe, just no DRAM
        assert mgr.lookup(keys) == 1
        spec = mgr.prepare_load(keys)
        assert spec.locations[0].dram_slot is None
        assert spec.locations[0].nvme_slab is not None
        mgr.complete_load(keys)

    def test_events_emitted(self, manager):
        keys = [make_key(0)]
        manager.prepare_store(keys)
        manager.complete_store(keys, success=True)

        events = list(manager.take_events())
        assert len(events) == 1
        assert events[0].removed is False

        assert list(manager.take_events()) == []

    def test_unlimited_nvme(self):
        """max_nvme_slabs=0 means unlimited."""
        config = TieringConfig(max_nvme_slabs=0, max_dram_slots=0)
        mgr = CertusOffloadingManager(config=config)
        keys = [make_key(i) for i in range(100)]
        output = mgr.prepare_store(keys)
        assert output is not None
        assert len(output.evicted_keys) == 0
        mgr.complete_store(keys, success=True)
        assert mgr.lookup(keys) == 100

    def test_no_file_paths(self, manager):
        """Verify no file paths anywhere in the spec."""
        keys = [make_key(0)]
        output = manager.prepare_store(keys)
        spec = output.store_spec
        assert not hasattr(spec, "file_paths")
        for loc in spec.locations:
            assert not hasattr(loc, "file_path")
