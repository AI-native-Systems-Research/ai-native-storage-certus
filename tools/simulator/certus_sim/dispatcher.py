"""Dispatcher: core cache orchestrator as SimPy processes.

Implements the certus-server dispatcher logic:
- populate: GPU→DRAM with async write-through to SSD
- lookup/batch_lookup: DRAM hit (hot) or SSD promotion (cold)
- evict_for_space: sparse-probe + shard-targeted LRU
- background write-through worker
- SSD capacity evictor
"""

from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, field
from enum import Enum, auto
from typing import Optional

import simpy

from certus_sim.config import SimConfig
from certus_sim.dispatch_map import DispatchMap, EntryState
from certus_sim.gpu_dma import GpuDmaModel
from certus_sim.memory_tier import MemoryTier
from certus_sim.metrics import Metrics
from certus_sim.pipeline import PipelineModel
from certus_sim.ssd_tier import SsdTier


class OpResult(Enum):
    SUCCESS = auto()
    KEY_NOT_FOUND = auto()
    ALREADY_EXISTS = auto()
    ALLOCATION_FAILED = auto()


@dataclass
class WriteJob:
    key: int
    size: int
    enqueue_time: float


class Dispatcher:
    """SimPy-based dispatcher modeling the certus-server cache engine."""

    def __init__(
        self,
        env: simpy.Environment,
        config: SimConfig,
        metrics: Metrics,
    ):
        self.env = env
        self.config = config
        self.metrics = metrics

        self.memory_tier = MemoryTier(
            config.memory_tier_capacity_bytes, config.memory_tier_shards
        )
        self.ssd = SsdTier(config.num_drives, config.drive_capacity_bytes)
        self.dispatch_map = DispatchMap()
        self.gpu = GpuDmaModel(config)
        self.pipeline = PipelineModel(config, self.gpu)

        self._write_queue: list[WriteJob] = []
        self._shutdown = False

        # Start background processes
        if config.write_through_enabled:
            env.process(self._background_write_through())
        if config.ssd_eviction_threshold > 0.0:
            env.process(self._ssd_evictor())

    def populate(self, key: int, size: int) -> simpy.events.Process:
        """SimPy process: populate a cache entry (GPU→DRAM→async SSD)."""
        return self.env.process(self._populate(key, size))

    def _populate(self, key: int, size: int):
        start = self.env.now

        # Check if already exists
        if self.dispatch_map.exists(key):
            self.metrics.record_populate(self.env.now - start, success=False)
            return OpResult.ALREADY_EXISTS

        # Dispatch map operation
        yield self.env.timeout(self.config.dispatch_map_op_us)

        # Evict if needed
        evict_result = yield self.env.process(self._evict_for_space(size, key))
        if evict_result == OpResult.ALLOCATION_FAILED:
            self.metrics.record_populate(self.env.now - start, success=False)
            return OpResult.ALLOCATION_FAILED

        # Memory tier insertion
        yield self.env.timeout(self.config.memory_tier_insert_us)
        inserted = self.memory_tier.insert(key, size, self.env.now)
        if not inserted:
            self.metrics.record_populate(self.env.now - start, success=False)
            return OpResult.ALLOCATION_FAILED

        # Register in dispatch map
        self.dispatch_map.create_memory_tier_entry(key, size, self.env.now)

        # GPU device-to-host DMA (populate direction)
        yield self.env.timeout(self.gpu.d2h_latency(size))

        # Downgrade write ref to read ref (data now readable)
        self.dispatch_map.downgrade_reference(key)

        # Enqueue write-through job
        if self.config.write_through_enabled:
            self._write_queue.append(WriteJob(key=key, size=size, enqueue_time=self.env.now))

        latency = self.env.now - start
        self.metrics.record_populate(latency, success=True)
        return OpResult.SUCCESS

    def lookup(self, key: int, size: int) -> simpy.events.Process:
        """SimPy process: lookup a single cache entry."""
        return self.env.process(self._lookup(key, size))

    def _lookup(self, key: int, size: int):
        start = self.env.now

        # Dispatch map lookup
        yield self.env.timeout(self.config.dispatch_map_op_us)
        entry = self.dispatch_map.lookup(key)

        if entry is None:
            self.metrics.record_lookup(self.env.now - start, hot=False, success=False)
            return OpResult.KEY_NOT_FOUND

        if entry.state == EntryState.MEMORY_TIER:
            # Hot path: DMA from memory tier to GPU
            self.dispatch_map.take_read(key)
            self.memory_tier.touch(key)
            yield self.env.timeout(self.gpu.h2d_latency(size))
            self.dispatch_map.release_read(key)
            self.dispatch_map.touch(key, self.env.now)

            latency = self.env.now - start
            self.metrics.record_lookup(latency, hot=True, success=True)
            return OpResult.SUCCESS

        elif entry.state == EntryState.BLOCK_DEVICE:
            # Cold path: promote from SSD
            yield self.env.process(
                self._promote_and_serve(key, entry.drive_idx, entry.ssd_offset, size)
            )
            latency = self.env.now - start
            self.metrics.record_lookup(latency, hot=False, success=True)
            return OpResult.SUCCESS

    def batch_lookup(self, keys: list[int], size: int) -> simpy.events.Process:
        """SimPy process: batch lookup with parallel cold promotion."""
        return self.env.process(self._batch_lookup(keys, size))

    def _batch_lookup(self, keys: list[int], size: int):
        start = self.env.now

        # Classify entries
        yield self.env.timeout(self.config.dispatch_map_op_us * len(keys))

        hot_keys: list[int] = []
        cold_by_drive: dict[int, list[int]] = defaultdict(list)

        for key in keys:
            entry = self.dispatch_map.lookup(key)
            if entry is None:
                self.metrics.record_lookup(0, hot=False, success=False)
            elif entry.state == EntryState.MEMORY_TIER:
                hot_keys.append(key)
            elif entry.state == EntryState.BLOCK_DEVICE:
                cold_by_drive[entry.drive_idx].append(key)

        # Serve hot entries inline
        if hot_keys:
            for key in hot_keys:
                self.dispatch_map.take_read(key)
                self.memory_tier.touch(key)
            yield self.env.timeout(self.gpu.h2d_latency(size))
            for key in hot_keys:
                self.dispatch_map.release_read(key)
                self.dispatch_map.touch(key, self.env.now)
                self.metrics.record_lookup(self.env.now - start, hot=True, success=True)

        # Parallel cold promotion across drives
        if cold_by_drive:
            entries_per_drive = {d: len(ks) for d, ks in cold_by_drive.items()}
            promote_time = self.pipeline.batch_promote_latency(entries_per_drive, size)
            yield self.env.timeout(promote_time)

            # Update state for all cold entries
            for drive_idx, cold_keys in cold_by_drive.items():
                for key in cold_keys:
                    # Evict space and re-insert into memory tier
                    yield self.env.process(self._evict_for_space(size, key))
                    self.memory_tier.insert(key, size, self.env.now)
                    entry = self.dispatch_map.lookup(key)
                    if entry:
                        entry.state = EntryState.MEMORY_TIER
                    self.dispatch_map.touch(key, self.env.now)
                    self.metrics.record_lookup(self.env.now - start, hot=False, success=True)

    def check(self, key: int) -> bool:
        """Synchronous check (no timing needed, instant map lookup)."""
        return self.dispatch_map.exists(key)

    def remove(self, key: int) -> simpy.events.Process:
        """SimPy process: remove a cache entry."""
        return self.env.process(self._remove(key))

    def _remove(self, key: int):
        yield self.env.timeout(self.config.dispatch_map_op_us)
        entry = self.dispatch_map.lookup(key)
        if entry is None:
            self.metrics.record_remove(success=False)
            return OpResult.KEY_NOT_FOUND

        # Remove from memory tier if present
        if entry.state == EntryState.MEMORY_TIER:
            self.memory_tier.remove(key)

        # Free SSD extent if present
        if entry.ssd_offset is not None and entry.drive_idx is not None:
            self.ssd.free(entry.drive_idx, entry.ssd_offset)

        self.dispatch_map.remove(key)
        self.metrics.record_remove(success=True)
        return OpResult.SUCCESS

    def touch(self, key: int) -> OpResult:
        """Touch an entry (update timestamp, refresh LRU)."""
        entry = self.dispatch_map.lookup(key)
        if entry is None:
            return OpResult.KEY_NOT_FOUND
        self.dispatch_map.touch(key, self.env.now)
        if entry.state == EntryState.MEMORY_TIER:
            self.memory_tier.touch(key)
        self.metrics.record_touch()
        return OpResult.SUCCESS

    # --- Internal processes ---

    def _promote_and_serve(self, key: int, drive_idx: int, offset: int, size: int):
        """Promote a cold entry from SSD back to memory tier + GPU."""
        # Evict space for the promoted entry
        yield self.env.process(self._evict_for_space(size, key))

        # Memory tier insertion
        yield self.env.timeout(self.config.memory_tier_insert_us)
        self.memory_tier.insert(key, size, self.env.now)

        # Pipelined SSD read + GPU DMA (already accounted for by caller in batch)
        promote_latency = self.pipeline.single_entry_promote_latency(size)
        yield self.env.timeout(promote_latency)

        # Update dispatch map: BlockDevice → MemoryTier (preserving ssd_offset)
        entry = self.dispatch_map.lookup(key)
        if entry:
            entry.state = EntryState.MEMORY_TIER
        self.dispatch_map.touch(key, self.env.now)

    def _evict_for_space(self, needed: int, target_key: int):
        """Spec algorithm: sparse-probe + shard-targeted LRU primary."""
        attempt = 0
        while self.memory_tier.used_bytes + needed > self.memory_tier.capacity_bytes:
            if attempt >= self.config.max_eviction_attempts:
                return OpResult.ALLOCATION_FAILED
            attempt += 1

            if attempt % 8 == 0:
                # Sparse probe: check oldest_keys for clean eviction candidates
                candidates = self.memory_tier.oldest_keys(4)
                evicted = False
                for cand_key in candidates:
                    if self.dispatch_map.is_evictable(cand_key):
                        self.memory_tier.remove(cand_key)
                        self.dispatch_map.convert_memory_tier_to_block(cand_key)
                        self.metrics.record_eviction(clean=True)
                        evicted = True
                        break
                if evicted:
                    continue

            # Primary path: shard-targeted blind LRU eviction
            evicted_entry = self.memory_tier.evict_lru_for_key(target_key)
            if evicted_entry is None:
                # Try global eviction as fallback
                evicted_entry = self.memory_tier.evict_lru()
                if evicted_entry is None:
                    return OpResult.ALLOCATION_FAILED

            # Try to transition to BlockDevice
            dm_entry = self.dispatch_map.lookup(evicted_entry.key)
            if dm_entry and dm_entry.ssd_offset is not None:
                self.dispatch_map.convert_memory_tier_to_block(evicted_entry.key)
                self.metrics.record_eviction(clean=True)
            else:
                # Data loss: no SSD backing, remove entirely
                self.dispatch_map.remove(evicted_entry.key)
                self.metrics.record_eviction(clean=False)

            # Small timing for eviction overhead
            yield self.env.timeout(0.1)

        return OpResult.SUCCESS

    def _background_write_through(self):
        """Background process that flushes memory-tier entries to SSD."""
        while not self._shutdown:
            if not self._write_queue:
                yield self.env.timeout(10.0)  # poll interval
                continue

            job = self._write_queue.pop(0)

            # Check entry still exists in memory tier
            entry = self.dispatch_map.lookup(job.key)
            if entry is None or entry.state != EntryState.MEMORY_TIER:
                continue
            if entry.ssd_offset is not None:
                continue  # already written

            # Peek memory tier (no LRU refresh per spec)
            mt_entry = self.memory_tier.peek(job.key)
            if mt_entry is None:
                continue

            # Allocate SSD extent
            result = self.ssd.allocate(job.key, job.size)
            if result is None:
                # SSD full — silently drop (spec FR-017)
                self.metrics.record_write_through(success=False)
                continue

            drive_idx, offset = result

            # NVMe write time (segmented by MDTS)
            num_segments = self.config.segments_per_entry()
            write_time = num_segments * self.config.nvme_write_latency_us
            yield self.env.timeout(write_time)

            # Update dispatch map with SSD offset
            self.dispatch_map.convert_to_storage(job.key, drive_idx, offset)
            self.metrics.record_write_through(success=True)

    def _ssd_evictor(self):
        """Periodic SSD capacity evictor."""
        while not self._shutdown:
            yield self.env.timeout(self.config.ssd_eviction_interval_us)

            utilization = self.ssd.combined_utilization()
            if utilization <= self.config.ssd_eviction_threshold:
                continue

            # Evict until below low watermark
            evicted = 0
            batch_keys = self.dispatch_map.oldest_keys(self.config.ssd_eviction_batch_size)
            for key in batch_keys:
                if self.ssd.combined_utilization() <= self.config.ssd_eviction_low_watermark:
                    break

                entry = self.dispatch_map.lookup(key)
                if entry is None:
                    continue
                if entry.state != EntryState.BLOCK_DEVICE:
                    continue
                if entry.read_ref > 0 or entry.write_ref > 0:
                    continue

                # Free SSD extent and remove from dispatch map
                if entry.ssd_offset is not None and entry.drive_idx is not None:
                    self.ssd.free(entry.drive_idx, entry.ssd_offset)
                self.dispatch_map.remove(key)
                evicted += 1
                self.metrics.record_ssd_eviction()

    def shutdown(self):
        self._shutdown = True
