# SPDX-License-Identifier: Apache-2.0
"""certus-shmq-connector: a vLLM KV-offloading connector for Certus over a
shared-memory control transport (drop-in alternative to the gRPC connector).

Unlike the in-process ``certus-connector`` (which embeds a PyO3 Rust engine),
this package is a pure-Python client that talks to a running
``certus-server`` over a lock-free ``/dev/shm`` mailbox
(``certus_shmq_connector.ring``). vLLM offloads KV blocks by DMA over CUDA IPC
handles; the server owns the SPDK/NVMe + DRAM tiers. The ring carries only the
small control plane — the KV bytes never cross it.

vLLM wires this in via ``kv_connector_extra_config``:
``{"spec_name": "CertusShmqOffloadingSpec", "spec_module_path":
"certus_shmq_connector.spec", "shm_path": "/dev/shm/certus-shmq"}``.

Only the transport client (``Ring``) is re-exported here; the vLLM plumbing
(``spec``/``manager``/``handler``) is imported lazily by vLLM through
``spec_module_path`` so importing this package stays vLLM-free.
"""

from .ring import Ring, RingError

__version__ = "0.1.0"

__all__ = ["Ring", "RingError"]
