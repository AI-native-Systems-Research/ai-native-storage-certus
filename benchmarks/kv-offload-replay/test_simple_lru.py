"""Unit tests for SimpleLRUTarget and trace utilities in replay_offloading_traces.py.

All tests run without hardware (no GPU / NVMe / SPDK required).
Run with:  pytest benchmarks/kv-offload-replay/test_simple_lru.py -v
"""

from __future__ import annotations

import gzip
import sys
import tempfile
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Allow importing without executing main()
# ---------------------------------------------------------------------------
REPLAY_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(REPLAY_DIR))

from replay_offloading_traces import (  # noqa: E402
    SimpleLRUTarget,
    PrepareStoreOutput,
    open_trace,
    _resolve_trace,
)


# ===========================================================================
# SimpleLRUTarget — basic operations
# ===========================================================================

class TestSimpleLRUTargetBasics:
    def test_lookup_miss_on_empty_cache(self):
        lru = SimpleLRUTarget(num_blocks=4)
        assert lru.lookup(["a", "b"]) == 0

    def test_complete_store_makes_key_findable(self):
        lru = SimpleLRUTarget(num_blocks=4)
        out = lru.prepare_store(["a"])
        assert out is not None
        lru.complete_store(["a"], success=True)
        assert lru.lookup(["a"]) == 1

    def test_lookup_prefix_stops_at_first_miss(self):
        lru = SimpleLRUTarget(num_blocks=4)
        for key in ["a", "b", "c"]:
            lru.prepare_store([key])
            lru.complete_store([key], success=True)
        # All three present → 3
        assert lru.lookup(["a", "b", "c"]) == 3
        # "x" not present → stops at 0
        assert lru.lookup(["x", "a", "b"]) == 0
        # "a" present, "x" absent → stops at 1
        assert lru.lookup(["a", "x", "b"]) == 1

    def test_complete_store_success_false_does_not_cache(self):
        lru = SimpleLRUTarget(num_blocks=4)
        out = lru.prepare_store(["k"])
        assert out is not None
        lru.complete_store(["k"], success=False)
        assert lru.lookup(["k"]) == 0


# ===========================================================================
# SimpleLRUTarget — eviction
# ===========================================================================

class TestSimpleLRUTargetEviction:
    def test_lru_evicts_oldest_entry_first(self):
        lru = SimpleLRUTarget(num_blocks=2)
        # Fill to capacity: a (oldest), b (newest)
        lru.prepare_store(["a"])
        lru.complete_store(["a"])
        lru.prepare_store(["b"])
        lru.complete_store(["b"])
        # Add c — should evict a (LRU)
        out = lru.prepare_store(["c"])
        assert out is not None
        assert "a" in out.block_hashes_evicted
        lru.complete_store(["c"])
        assert lru.lookup(["c"]) == 1
        assert lru.lookup(["a"]) == 0

    def test_touch_promotes_to_mru_prevents_eviction(self):
        lru = SimpleLRUTarget(num_blocks=2)
        lru.prepare_store(["a"])
        lru.complete_store(["a"])
        lru.prepare_store(["b"])
        lru.complete_store(["b"])
        # Touch a → a is now MRU, b becomes LRU
        lru.touch(["a"])
        out = lru.prepare_store(["c"])
        assert out is not None
        assert "b" in out.block_hashes_evicted
        assert "a" not in out.block_hashes_evicted

    def test_request_larger_than_capacity_returns_none(self):
        lru = SimpleLRUTarget(num_blocks=2)
        # Requesting 3 blocks when capacity is 2
        out = lru.prepare_store(["x", "y", "z"])
        assert out is None

    def test_evicts_exactly_enough_to_make_room(self):
        lru = SimpleLRUTarget(num_blocks=3)
        for k in ["a", "b", "c"]:
            lru.prepare_store([k])
            lru.complete_store([k])
        # Cache is full (3/3); store 2 more → should evict exactly 2
        out = lru.prepare_store(["d", "e"])
        assert out is not None
        assert len(out.block_hashes_evicted) == 2

    def test_already_cached_key_not_re_stored(self):
        lru = SimpleLRUTarget(num_blocks=4)
        lru.prepare_store(["a"])
        lru.complete_store(["a"])
        out = lru.prepare_store(["a"])
        # "a" is already cached — should not appear in to_store
        assert out is not None
        assert "a" not in out.block_hashes_to_store


# ===========================================================================
# SimpleLRUTarget — pending-set (in-flight stores)
# ===========================================================================

