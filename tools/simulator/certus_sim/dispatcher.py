"""Dispatcher: core cache orchestrator as SimPy processes.

Implements the certus-server dispatcher logic matching the current Rust
IDispatcher trait (components/interfaces/src/idispatcher.rs):

- populate: three-phase (reserve_memory -> populate_memory -> memory_populated)
- prepare_store / commit_store / cancel_store: direct GPU->SSD write path
- lookup / batch_lookup: staging hit, memory-tier hit (hot), or SSD promotion (cold)
- evict_for_space: sparse-probe + shard-targeted LRU
- per-drive background write-through workers
- SSD capacity evictor
- promote_to_memory_tier: background SSD->DRAM promotion
- clear_memory_tier / flush_to_ssd
"""

from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, field
from enum import Enum, auto
from typing import Optional

import simpy

from certus_sim.config import SimConfig
from certus_sim.dispatch_map import DispatchMap, EntryState, LookupResult
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
    IO_ERROR = auto()


@dataclass
class WriteJob:
    key: int
    size: int
    drive_idx: int
    enqueue_time: float


@dataclass
class PendingStore:
    """Tracks a prepare_store that hasn't been committed yet."""
    key: int
    size: int
    drive_idx: int
    ssd_offset: int


class Dispatcher:
    """SimPy-based dispatcher modeling the certus-server cache engine.

    Matches the current Rust IDispatcher trait with:
    - Three-phase populate (reserve_memory / populate_memory / memory_populated)
    - Direct-write path (prepare_store / commit_store / cancel_store)
    - Per-drive background write-through
    - Staging state in dispatch map
    """

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

        # Per-drive write queues (matches Rust ParallelBackgroundWriter)
        self._write_queues: list[list[WriteJob]] = [[] for _ in range(config.num_drives)]
        self._pending_stores: dict[int, PendingStore] = {}
        self._reserved_memory: dict[int, int] = {}  # key -> size
        self._shutdown = False

        # Start background processes
        if config.write_through_enabled:
            for drive_idx in range(config.num_drives):
                env.process(self._background_write_through(drive_idx))
        if config.ssd_eviction_threshold > 0.0:
            env.process(self._ssd_evictor())

    # === Three-Phase Populate API ===

    def reserve_memory(self, key: int, size: int) -> simpy.events.Process:
        """Phase 1: Reserve DRAM slot, evicting if necessary."""
        return self.env.process(self._reserve_memory(key, size))

    def _reserve_memory(self, key: int, size: int):
        start = self.env.now

        if self.dispatch_map.exists(key):
            self.metrics.record_populate(self.env.now - start, success=False)
            return OpResult.ALREADY_EXISTS

        # Evict if needed
        evict_result = yield self.env.process(self._evict_for_space(size, key))
        if evict_result == OpResult.ALLOCATION_FAILED:
            self.metrics.record_populate(self.env.now - start, success=False)
            return OpResult.ALLOCATION_FAILED

        # Memory tier insertion
        yield self.env.timeout(self.config.memory_tier_insert_us)
        inserted = self.memory_tier.insert(key, size, self.env.now)
        if not inserted:
            return OpResult.ALLOCATION_FAILED

        self._reserved_memory[key] = size
        return OpResult.SUCCESS

    def populate_memory(self, key: int, size: int) -> simpy.events.Process:
        """Phase 2: DMA copy from GPU into reserved memory-tier slot."""
        return self.env.process(self._populate_memory(key, size))

    def _populate_memory(self, key: int, size: int):
        if key not in self._reserved_memory:
            return OpResult.KEY_NOT_FOUND

        # GPU device-to-host DMA
        yield self.env.timeout(self.gpu.d2h_latency(size))
        return OpResult.SUCCESS

    def memory_populated(self, key: int, size: int) -> simpy.events.Process:
        """Phase 3: Register in dispatch-map and enqueue write-through."""
        return self.env.process(self._memory_populated(key, size))

    def _memory_populated(self, key: int, size: int):
        if key not in self._reserved_memory:
            return OpResult.KEY_NOT_FOUND

        del self._reserved_memory[key]

        # Register in dispatch map (with write_ref=1)
        yield self.env.timeout(self.config.dispatch_map_op_us)
        self.dispatch_map.create_memory_tier_entry(key, size, self.env.now)

        # Downgrade write ref to read ref (data now readable)
        self.dispatch_map.downgrade_reference(key)

        # Enqueue write-through job on the target drive
        if self.config.write_through_enabled:
            drive_idx = self.config.drive_for_key(key)
            self._write_queues[drive_idx].append(
                WriteJob(key=key, size=size, drive_idx=drive_idx, enqueue_time=self.env.now)
            )

        return OpResult.SUCCESS

    def release_memory(self, key: int) -> OpResult:
        """Cancel a reservation without populating."""
        if key in self._reserved_memory:
            size = self._reserved_memory.pop(key)
            self.memory_tier.remove(key)
        return OpResult.SUCCESS

    # === Combined populate (convenience, uses three-phase internally) ===

    def populate(self, key: int, size: int) -> simpy.events.Process:
        """SimPy process: populate a cache entry (GPU->DRAM->async SSD)."""
        return self.env.process(self._populate(key, size))

    def _populate(self, key: int, size: int):
        start = self.env.now

        # Phase 1: reserve memory
        result = yield self.env.process(self._reserve_memory(key, size))
        if result != OpResult.SUCCESS:
            self.metrics.record_populate(self.env.now - start, success=False)
            return result

        # Phase 2: GPU D2H DMA
        result = yield self.env.process(self._populate_memory(key, size))
        if result != OpResult.SUCCESS:
            self.release_memory(key)
            self.metrics.record_populate(self.env.now - start, success=False)
            return result

        # Phase 3: finalize
        result = yield self.env.process(self._memory_populated(key, size))
        if result != OpResult.SUCCESS:
            self.metrics.record_populate(self.env.now - start, success=False)
            return result

        latency = self.env.now - start
        self.metrics.record_populate(latency, success=True)
        return OpResult.SUCCESS

    # === Direct-Write Path (prepare_store / commit_store / cancel_store) ===

    def prepare_store(self, key: int, size: int) -> simpy.events.Process:
        """Allocate SSD extent and staging entry for direct GPU->SSD write."""
        return self.env.process(self._prepare_store(key, size))

    def _prepare_store(self, key: int, size: int):
        start = self.env.now

        if self.dispatch_map.exists(key):
            self.metrics.record_prepare_store(self.env.now - start, success=False)
            return OpResult.ALREADY_EXISTS

        # Create staging entry in dispatch map (write_ref=1)
        yield self.env.timeout(self.config.dispatch_map_op_us)
        if not self.dispatch_map.create_staging(key, size):
            self.metrics.record_prepare_store(self.env.now - start, success=False)
            return OpResult.ALREADY_EXISTS

        # Allocate SSD extent
        result = self.ssd.allocate(key, size)
        if result is None:
            self.dispatch_map.remove(key)
            self.metrics.record_prepare_store(self.env.now - start, success=False)
            return OpResult.ALLOCATION_FAILED

        drive_idx, offset = result

        self._pending_stores[key] = PendingStore(
            key=key, size=size, drive_idx=drive_idx, ssd_offset=offset,
        )

        latency = self.env.now - start
        self.metrics.record_prepare_store(latency, success=True)
        return OpResult.SUCCESS

    def commit_store(self, key: int) -> simpy.events.Process:
        """Write staging buffer to SSD and publish the extent."""
        return self.env.process(self._commit_store(key))

    def _commit_store(self, key: int):
        start = self.env.now

        pending = self._pending_stores.pop(key, None)
        if pending is None:
            self.metrics.record_commit_store(self.env.now - start, success=False)
            return OpResult.KEY_NOT_FOUND

        # NVMe write time (segmented by MDTS)
        num_segments = max(1, (pending.size + self.config.mdts_bytes - 1) // self.config.mdts_bytes)
        write_time = num_segments * self.config.nvme_write_latency_us
        yield self.env.timeout(write_time)

        # Convert staging to block device in dispatch map
        self.dispatch_map.convert_to_storage(key, pending.drive_idx, pending.ssd_offset)

        # Release write ref
        self.dispatch_map.release_write(key)

        latency = self.env.now - start
        self.metrics.record_commit_store(latency, success=True)
        return OpResult.SUCCESS

    def cancel_store(self, key: int) -> OpResult:
        """Cancel a prepared store, freeing the reserved extent."""
        pending = self._pending_stores.pop(key, None)
        if pending is None:
            return OpResult.KEY_NOT_FOUND

        # Free the SSD extent
        self.ssd.free(pending.drive_idx, pending.ssd_offset)
        # Remove from dispatch map
        self.dispatch_map.release_write(key)
        self.dispatch_map.remove(key)
        self.metrics.record_cancel_store()
        return OpResult.SUCCESS

    # === Lookup Operations ===

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

        if entry.state == EntryState.STAGING:
            # Staging path: copy from staging buffer to GPU
            self.dispatch_map.take_read(key)
            yield self.env.timeout(self.gpu.h2d_latency(size))
            self.dispatch_map.release_read(key)
            self.dispatch_map.touch(key, self.env.now)

            latency = self.env.now - start
            self.metrics.record_lookup(latency, hot=True, success=True)
            return OpResult.SUCCESS

        elif entry.state == EntryState.MEMORY_TIER:
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
        staging_keys: list[int] = []
        cold_by_drive: dict[int, list[int]] = defaultdict(list)

        for key in keys:
            entry = self.dispatch_map.lookup(key)
            if entry is None:
                self.metrics.record_lookup(0, hot=False, success=False)
            elif entry.state == EntryState.STAGING:
                staging_keys.append(key)
            elif entry.state == EntryState.MEMORY_TIER:
                hot_keys.append(key)
            elif entry.state == EntryState.BLOCK_DEVICE:
                cold_by_drive[entry.drive_idx].append(key)

        # Serve staging entries inline
        if staging_keys:
            for key in staging_keys:
                self.dispatch_map.take_read(key)
            yield self.env.timeout(self.gpu.h2d_latency(size))
            for key in staging_keys:
                self.dispatch_map.release_read(key)
                self.dispatch_map.touch(key, self.env.now)
                self.metrics.record_lookup(self.env.now - start, hot=True, success=True)

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

            for drive_idx, cold_keys in cold_by_drive.items():
                for key in cold_keys:
                    yield self.env.process(self._evict_for_space(size, key))
                    self.memory_tier.insert(key, size, self.env.now)
                    entry = self.dispatch_map.lookup(key)
                    if entry:
                        entry.state = EntryState.MEMORY_TIER
                    self.dispatch_map.touch(key, self.env.now)
                    self.metrics.record_lookup(self.env.now - start, hot=False, success=True)

    # === Other Operations ===

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

    def promote_to_memory_tier(self, keys: list[int]) -> simpy.events.Process:
        """Promote SSD-resident entries to memory-tier (background, best-effort)."""
        return self.env.process(self._promote_to_memory_tier(keys))

    def _promote_to_memory_tier(self, keys: list[int]):
        for key in keys:
            entry = self.dispatch_map.lookup(key)
            if entry is None:
                continue

            if entry.state == EntryState.BLOCK_DEVICE:
                # Evict space
                evict_result = yield self.env.process(
                    self._evict_for_space(entry.size, key)
                )
                if evict_result != OpResult.SUCCESS:
                    continue

                # Memory tier insertion
                yield self.env.timeout(self.config.memory_tier_insert_us)
                inserted = self.memory_tier.insert(key, entry.size, self.env.now)
                if not inserted:
                    continue

                # SSD read
                num_segments = max(1, (entry.size + self.config.mdts_bytes - 1) // self.config.mdts_bytes)
                read_time = num_segments * self.config.nvme_read_latency_us
                yield self.env.timeout(read_time)

                # Update dispatch map
                entry.state = EntryState.MEMORY_TIER
                self.dispatch_map.touch(key, self.env.now)
                self.metrics.record_promotion()
            else:
                # Already in memory-tier or staging — just refresh timestamp
                self.dispatch_map.touch(key, self.env.now)

            # Backfill delay between promotions
            if self.config.backfill_delay_us > 0:
                yield self.env.timeout(self.config.backfill_delay_us)

    def clear_memory_tier(self) -> simpy.events.Process:
        """Evict all entries from memory-tier."""
        return self.env.process(self._clear_memory_tier())

    def _clear_memory_tier(self):
        cleared = 0
        entries = self.dispatch_map.entries_in_state(EntryState.MEMORY_TIER)
        for entry in entries:
            if entry.read_ref > 0 or entry.write_ref > 0:
                continue
            self.memory_tier.remove(entry.key)
            if entry.ssd_offset is not None:
                self.dispatch_map.convert_memory_tier_to_block(entry.key)
            else:
                self.dispatch_map.remove(entry.key)
            cleared += 1
        yield self.env.timeout(0.1 * cleared)
        return cleared

    def flush_to_ssd(self) -> simpy.events.Process:
        """Block until all pending write-through jobs complete."""
        return self.env.process(self._flush_to_ssd())

    def _flush_to_ssd(self):
        flushed = 0
        for drive_idx in range(self.config.num_drives):
            while self._write_queues[drive_idx]:
                job = self._write_queues[drive_idx].pop(0)
                entry = self.dispatch_map.lookup(job.key)
                if entry is None or entry.state != EntryState.MEMORY_TIER:
                    continue
                if entry.ssd_offset is not None:
                    continue

                result = self.ssd.allocate(job.key, job.size)
                if result is None:
                    continue

                alloc_drive_idx, offset = result
                num_segments = self.config.segments_per_entry()
                write_time = num_segments * self.config.nvme_write_latency_us
                yield self.env.timeout(write_time)

                self.dispatch_map.convert_to_storage(job.key, alloc_drive_idx, offset)
                flushed += 1
        return flushed

    # === Internal processes ===

    def _promote_and_serve(self, key: int, drive_idx: int, offset: int, size: int):
        """Promote a cold entry from SSD back to memory tier + GPU."""
        yield self.env.process(self._evict_for_space(size, key))

        yield self.env.timeout(self.config.memory_tier_insert_us)
        self.memory_tier.insert(key, size, self.env.now)

        promote_latency = self.pipeline.single_entry_promote_latency(size)
        yield self.env.timeout(promote_latency)

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

            evicted_entry = self.memory_tier.evict_lru_for_key(target_key)
            if evicted_entry is None:
                evicted_entry = self.memory_tier.evict_lru()
                if evicted_entry is None:
                    return OpResult.ALLOCATION_FAILED

            dm_entry = self.dispatch_map.lookup(evicted_entry.key)
            if dm_entry and dm_entry.ssd_offset is not None:
                self.dispatch_map.convert_memory_tier_to_block(evicted_entry.key)
                self.metrics.record_eviction(clean=True)
            else:
                self.dispatch_map.remove(evicted_entry.key)
                self.metrics.record_eviction(clean=False)

            yield self.env.timeout(0.1)

        return OpResult.SUCCESS

    def _background_write_through(self, drive_idx: int):
        """Per-drive background process that flushes memory-tier entries to SSD.

        Matches Rust's ParallelBackgroundWriter (one writer per drive).
        """
        while not self._shutdown:
            queue = self._write_queues[drive_idx]
            if not queue:
                yield self.env.timeout(10.0)
                continue

            job = queue.pop(0)

            # Check entry still exists in memory tier
            entry = self.dispatch_map.lookup(job.key)
            if entry is None or entry.state != EntryState.MEMORY_TIER:
                continue
            if entry.ssd_offset is not None:
                continue

            # Peek memory tier (no LRU refresh per spec)
            mt_entry = self.memory_tier.peek(job.key)
            if mt_entry is None:
                continue

            # Allocate SSD extent on this drive
            drive = self.ssd.drives[drive_idx]
            offset = drive.allocate(job.key, job.size)
            if offset is None:
                self.metrics.record_write_through(success=False)
                continue

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

                if entry.ssd_offset is not None and entry.drive_idx is not None:
                    self.ssd.free(entry.drive_idx, entry.ssd_offset)
                self.dispatch_map.remove(key)
                evicted += 1
                self.metrics.record_ssd_eviction()

    def shutdown(self):
        self._shutdown = True
