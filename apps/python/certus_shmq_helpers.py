# SPDX-License-Identifier: Apache-2.0
"""Shared shmq transport helpers for the apps/python benchmark + test scripts.

These scripts used to talk to the Certus dispatcher over gRPC (``dispatcher_pb2``
+ ``DispatcherStub``). gRPC has been removed; the sole control transport is now
the ``/dev/shm`` mailbox served by ``certus-server``, spoken by the pure-Python
``Ring`` client in ``certus_shmq_connector.ring``.

This module is the single place that:

  * locates the ``certus-shmq-connector`` package (it is a sibling of the repo
    root, not pip-installed for these standalone scripts — ``ring.py`` is
    dependency-light, so a ``sys.path`` insert is enough and avoids pulling the
    connector's heavy vLLM/torch runtime deps just to run a benchmark), and
  * builds the single-region ``HandleBatch`` entries that ``populate`` /
    ``lookup`` / ``copy_to_store`` take, so the fragile CUDA-IPC-handle framing
    lives in exactly one spot instead of being re-spelled in twelve scripts.

Concurrency note: the ``Ring`` client is one-request-in-flight *per channel*, and
each calling thread claims its own channel on first use. The gRPC scripts got
pipelining from ``stub.X.future(req)`` (many in-flight on one channel); the shmq
equivalent is a ``ThreadPoolExecutor`` whose worker count is the pipeline depth,
each worker submitting a blocking ``ring.X(...)`` call. The server must expose
at least that many ``--channels`` or the extra threads get no channel and error.
"""

from __future__ import annotations

import argparse
import os
import sys
from typing import Sequence

# ── locate + import the connector's Ring client ─────────────────────────────

_THIS_DIR = os.path.dirname(os.path.abspath(__file__))
# apps/python -> repo root -> certus-shmq-connector
_CONNECTOR_ROOT = os.path.normpath(
    os.path.join(_THIS_DIR, "..", "..", "certus-shmq-connector")
)
if _CONNECTOR_ROOT not in sys.path:
    sys.path.insert(0, _CONNECTOR_ROOT)

from certus_shmq_connector.ring import Ring, RingError  # noqa: E402

DEFAULT_SHM_PATH = "/dev/shm/certus-shmq"

__all__ = [
    "Ring",
    "RingError",
    "DEFAULT_SHM_PATH",
    "add_shm_arg",
    "connect",
    "single_region",
    "single_region_entries",
]


def add_shm_arg(parser: argparse.ArgumentParser) -> None:
    """Add the standard ``--shm-path`` argument (replaces the old ``--server``)."""
    parser.add_argument(
        "--shm-path",
        default=DEFAULT_SHM_PATH,
        help=f"certus-server shmq mailbox path (default: {DEFAULT_SHM_PATH})",
    )


def connect(shm_path: str, *, ready_timeout: float = 30.0, **kw) -> Ring:
    """Attach to the server's shmq mailbox, blocking up to ``ready_timeout`` for
    the server to publish its ready magic. Extra kwargs pass through to ``Ring``."""
    return Ring(shm_path, ready_timeout=ready_timeout, **kw)


def single_region(handle: bytes, gpu_device: int, size: int, offset: int = 0):
    """One ``(handle, gpu_device_id, offset, size)`` region tuple.

    ``populate``/``lookup`` take exactly one region per key (a single CUDA IPC
    handle); ``copy_to_store`` may take several. This builds the one-region case.
    """
    return (handle, gpu_device, offset, size)


def single_region_entries(items: Sequence[tuple], gpu_device: int):
    """Build ``[(key, [region])]`` entries from ``[(key, handle, size)]`` (or
    ``[(key, handle, size, offset)]``) for a single-region op."""
    entries = []
    for it in items:
        if len(it) == 4:
            key, handle, size, offset = it
        else:
            key, handle, size = it
            offset = 0
        entries.append((key, [single_region(handle, gpu_device, size, offset)]))
    return entries