class TestSimpleLRUTargetPending:
    def test_pending_blocks_not_double_stored(self):
        lru = SimpleLRUTarget(num_blocks=4)
        out1 = lru.prepare_store(["a"])
        assert out1 is not None
        assert "a" in out1.block_hashes_to_store
        # Second prepare_store before complete_store — already pending
        out2 = lru.prepare_store(["a"])
        assert out2 is not None
        assert "a" not in out2.block_hashes_to_store

    def test_complete_store_clears_pending(self):
        lru = SimpleLRUTarget(num_blocks=2)
        lru.prepare_store(["a"])
        lru.complete_store(["a"])
        # After completion, 'a' should be cached and pending should be empty
        assert lru.lookup(["a"]) == 1
        assert "a" not in lru._pending

    def test_failed_complete_store_also_clears_pending(self):
        lru = SimpleLRUTarget(num_blocks=2)
        lru.prepare_store(["a"])
        lru.complete_store(["a"], success=False)
        # Even on failure, the key should no longer be in pending
        assert "a" not in lru._pending


# ===========================================================================
# SimpleLRUTarget — prepare_load
# ===========================================================================

class TestSimpleLRUTargetLoad:
    def test_prepare_load_raises_on_miss(self):
        lru = SimpleLRUTarget(num_blocks=4)
        with pytest.raises(KeyError):
            lru.prepare_load(["missing"])

    def test_prepare_load_succeeds_on_cached_key(self):
        lru = SimpleLRUTarget(num_blocks=4)
        lru.prepare_store(["k"])
        lru.complete_store(["k"])
        # Should not raise
        lru.prepare_load(["k"])

    def test_complete_load_is_a_no_op(self):
        lru = SimpleLRUTarget(num_blocks=4)
        lru.prepare_store(["k"])
        lru.complete_store(["k"])
        lru.prepare_load(["k"])
        # complete_load must not raise and should return None
        result = lru.complete_load(["k"])
        assert result is None


# ===========================================================================
# open_trace() — gz / plain detection
# ===========================================================================

class TestOpenTrace:
    def test_opens_plain_text_file(self):
        with tempfile.NamedTemporaryFile(suffix=".jsonl", mode="w", delete=False) as f:
            f.write('{"event": "test"}\n')
            path = Path(f.name)
        try:
            with open_trace(path) as fh:
                line = fh.readline()
            assert '"event"' in line
        finally:
            path.unlink(missing_ok=True)

    def test_opens_gzip_file(self):
        with tempfile.NamedTemporaryFile(suffix=".gz", delete=False) as f:
            gz_path = Path(f.name)
        try:
            with gzip.open(gz_path, "wt") as gz:
                gz.write('{"event": "gz-test"}\n')
            with open_trace(gz_path) as fh:
                line = fh.readline()
            assert '"event"' in line
        finally:
            gz_path.unlink(missing_ok=True)


# ===========================================================================
# _resolve_trace() — path resolution
# ===========================================================================

class TestResolveTrace:
    def test_resolves_plain_jsonl_files(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            prefix = Path(tmpdir) / "run"
            mgr_path = Path(str(prefix) + ".mgr.jsonl")
            handler_path = Path(str(prefix) + ".handler.jsonl")
            mgr_path.touch()
            handler_path.touch()

            mgr, handler = _resolve_trace(str(prefix))
            assert mgr == mgr_path
            assert handler == handler_path

    def test_resolves_gz_files(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            prefix = Path(tmpdir) / "run"
            mgr_path = Path(str(prefix) + ".mgr.jsonl.gz")
            handler_path = Path(str(prefix) + ".handler.jsonl.gz")
            mgr_path.touch()
            handler_path.touch()

            mgr, handler = _resolve_trace(str(prefix))
            assert mgr == mgr_path
            assert handler == handler_path

    def test_prefers_gz_over_plain(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            prefix = Path(tmpdir) / "run"
            for suffix in (".mgr.jsonl.gz", ".mgr.jsonl",
                           ".handler.jsonl.gz", ".handler.jsonl"):
                Path(str(prefix) + suffix).touch()

            mgr, handler = _resolve_trace(str(prefix))
            assert str(mgr).endswith(".gz")
            assert str(handler).endswith(".gz")

    def test_raises_on_missing_mgr_trace(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            prefix = Path(tmpdir) / "run"
            Path(str(prefix) + ".handler.jsonl").touch()
            with pytest.raises(FileNotFoundError, match="manager trace"):
                _resolve_trace(str(prefix))

    def test_raises_on_missing_handler_trace(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            prefix = Path(tmpdir) / "run"
            Path(str(prefix) + ".mgr.jsonl").touch()
            with pytest.raises(FileNotFoundError, match="handler trace"):
                _resolve_trace(str(prefix))

    def test_raises_on_completely_missing_trace(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            prefix = Path(tmpdir) / "nonexistent"
            with pytest.raises(FileNotFoundError):
                _resolve_trace(str(prefix))
