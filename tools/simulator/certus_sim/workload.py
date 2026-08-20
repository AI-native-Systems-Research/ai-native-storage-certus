"""Workload generators and trace replay.

Supports:
- Batch inference trace replay from JSONL files
- Synthetic workload generation for quick testing
- Mixed workloads with populate + direct-write + lookup phases
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

import simpy

from certus_sim.config import SimConfig
from certus_sim.server import Server


@dataclass
class BatchOp:
    """A single batch operation from a trace or generator."""
    op: str  # "populate", "lookup", "check", "remove", "touch", "prepare_store", "commit_store", "cancel_store", "promote"
    keys: list[int]
    size: int  # entry size in bytes
    time_us: float  # absolute simulation time to issue this op


def load_trace(path: str | Path) -> list[BatchOp]:
    """Load a JSONL trace file.

    Each line: {"op": "populate"|"lookup"|"prepare_store"|..., "keys": [1,2,3], "size": 131072, "time_us": 1000.0}
    """
    ops: list[BatchOp] = []
    with open(path, "r") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            record = json.loads(line)
            ops.append(BatchOp(
                op=record["op"],
                keys=record["keys"],
                size=record.get("size", 131072),
                time_us=record.get("time_us", 0.0),
            ))
    return sorted(ops, key=lambda o: o.time_us)


def generate_synthetic(
    num_populate: int,
    num_lookup: int,
    entry_size: int,
    key_space: int = 0,
    batch_size: int = 100,
    inter_batch_us: float = 1000.0,
    num_direct_write: int = 0,
) -> list[BatchOp]:
    """Generate a synthetic workload: populate phase, optional direct-write, then lookup.

    key_space: if > num_populate, lookups sample from a wider range (causing misses)
    num_direct_write: number of entries to write via prepare_store/commit_store path
    """
    import numpy as np

    if key_space == 0:
        key_space = num_populate + num_direct_write

    ops: list[BatchOp] = []
    time = 0.0

    # Populate phase: sequential keys in batches (GPU->DRAM->SSD path)
    for start in range(0, num_populate, batch_size):
        end = min(start + batch_size, num_populate)
        keys = list(range(start, end))
        ops.append(BatchOp(op="populate", keys=keys, size=entry_size, time_us=time))
        time += inter_batch_us

    # Direct-write phase: prepare_store + commit_store (GPU->SSD path)
    if num_direct_write > 0:
        dw_start_key = num_populate
        for start in range(0, num_direct_write, batch_size):
            end = min(start + batch_size, num_direct_write)
            keys = list(range(dw_start_key + start, dw_start_key + end))
            ops.append(BatchOp(op="prepare_store", keys=keys, size=entry_size, time_us=time))
            time += inter_batch_us
            ops.append(BatchOp(op="commit_store", keys=keys, size=entry_size, time_us=time))
            time += inter_batch_us

    # Lookup phase: random keys (Zipf-like access to get cache dynamics)
    rng = np.random.default_rng(42)
    remaining = num_lookup
    while remaining > 0:
        count = min(batch_size, remaining)
        zipf_keys = rng.zipf(1.5, size=count * 2)
        keys = list(dict.fromkeys(int(k % key_space) for k in zipf_keys))[:count]
        remaining -= len(keys)
        ops.append(BatchOp(op="lookup", keys=keys, size=entry_size, time_us=time))
        time += inter_batch_us

    return ops


class WorkloadDriver:
    """Drives a workload (trace or synthetic) against the control-plane server model."""

    def __init__(
        self,
        env: simpy.Environment,
        server: Server,
        config: SimConfig,
    ):
        self.env = env
        self.server = server
        self.config = config

    def run(self, ops: list[BatchOp]) -> simpy.events.Process:
        return self.env.process(self._run(ops))

    def _run(self, ops: list[BatchOp]):
        for op in ops:
            # Wait until the scheduled time
            if op.time_us > self.env.now:
                yield self.env.timeout(op.time_us - self.env.now)

            if op.op == "populate":
                yield self.server.handle_populate(op.keys, op.size)
            elif op.op == "lookup":
                yield self.server.handle_lookup(op.keys, op.size)
            elif op.op == "check":
                yield self.server.handle_check(op.keys)
            elif op.op == "remove":
                yield self.server.handle_remove(op.keys)
            elif op.op == "touch":
                yield self.server.handle_touch(op.keys)
            elif op.op == "prepare_store":
                yield self.server.handle_prepare_store(op.keys, op.size)
            elif op.op == "commit_store":
                yield self.server.handle_commit_store(op.keys)
            elif op.op == "cancel_store":
                yield self.server.handle_cancel_store(op.keys)
            elif op.op == "promote":
                yield self.server.handle_promote(op.keys)
