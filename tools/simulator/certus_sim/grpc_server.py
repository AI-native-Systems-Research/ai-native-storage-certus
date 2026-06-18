"""gRPC server model: request serialization and batch processing.

Models spec FR-013 (Mutex serialization) and FR-015 (duplicate key rejection).
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import simpy

from certus_sim.config import SimConfig
from certus_sim.dispatcher import Dispatcher, OpResult

if TYPE_CHECKING:
    from certus_sim.metrics import Metrics


class GrpcServer:
    """Models the certus-server gRPC layer with request serialization."""

    def __init__(
        self,
        env: simpy.Environment,
        config: SimConfig,
        dispatcher: Dispatcher,
        metrics: "Metrics",
    ):
        self.env = env
        self.config = config
        self.dispatcher = dispatcher
        self.metrics = metrics
        self._mutex = simpy.Resource(env, capacity=1)

    def handle_populate(self, keys: list[int], size: int) -> simpy.events.Process:
        return self.env.process(self._handle_populate(keys, size))

    def _handle_populate(self, keys: list[int], size: int):
        yield self.env.timeout(self.config.grpc_overhead_us)

        if len(keys) != len(set(keys)):
            return

        req = self._mutex.request()
        yield req
        try:
            for key in keys:
                yield self.dispatcher.populate(key, size)
        finally:
            self._mutex.release(req)

    def handle_lookup(self, keys: list[int], size: int) -> simpy.events.Process:
        return self.env.process(self._handle_lookup(keys, size))

    def _handle_lookup(self, keys: list[int], size: int):
        yield self.env.timeout(self.config.grpc_overhead_us)

        if len(keys) != len(set(keys)):
            return

        req = self._mutex.request()
        yield req
        try:
            yield self.dispatcher.batch_lookup(keys, size)
        finally:
            self._mutex.release(req)

    def handle_check(self, keys: list[int]) -> simpy.events.Process:
        return self.env.process(self._handle_check(keys))

    def _handle_check(self, keys: list[int]):
        yield self.env.timeout(self.config.grpc_overhead_us)

        req = self._mutex.request()
        yield req
        try:
            yield self.env.timeout(self.config.dispatch_map_op_us * len(keys))
            for key in keys:
                self.dispatcher.check(key)
        finally:
            self._mutex.release(req)

    def handle_remove(self, keys: list[int]) -> simpy.events.Process:
        return self.env.process(self._handle_remove(keys))

    def _handle_remove(self, keys: list[int]):
        yield self.env.timeout(self.config.grpc_overhead_us)

        if len(keys) != len(set(keys)):
            return

        req = self._mutex.request()
        yield req
        try:
            for key in keys:
                yield self.dispatcher.remove(key)
        finally:
            self._mutex.release(req)

    def handle_touch(self, keys: list[int]) -> simpy.events.Process:
        return self.env.process(self._handle_touch(keys))

    def _handle_touch(self, keys: list[int]):
        yield self.env.timeout(self.config.grpc_overhead_us)

        req = self._mutex.request()
        yield req
        try:
            yield self.env.timeout(self.config.dispatch_map_op_us * len(keys))
            for key in keys:
                self.dispatcher.touch(key)
        finally:
            self._mutex.release(req)
