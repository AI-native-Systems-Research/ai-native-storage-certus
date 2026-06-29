"""Certus Server Discrete Event Simulator.

Models the certus-server two-tier GPU cache system (DRAM memory-tier + NVMe SSDs)
using SimPy for discrete event simulation with realistic pipeline stage timing.

Matches the Rust IDispatcher trait with:
- Three-phase populate (reserve_memory / populate_memory / memory_populated)
- Direct-write path (prepare_store / commit_store / cancel_store)
- Staging / MemoryTier / BlockDevice entry states
- Per-drive background write-through
- Pipelined SSD->DRAM->GPU cold promotion
"""

from certus_sim.config import SimConfig

__all__ = ["SimConfig"]
