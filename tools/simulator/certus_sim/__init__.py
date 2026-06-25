"""Certus Server Discrete Event Simulator.

Models the certus-server two-tier GPU cache system (DRAM memory-tier + NVMe SSDs)
using SimPy for discrete event simulation with realistic pipeline stage timing.
"""

from certus_sim.config import SimConfig

__all__ = ["SimConfig"]
