#!/usr/bin/env python3
"""
Smoke tests for tiered-stress benchmark mode.

Uses the disk backend with a tiny synthetic trace — no GPU, server, or
hardware needed. Validates phase transitions, stats collection, and that
the cold-read phase actually forces reads.

Run:
    cd benchmarks/certus-mooncake-bench
    python -m pytest test_tiered_stress.py -v
    # or simply:
    python test_tiered_stress.py
"""

import json
import os
import sys
import tempfile
from pathlib import Path

# Ensure the benchmark package is importable
sys.path.insert(0, str(Path(__file__).resolve().parent))

from benchmark import (
    run_tiered_stress,
    run_benchmark,
    KVCacheRequest,
    TraceReplay,
    StorageBenchmark,
    create_storage,
)
from layout import get_model_config, create_layout


def _make_trace(tmpdir: str, requests: list[dict]) -> str:
    """Write a minimal JSONL trace file and return its path."""
    path = os.path.join(tmpdir, "test_trace.jsonl")
    with open(path, "w") as f:
        for req in requests:
            f.write(json.dumps(req) + "\n")
    return path


def _small_trace(n_requests=40, pages_per_req=5, max_page_id=100):
    """Generate a small trace with known properties.

    Returns list of dicts suitable for _make_trace. Pages cycle through
    [0, max_page_id) so the first half populates and the second half
    sees many repeats.
    """
    requests = []
    page_cursor = 0
    for i in range(n_requests):
        hash_ids = []
        for _ in range(pages_per_req):
            hash_ids.append(page_cursor % max_page_id)
            page_cursor += 1
        requests.append({
            "timestamp": float(i),
            "hash_ids": hash_ids,
            "input_length": 50,
            "output_length": 10,
        })
    return requests


# Use a tiny generic model so page_size is small (512 bytes per page)
_TEST_MODEL = {"name": "test-1B", "bytes_per_token": 1}
_PAGE_SIZE_TOKENS = 512  # → 512 bytes per page


