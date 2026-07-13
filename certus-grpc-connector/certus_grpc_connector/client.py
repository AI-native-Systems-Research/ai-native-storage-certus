# SPDX-License-Identifier: Apache-2.0
"""gRPC channel/stub factory and small helpers for the Certus dispatcher API."""

from __future__ import annotations

import grpc

from . import dispatcher_pb2 as pb
from . import dispatcher_pb2_grpc as pb_grpc

# KV blocks can be multiple MiB; allow large gRPC messages (matches the
# apps/python test clients).
_MAX_MSG_BYTES = 256 * 1024 * 1024


def make_stub(server: str) -> tuple[grpc.Channel, "pb_grpc.DispatcherStub"]:
    """Open an insecure channel to ``server`` (host:port) and return (channel, stub)."""
    channel = grpc.insecure_channel(
        server,
        options=[
            ("grpc.max_send_message_length", _MAX_MSG_BYTES),
            ("grpc.max_receive_message_length", _MAX_MSG_BYTES),
        ],
    )
    return channel, pb_grpc.DispatcherStub(channel)


def all_success(results) -> bool:
    """True if every EntryResult in ``results`` reports success."""
    return all(r.success for r in results)
