"""Budgets and a no-progress breaker; code, not the model, decides when to stop trying."""
from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import List, Optional


@dataclass
class Budget:
    max_attempts: Optional[int] = None
    max_model_calls: Optional[int] = None
    max_wall_seconds: Optional[float] = None
    attempts: int = 0
    model_calls: int = 0
    started: float = field(default_factory=time.monotonic)

    def charge_attempt(self) -> None:
        self.attempts += 1

    def charge_model_call(self, count: int = 1) -> None:
        self.model_calls += count

    def elapsed(self) -> float:
        return time.monotonic() - self.started

    def exhausted(self) -> Optional[str]:
        if self.max_attempts is not None and self.attempts >= self.max_attempts:
            return f"attempt budget spent ({self.attempts}/{self.max_attempts})"
        if self.max_model_calls is not None and self.model_calls >= self.max_model_calls:
            return f"model-call budget spent ({self.model_calls}/{self.max_model_calls})"
        if self.max_wall_seconds is not None and self.elapsed() >= self.max_wall_seconds:
            return f"wall-clock budget spent ({self.elapsed():.0f}s/{self.max_wall_seconds:.0f}s)"
        return None


@dataclass
class NoProgress:
    """Trips when the same failure signature repeats `limit` times in a row."""

    limit: int = 2
    history: List[str] = field(default_factory=list)

    def observe(self, signature: str) -> bool:
        self.history.append(signature)
        tail = self.history[-self.limit:]
        return len(tail) == self.limit and len(set(tail)) == 1
