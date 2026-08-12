# SPDX-License-Identifier: Apache-2.0
"""Patch installed vLLM 0.26 with a narrow lookup_many scheduler hook.

The benchmark image starts from the official vLLM image. Do not copy whole
files from a local vLLM checkout into that image: nearby APIs can drift between
trees. This script edits only the small lookup batching surface needed by the
Certus connector and fails the build if the installed source does not match the
expected vLLM 0.26 shape.
"""

from __future__ import annotations

import importlib
from pathlib import Path


def _replace_once(text: str, old: str, new: str, path: Path) -> str:
    if old not in text:
        raise RuntimeError(f"expected patch marker not found in {path}")
    return text.replace(old, new, 1)


def _replace_once_or_verify(text: str, old: str, new: str, path: Path) -> str:
    if old in text:
        return text.replace(old, new, 1)
    if new in text:
        return text
    raise RuntimeError(f"expected patch marker not found in {path}")


def _ensure_collections_sequence(text: str, path: Path) -> str:
    if "from collections.abc import Collection, Iterable, Sequence" in text:
        return text
    if "from collections.abc import Iterable, Sequence" in text:
        return text
    if "from collections.abc import Collection, Iterable\n" in text:
        return text.replace(
            "from collections.abc import Collection, Iterable\n",
            "from collections.abc import Collection, Iterable, Sequence\n",
            1,
        )
    if "from collections.abc import Iterable\n" in text:
        return text.replace(
            "from collections.abc import Iterable\n",
            "from collections.abc import Iterable, Sequence\n",
            1,
        )
    raise RuntimeError(f"could not patch Sequence import in {path}")


def _patch_base(vllm_root: Path) -> None:
    path = vllm_root / "v1/kv_offload/base.py"
    text = path.read_text()
    text = _ensure_collections_sequence(text, path)

    if "    def lookup_many(\n" not in text:
        lookup_many = '''    def lookup_many(
        self, keys: Sequence[OffloadKey], req_context: ReqContext
    ) -> Sequence[LookupResult]:
        """
        Batch variant of lookup(). Managers can override this to collapse
        scheduler lookup probes into one backend request. The default preserves
        the public per-key behavior exactly.

        Args:
            keys: the keys identifying the blocks to lookup.
            req_context: per-request context (e.g. kv_transfer_params).

        Returns:
            One LookupResult per input key, in the same order.
        """
        return [self.lookup(key, req_context) for key in keys]

'''
        text = _replace_once(
            text,
            "    @abstractmethod\n    def prepare_load(\n",
            lookup_many + "    @abstractmethod\n    def prepare_load(\n",
            path,
        )

    path.write_text(text)


def _patch_scheduler(vllm_root: Path) -> None:
    path = (
        vllm_root
        / "distributed/kv_transfer/kv_connector/v1/offloading/scheduler.py"
    )
    text = path.read_text()
    text = _ensure_collections_sequence(text, path)

    if "    def _lookup_many(\n" not in text:
        lookup_many = '''    def _lookup_many(
        self, keys: Sequence[OffloadKey], req_context: ReqContext
    ) -> Sequence[LookupResult]:
        return self.manager.lookup_many(keys, req_context)

'''
        text = _replace_once(
            text,
            "    def _maximal_prefix_lookup(\n",
            lookup_many + "    def _maximal_prefix_lookup(\n",
            path,
        )

    text = _replace_once_or_verify(
        text,
        """        hit_count = 0
        defer_lookup = False
        for key in keys:
            match self.manager.lookup(key, req_context):
""",
        """        keys = list(keys)
        hit_count = 0
        defer_lookup = False
        for result in self._lookup_many(keys, req_context):
            match result:
""",
        path,
    )

    text = text.replace(
        """                case LookupResult.RETRY:
                    # Don't break: keep scanning to let manager kick off
                    # async lookups (until a miss is detected).
                    defer_lookup = True
""",
        """                case LookupResult.RETRY:
                    # Location uncertain: keep scanning so managers that defer
                    # lookup can make progress across the whole probe set.
                    defer_lookup = True
""",
        1,
    )

    text = _replace_once_or_verify(
        text,
        """        defer_lookup = False
        consecutive_hits = 0
        for idx in range(len(keys) - 1, -1, -1):
            match self.manager.lookup(keys[idx], req_context):
""",
        """        defer_lookup = False
        consecutive_hits = 0
        results = self._lookup_many(keys, req_context)
        for idx in range(len(keys) - 1, -1, -1):
            match results[idx]:
""",
        path,
    )

    text = text.replace(
        "                case LookupResult.RETRY:\n"
        "                    # Block location uncertain \u2014 does not count as hit.\n"
        "                    # Don't break: keep scanning to let manager kick off\n"
        "                    # async lookups.\n"
        "                    defer_lookup = True\n"
        "                    consecutive_hits = 0\n",
        """                case LookupResult.RETRY:
                    # Block location uncertain; keep scanning so deferred
                    # lookup managers can make progress across the probe set.
                    defer_lookup = True
                    consecutive_hits = 0
""",
        1,
    )

    if "self.manager.lookup(" in text:
        raise RuntimeError(f"unbatched manager.lookup remains in {path}")

    path.write_text(text)


def main() -> None:
    vllm_root = Path(importlib.import_module("vllm").__file__).parent
    _patch_base(vllm_root)
    _patch_scheduler(vllm_root)
    print(f"Patched vLLM lookup_many hook under {vllm_root}")


if __name__ == "__main__":
    main()
