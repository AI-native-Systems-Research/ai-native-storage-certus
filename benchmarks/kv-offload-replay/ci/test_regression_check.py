"""Unit tests for ci/regression_check.py.

All tests run without hardware (no NVMe / GPU / SPDK needed).
Run with:  pytest benchmarks/kv-offload-replay/ci/test_regression_check.py -v
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

# ---------------------------------------------------------------------------
# Allow importing regression_check without executing main()
# ---------------------------------------------------------------------------
CI_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(CI_DIR))

import regression_check as rc  # noqa: E402


# ===========================================================================
# Helpers
# ===========================================================================

def _fake_results(throughput_mbps: float, hit_ratio: float = 0.70) -> dict:
    """Build a minimal results dict matching what run_replay() returns."""
    return {
        "handler": {"throughput_mbps": throughput_mbps},
        "manager": {"lookup": {"hit_ratio": hit_ratio}},
    }


def _make_baselines(connector: str = "certus",
                    throughput_mbps: float = 300.0,
                    tolerance_pct: int = 15) -> dict:
    return {
        connector: {
            "throughput_mbps": throughput_mbps,
            "tolerance_pct": tolerance_pct,
        }
    }


# ===========================================================================
# check_regression()
# ===========================================================================

class TestCheckRegression:
    """Tests for the core pass/fail decision logic."""

    def test_pass_well_above_lower_bound(self):
        baselines = _make_baselines(throughput_mbps=300.0, tolerance_pct=15)
        # lower bound = 300 * 0.85 = 255.0; 308 > 255 → PASS
        assert rc.check_regression("certus", _fake_results(308.0), baselines) is True

    def test_pass_exactly_at_lower_bound(self):
        baselines = _make_baselines(throughput_mbps=300.0, tolerance_pct=15)
        # lower bound = 255.0 exactly
        assert rc.check_regression("certus", _fake_results(255.0), baselines) is True

    def test_fail_one_mb_below_lower_bound(self):
        baselines = _make_baselines(throughput_mbps=300.0, tolerance_pct=15)
        # lower bound = 255.0; 254.9 < 255 → FAIL
        assert rc.check_regression("certus", _fake_results(254.9), baselines) is False

    def test_fail_significantly_below_baseline(self):
        baselines = _make_baselines(throughput_mbps=300.0, tolerance_pct=15)
        assert rc.check_regression("certus", _fake_results(100.0), baselines) is False

    def test_missing_baseline_returns_false(self, capsys):
        # No baseline for 'certus' → should return False (not raise)
        result = rc.check_regression("certus", _fake_results(300.0), {})
        assert result is False
        captured = capsys.readouterr()
        assert "no local baseline" in captured.out

    def test_pass_faster_than_baseline(self):
        baselines = _make_baselines(throughput_mbps=300.0, tolerance_pct=15)
        # Faster than baseline always passes
        assert rc.check_regression("certus", _fake_results(500.0), baselines) is True

    def test_zero_tolerance_pct_means_exact_match_needed(self):
        baselines = _make_baselines(throughput_mbps=300.0, tolerance_pct=0)
        # lower bound = 300 * 1.0 = 300.0; 299.9 < 300 → FAIL
        assert rc.check_regression("certus", _fake_results(299.9), baselines) is False
        # Exactly 300 → PASS
        assert rc.check_regression("certus", _fake_results(300.0), baselines) is True

    def test_prints_pass_status(self, capsys):
        baselines = _make_baselines(throughput_mbps=300.0, tolerance_pct=15)
        rc.check_regression("certus", _fake_results(308.0), baselines)
        captured = capsys.readouterr()
        assert "PASS" in captured.out

    def test_prints_fail_status(self, capsys):
        baselines = _make_baselines(throughput_mbps=300.0, tolerance_pct=15)
        rc.check_regression("certus", _fake_results(100.0), baselines)
        captured = capsys.readouterr()
        assert "FAIL" in captured.out


# ===========================================================================
# calibrate()
# ===========================================================================

class TestCalibrate:
    """Tests for baseline persistence and hardware capability warning."""

    def _reference_baselines(self) -> dict:
        return {
            "certus": {"throughput_mbps": 305.0, "hit_ratio": 0.70, "tolerance_pct": 15}
        }

    def test_saves_baseline_file(self):
        results = _fake_results(300.0, hit_ratio=0.71)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "baselines.json"
            rc.calibrate("certus", results, self._reference_baselines(), path)
            assert path.exists()
            saved = json.loads(path.read_text())
            assert "certus" in saved
            assert saved["certus"]["throughput_mbps"] == pytest.approx(300.0)
            assert saved["certus"]["hit_ratio"] == pytest.approx(0.71)
            assert saved["certus"]["tolerance_pct"] == 15

    def test_returns_true_when_machine_is_fast_enough(self):
        # 300 / 305 = 98.4% — well within 25% threshold
        results = _fake_results(300.0)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "baselines.json"
            ok = rc.calibrate("certus", results, self._reference_baselines(), path)
        assert ok is True

    def test_returns_false_when_machine_is_too_slow(self, capsys):
        # 200 / 305 = 34.4% slower than reference → exceeds 25% threshold → False
        results = _fake_results(200.0)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "baselines.json"
            ok = rc.calibrate("certus", results, self._reference_baselines(), path)
        assert ok is False
        captured = capsys.readouterr()
        assert "WARNING" in captured.out

    def test_warn_threshold_is_25_pct(self, capsys):
        # 305 * 0.74 = 225.7 — just over 26% deficit → should warn (False)
        slow = 305.0 * 0.74
        results = _fake_results(slow)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "baselines.json"
            ok = rc.calibrate("certus", results, self._reference_baselines(), path)
        assert ok is False

        # 305 * 0.76 = 231.8 — 24% deficit → ok (no warning, True)
        fast_enough = 305.0 * 0.76
        results2 = _fake_results(fast_enough)
        with tempfile.TemporaryDirectory() as tmpdir2:
            path2 = Path(tmpdir2) / "baselines.json"
            ok2 = rc.calibrate("certus", results2, self._reference_baselines(), path2)
        assert ok2 is True

    def test_returns_true_when_no_reference_entry_for_connector(self):
        # No reference for this connector → no warning fires
        results = _fake_results(50.0)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "baselines.json"
            ok = rc.calibrate("certus", results, {}, path)
        assert ok is True

    def test_overwrites_existing_baseline(self):
        results1 = _fake_results(300.0, hit_ratio=0.70)
        results2 = _fake_results(310.0, hit_ratio=0.72)
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "baselines.json"
            rc.calibrate("certus", results1, self._reference_baselines(), path)
            rc.calibrate("certus", results2, self._reference_baselines(), path)
            saved = json.loads(path.read_text())
        assert saved["certus"]["throughput_mbps"] == pytest.approx(310.0)

    def test_preserves_other_connectors_on_update(self):
        existing = {"cpu": {"throughput_mbps": 20000.0, "hit_ratio": 0.98, "tolerance_pct": 10}}
        with tempfile.TemporaryDirectory() as tmpdir:
            path = Path(tmpdir) / "baselines.json"
            path.write_text(json.dumps(existing))
            results = _fake_results(300.0)
            rc.calibrate("certus", results, self._reference_baselines(), path)
            saved = json.loads(path.read_text())
        assert "cpu" in saved, "existing connector entry should be preserved"
        assert "certus" in saved, "new connector entry should be added"


# ===========================================================================
# run_replay()
# ===========================================================================

class TestRunReplay:
    """Tests for subprocess invocation and JSON result parsing."""

    def _good_results(self) -> dict:
        return {
            "handler": {"throughput_mbps": 305.0},
            "manager": {"lookup": {"hit_ratio": 0.70}},
        }

    def test_returns_parsed_json_on_success(self):
        expected = self._good_results()
        with tempfile.TemporaryDirectory() as tmpdir:
            output_file = Path(tmpdir) / "result.json"

            def _run(cmd, **kw):
                out_path = cmd[cmd.index("--output-json") + 1]
                Path(out_path).write_text(json.dumps(expected))

            with patch("regression_check.subprocess.run", side_effect=_run):
                with patch("regression_check.tempfile.NamedTemporaryFile") as ntf:
                    ntf.return_value.__enter__.return_value.name = str(output_file)
                    ntf.return_value.__exit__ = MagicMock(return_value=False)
                    result = rc.run_replay("certus", "trace/foo", 32768)

        assert result == expected

    def test_returns_none_when_output_file_missing(self):
        """If the replay script exits without writing the JSON, return None."""
        with patch("regression_check.subprocess.run"):
            with patch("regression_check.tempfile.NamedTemporaryFile") as ntf:
                ntf.return_value.__enter__.return_value.name = "/nonexistent/path/result.json"
                ntf.return_value.__exit__ = MagicMock(return_value=False)
                result = rc.run_replay("certus", "trace/foo", 32768)
        assert result is None

    def test_returns_none_on_malformed_json(self):
        """If the output file contains invalid JSON, return None."""
        with tempfile.TemporaryDirectory() as tmpdir:
            bad_file = Path(tmpdir) / "result.json"
            bad_file.write_text("not valid json{{{{")

            with patch("regression_check.subprocess.run"):
                with patch("regression_check.tempfile.NamedTemporaryFile") as ntf:
                    ntf.return_value.__enter__.return_value.name = str(bad_file)
                    ntf.return_value.__exit__ = MagicMock(return_value=False)
                    result = rc.run_replay("certus", "trace/foo", 32768)
        assert result is None


# ===========================================================================
# baselines.json schema validation
# ===========================================================================

class TestBaselineSchema:
    """Validates the committed reference baselines.json."""

    REQUIRED_CONNECTORS = ("certus", "cpu", "fs")
    REQUIRED_KEYS = ("throughput_mbps", "hit_ratio", "tolerance_pct")

    @pytest.fixture
    def reference(self) -> dict:
        path = CI_DIR / "baselines.json"
        assert path.exists(), f"baselines.json not found at {path}"
        return json.loads(path.read_text())

    def test_all_connectors_present(self, reference):
        for connector in self.REQUIRED_CONNECTORS:
            assert connector in reference, f"connector '{connector}' missing from baselines.json"

    def test_all_required_keys_present(self, reference):
        for connector, entry in reference.items():
            for key in self.REQUIRED_KEYS:
                assert key in entry, (
                    f"key '{key}' missing for connector '{connector}' in baselines.json"
                )

    def test_throughput_values_are_positive(self, reference):
        for connector, entry in reference.items():
            assert entry["throughput_mbps"] > 0, (
                f"throughput_mbps must be positive for '{connector}'"
            )

    def test_tolerance_pct_in_valid_range(self, reference):
        for connector, entry in reference.items():
            assert 0 < entry["tolerance_pct"] <= 50, (
                f"tolerance_pct out of range for '{connector}': {entry['tolerance_pct']}"
            )

    def test_certus_throughput_is_reasonable(self, reference):
        # Certus is CXL DRAM — expect hundreds of MB/s (not GB/s like CPU DRAM)
        certus_tp = reference["certus"]["throughput_mbps"]
        assert 10.0 < certus_tp < 10_000.0, (
            f"Certus throughput {certus_tp} MB/s looks implausible"
        )

    def test_cpu_faster_than_certus(self, reference):
        # CPU (DRAM) should be faster than CXL DRAM
        assert reference["cpu"]["throughput_mbps"] > reference["certus"]["throughput_mbps"], (
            "CPU throughput should exceed Certus throughput in baselines.json"
        )
