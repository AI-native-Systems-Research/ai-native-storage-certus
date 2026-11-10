"""Measurement statistics for search agents: code, never the model, decides what improved.

Standard library only. Values are relative: a spread of 0.03 means samples typically sit about
3% from their median, and an improvement of 0.05 means 5% better in the metric's direction.
"""
from __future__ import annotations

import math
import statistics
from dataclasses import dataclass
from typing import List, Optional, Sequence

# Noise multiplier: a noisy "improvement" must exceed this many robust spreads of the pilot.
NOISE_K = 2.0
# 1.4826 * MAD estimates the standard deviation for normally distributed samples.
MAD_TO_SIGMA = 1.4826


def median(values: Sequence[float]) -> float:
    if not values:
        raise ValueError("no samples")
    return float(statistics.median(values))


def relative_spread(values: Sequence[float]) -> float:
    """Robust relative spread: 1.4826 * MAD / |median|. 0 for fewer than two samples."""
    if len(values) < 2:
        return 0.0
    center = median(values)
    mad = median([abs(v - center) for v in values])
    return MAD_TO_SIGMA * mad / abs(center) if center else (0.0 if mad == 0 else math.inf)


def improvement(parent: float, candidate: float, direction: str) -> float:
    """Relative improvement of candidate over parent; positive means better."""
    if direction not in ("higher", "lower"):
        raise ValueError(f"direction must be 'higher' or 'lower', got {direction!r}")
    if parent == 0:
        delta = candidate - parent
        return delta if direction == "higher" else -delta
    change = (candidate - parent) / abs(parent)
    return change if direction == "higher" else -change


def repeats_for(spread: float, threshold: float, *, minimum: int = 3, maximum: int = 15) -> int:
    """Samples per measurement so the median is stable relative to the acceptance threshold.

    The standard error of a median is about 1.25 * sigma / sqrt(n); ask for enough samples that
    it is at most half the threshold, clamped to [minimum, maximum].
    """
    if spread <= 0:
        return minimum
    if threshold <= 0:
        return maximum
    needed = math.ceil((1.25 * spread / (threshold / 2)) ** 2)
    return max(minimum, min(maximum, needed))


@dataclass
class Decision:
    accepted: bool
    improvement: float
    required: float
    reason: str


def accept(parent: Sequence[float], candidate: Sequence[float], *, direction: str,
           threshold: float = 0.0, noise: Optional[float] = None) -> Decision:
    """Accept a candidate only if its median beats the parent's by the required margin.

    Deterministic metrics (`noise` is None) need a strict improvement of at least `threshold`.
    Noisy metrics need at least max(threshold, NOISE_K * noise), where `noise` is the pilot's
    relative spread, so an improvement inside the measurement noise is never accepted.
    """
    gain = improvement(median(parent), median(candidate), direction)
    required = threshold if noise is None else max(threshold, NOISE_K * noise)
    if gain <= 0:
        return Decision(False, gain, required, f"not better ({gain:+.2%})")
    if gain < required:
        why = "within measurement noise" if noise is not None and required > threshold else "below the threshold"
        return Decision(False, gain, required, f"{gain:+.2%} is {why} (needs {required:+.2%})")
    return Decision(True, gain, required, f"{gain:+.2%} (needs {required:+.2%})")


def within(value: float, *, maximum: Optional[float] = None, minimum: Optional[float] = None) -> bool:
    return (maximum is None or value <= maximum) and (minimum is None or value >= minimum)


def summary(values: List[float]) -> dict:
    return {"median": median(values), "spread": relative_spread(values), "samples": list(values)}
