# SPDX-License-Identifier: Apache-2.0
"""Native OffloadingManager backed by the Rust certus_native engine.

This is the production manager — all index, allocation, and eviction logic
lives in Rust (dispatch-map, extent-manager, dispatcher). Python is a thin
adapter converting vLLM's OffloadKey (bytes) to u64 and constructing the
PrepareStoreOutput that vLLM expects.
"""

from __future__ import annotations

from collections.abc import Iterable

from vllm.v1.kv_offload.abstract import (
    LoadStoreSpec,
    OffloadingEvent,
    OffloadingManager,
    OffloadKey,
    PrepareStoreOutput,
)

from certus_connector.mediums import BlockLocation, CertusLoadStoreSpec

try:
    import certus_native
except ImportError:
    certus_native = None


def _key_to_u64(key: OffloadKey) -> int:
    """Convert an OffloadKey (bytes) to a u64 for the Rust engine."""
    if isinstance(key, int):
        return key
    return int.from_bytes(key[:8], "big")


def _keys_to_u64s(keys: Iterable[OffloadKey]) -> list[int]:
    return [_key_to_u64(k) for k in keys]


class NativeCertusOffloadingManager(OffloadingManager):
    """Production manager delegating to the Rust certus_native engine.

    All state (index, LRU, allocation) lives in Rust. This class only
    adapts between vLLM's Python types and the native u64-keyed API.
    """

    def __init__(self, engine: "certus_native.CertusEngine"):
        if certus_native is None:
            raise ImportError(
                "certus_native is not installed. "
                "Build with: maturin develop -p certus-native"
            )
        self._engine = engine
        self._events: list[OffloadingEvent] = []

    def lookup(self, key: OffloadKey, req_context=None) -> bool | None:
        int_key = _key_to_u64(key)
        count = self._engine.batch_check([int_key])
        result = count > 0
        print(f"[MGR] lookup key={int_key} -> {result}", flush=True)
        return result

    def prepare_load(self, keys: Iterable[OffloadKey], req_context=None) -> LoadStoreSpec:
        int_keys = _keys_to_u64s(keys)
        print(f"[MGR] prepare_load n={len(int_keys)}", flush=True)
        self._engine.prepare_load(int_keys)
        locations = [BlockLocation(nvme_slab=k, dram_slot=None) for k in int_keys]
        return CertusLoadStoreSpec(locations)

    def touch(self, keys: Iterable[OffloadKey]) -> None:
        int_keys = _keys_to_u64s(keys)
        print(f"[MGR] touch n={len(int_keys)}", flush=True)
        self._engine.touch(int_keys)

    def complete_load(self, keys: Iterable[OffloadKey]) -> None:
        int_keys = _keys_to_u64s(keys)
        print(f"[MGR] complete_load n={len(int_keys)}", flush=True)
        self._engine.complete_load(int_keys)

    def prepare_store(self, keys: Iterable[OffloadKey], req_context=None) -> PrepareStoreOutput | None:
        keys_list = list(keys)
        int_keys = _keys_to_u64s(keys_list)
        print(f"[MGR] prepare_store n={len(int_keys)}", flush=True)

        result = self._engine.prepare_store(int_keys)
        if result is None:
            print(f"[MGR] prepare_store -> None (rejected)", flush=True)
            return None

        to_store_ints, evicted_ints = result
        print(f"[MGR] prepare_store -> store={len(to_store_ints)} evict={len(evicted_ints)}", flush=True)

        to_store_keys = [keys_list[int_keys.index(k)] for k in to_store_ints]
        evicted_keys = [
            k.to_bytes(8, "big") for k in evicted_ints
        ]

        locations = [
            BlockLocation(nvme_slab=k, dram_slot=None) for k in to_store_ints
        ]

        if evicted_keys:
            self._events.append(
                OffloadingEvent(
                    keys=evicted_keys,
                    medium=CertusLoadStoreSpec.medium(),
                    removed=True,
                )
            )

        return PrepareStoreOutput(
            keys_to_store=to_store_keys,
            store_spec=CertusLoadStoreSpec(locations),
            evicted_keys=evicted_keys,
        )

    def complete_store(self, keys: Iterable[OffloadKey], success: bool = True) -> None:
        int_keys = _keys_to_u64s(keys)
        print(f"[MGR] complete_store n={len(int_keys)} success={success}", flush=True)
        self._engine.complete_store(int_keys, success)

    def take_events(self) -> Iterable[OffloadingEvent]:
        yield from self._events
        self._events.clear()

    def shutdown(self) -> None:
        self._engine.shutdown()