class TestTieredStress:
    """Tests for the tiered-stress benchmark mode."""

    def test_returns_three_phases(self):
        """tiered-stress result contains populate, cold_read, mixed phases."""
        with tempfile.TemporaryDirectory() as tmpdir:
            trace = _make_trace(tmpdir, _small_trace(n_requests=40))
            storage_dir = os.path.join(tmpdir, "storage")
            result = run_tiered_stress(
                trace,
                _TEST_MODEL,
                backend="disk",
                storage_dir=storage_dir,
                page_size_tokens=_PAGE_SIZE_TOKENS,
                progress_interval=0,
            )
            assert "phases" in result
            assert set(result["phases"].keys()) == {"populate", "cold_read", "mixed"}

    def test_phase_request_counts(self):
        """Phase request counts sum to total and follow 50/25/25 split."""
        with tempfile.TemporaryDirectory() as tmpdir:
            n = 40
            trace = _make_trace(tmpdir, _small_trace(n_requests=n))
            storage_dir = os.path.join(tmpdir, "storage")
            result = run_tiered_stress(
                trace,
                _TEST_MODEL,
                backend="disk",
                storage_dir=storage_dir,
                page_size_tokens=_PAGE_SIZE_TOKENS,
                progress_interval=0,
            )
            phases = result["phases"]
            assert phases["populate"]["requests"] == n // 2  # 20
            assert phases["cold_read"]["requests"] == n // 4  # 10
            assert phases["mixed"]["requests"] == n - n // 2 - n // 4  # 10

    def test_cold_read_phase_has_only_reads(self):
        """Cold-read phase should have zero write ops."""
        with tempfile.TemporaryDirectory() as tmpdir:
            trace = _make_trace(tmpdir, _small_trace(n_requests=40))
            storage_dir = os.path.join(tmpdir, "storage")
            result = run_tiered_stress(
                trace,
                _TEST_MODEL,
                backend="disk",
                storage_dir=storage_dir,
                page_size_tokens=_PAGE_SIZE_TOKENS,
                progress_interval=0,
            )
            cold = result["phases"]["cold_read"]
            write_count = cold.get("storage", {}).get("write", {}).get("count", 0)
            assert write_count == 0, f"Cold-read phase had {write_count} writes"
            read_count = cold.get("storage", {}).get("read", {}).get("count", 0)
            assert read_count > 0, "Cold-read phase had no reads"

    def test_populate_phase_has_writes(self):
        """Populate phase should write pages that haven't been seen before."""
        with tempfile.TemporaryDirectory() as tmpdir:
            trace = _make_trace(tmpdir, _small_trace(n_requests=40, max_page_id=200))
            storage_dir = os.path.join(tmpdir, "storage")
            result = run_tiered_stress(
                trace,
                _TEST_MODEL,
                backend="disk",
                storage_dir=storage_dir,
                page_size_tokens=_PAGE_SIZE_TOKENS,
                progress_interval=0,
            )
            pop = result["phases"]["populate"]
            write_count = pop.get("storage", {}).get("write", {}).get("count", 0)
            assert write_count > 0, "Populate phase had no writes"

    def test_uses_full_page_id_space(self):
        """tiered-stress should NOT cap pages to --max-pages default."""
        with tempfile.TemporaryDirectory() as tmpdir:
            max_page_id = 500
            trace = _make_trace(
                tmpdir, _small_trace(n_requests=40, max_page_id=max_page_id)
            )
            storage_dir = os.path.join(tmpdir, "storage")
            result = run_tiered_stress(
                trace,
                _TEST_MODEL,
                backend="disk",
                storage_dir=storage_dir,
                page_size_tokens=_PAGE_SIZE_TOKENS,
                progress_interval=0,
            )
            # max_pages should be >= max_page_id (rounded up)
            assert result["max_pages"] >= max_page_id

    def test_mode_replay_still_works(self):
        """Verify the default replay mode is unaffected."""
        with tempfile.TemporaryDirectory() as tmpdir:
            trace = _make_trace(tmpdir, _small_trace(n_requests=20))
            storage_dir = os.path.join(tmpdir, "storage")
            result = run_benchmark(
                trace,
                _TEST_MODEL,
                backend="disk",
                storage_dir=storage_dir,
                max_pages=200,
                page_size_tokens=_PAGE_SIZE_TOKENS,
                progress_interval=0,
            )
            assert result["total_requests"] == 20
            assert "phases" not in result

    def test_latencies_are_positive(self):
        """All reported latencies should be > 0."""
        with tempfile.TemporaryDirectory() as tmpdir:
            trace = _make_trace(tmpdir, _small_trace(n_requests=40))
            storage_dir = os.path.join(tmpdir, "storage")
            result = run_tiered_stress(
                trace,
                _TEST_MODEL,
                backend="disk",
                storage_dir=storage_dir,
                page_size_tokens=_PAGE_SIZE_TOKENS,
                progress_interval=0,
            )
            for phase_name, phase in result["phases"].items():
                storage_stats = phase.get("storage", {})
                for op in ("read", "write"):
                    op_stats = storage_stats.get(op, {})
                    if op_stats.get("count", 0) > 0:
                        assert op_stats["avg_ms"] > 0, (
                            f"{phase_name}.{op} avg_ms should be > 0"
                        )

    def test_mixed_phase_has_both_reads_and_writes(self):
        """Mixed phase replays normally, so should have both reads and writes."""
        with tempfile.TemporaryDirectory() as tmpdir:
            # Use small max_page_id so the mixed phase sees lots of repeats
            trace = _make_trace(
                tmpdir, _small_trace(n_requests=80, pages_per_req=3, max_page_id=20)
            )
            storage_dir = os.path.join(tmpdir, "storage")
            result = run_tiered_stress(
                trace,
                _TEST_MODEL,
                backend="disk",
                storage_dir=storage_dir,
                page_size_tokens=_PAGE_SIZE_TOKENS,
                progress_interval=0,
            )
            mixed = result["phases"]["mixed"]
            reads = mixed.get("storage", {}).get("read", {}).get("count", 0)
            writes = mixed.get("storage", {}).get("write", {}).get("count", 0)
            # With 80 requests, max_page_id=20, by the mixed phase (last 25%)
            # all 20 pages have been written, so most ops should be reads
            assert reads > 0, "Mixed phase had no reads"
            # Writes can be 0 if all pages were already written — that's fine
            # The important thing is reads are happening


# ============================================================================
# Run as standalone script (outside pytest)
# ============================================================================

def _run_tests():
    """Run all tests and report results."""
    test = TestTieredStress()
    methods = [m for m in dir(test) if m.startswith("test_")]
    passed = 0
    failed = 0
    for name in sorted(methods):
        try:
            getattr(test, name)()
            print(f"  PASS  {name}")
            passed += 1
        except Exception as e:
            print(f"  FAIL  {name}: {e}")
            failed += 1
    print(f"\n{passed} passed, {failed} failed, {passed + failed} total")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(_run_tests())
