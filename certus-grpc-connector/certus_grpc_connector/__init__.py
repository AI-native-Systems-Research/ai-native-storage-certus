# SPDX-License-Identifier: Apache-2.0
"""Certus gRPC OffloadingSpec connector for vLLM KV cache offloading.

Unlike the in-process ``certus-connector`` (which embeds a PyO3 Rust engine),
this package is a pure-Python gRPC client that talks to a running
``certus-server``. vLLM offloads KV blocks by DMA over CUDA IPC handles; the
server owns the SPDK/NVMe + DRAM tiers.
"""

__version__ = "0.1.0"
