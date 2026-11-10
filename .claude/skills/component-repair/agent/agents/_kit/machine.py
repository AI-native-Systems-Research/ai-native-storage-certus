"""Transitions-table state machine. Handlers return a condition; the table picks the next state.

Undefined transitions raise instead of guessing, every transition is written to the ledger,
and a run can resume from the ledger's last recorded state.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Dict, Iterable, Mapping, Optional, Tuple

from .ledger import Ledger

DEFAULT = "_default"
Handler = Callable[[Any], str]


class InvalidTransition(Exception):
    def __init__(self, state: str, condition: str):
        super().__init__(f"no transition from {state} on {condition!r}")
        self.state = state
        self.condition = condition


@dataclass
class StateMachine:
    transitions: Mapping[Tuple[str, str], str]
    initial: str
    terminal: Tuple[str, ...]
    owners: Optional[Mapping[str, str]] = None

    def __post_init__(self) -> None:
        states = {self.initial, *self.terminal}
        for (source, _), target in self.transitions.items():
            states.update((source, target))
        self.states = frozenset(states)
        for terminal in self.terminal:
            if any(source == terminal for source, _ in self.transitions):
                raise ValueError(f"terminal state {terminal} has outgoing transitions")

    @classmethod
    def from_edges(cls, edges: Iterable[Tuple[str, str, str]], initial: str, terminal: Iterable[str],
                   owners: Optional[Mapping[str, str]] = None) -> "StateMachine":
        table: Dict[Tuple[str, str], str] = {}
        for source, condition, target in edges:
            if (source, condition) in table:
                raise ValueError(f"duplicate transition {source} --{condition}-->")
            table[(source, condition)] = target
        return cls(table, initial, tuple(terminal), owners)

    @classmethod
    def from_architecture(cls, architecture: Mapping[str, Any]) -> "StateMachine":
        """Load the lifecycle an agent's architecture.yaml declares."""
        lifecycle = architecture.get("lifecycle") or architecture.get("state_machine")
        edges = [(t["from"], t["on"], t["to"]) for t in lifecycle["transitions"]]
        owners = {s["name"]: s["owner"] for s in lifecycle.get("states", [])}
        return cls.from_edges(edges, lifecycle["initial"], lifecycle["terminal"], owners)

    def next(self, state: str, condition: str) -> str:
        if (state, condition) in self.transitions:
            return self.transitions[(state, condition)]
        raise InvalidTransition(state, condition)

    def conditions(self, state: str) -> Tuple[str, ...]:
        return tuple(cond for source, cond in self.transitions if source == state)

    def run(
        self,
        handlers: Mapping[str, Handler],
        context: Any,
        *,
        ledger: Optional[Ledger] = None,
        start: Optional[str] = None,
        max_steps: int = 10_000,
    ) -> str:
        state = start or self.initial
        for _ in range(max_steps):
            if state in self.terminal:
                if state in handlers:
                    handlers[state](context)
                if ledger is not None:
                    ledger.append("terminal", state=state)
                return state
            handler = handlers.get(state)
            if handler is None:
                if self.conditions(state) == (DEFAULT,):
                    condition = DEFAULT
                else:
                    raise KeyError(f"no handler for state {state}")
            else:
                condition = handler(context) or DEFAULT
            target = self.next(state, condition)
            if ledger is not None:
                ledger.append("transition", source=state, on=condition, target=target)
            state = target
        raise RuntimeError(f"state machine exceeded {max_steps} steps")

    @staticmethod
    def resume_state(ledger: Ledger) -> Optional[str]:
        """The state to resume from, or None if the run finished or never started."""
        last = ledger.last()
        if last is None or last["event"] == "terminal":
            return None
        transition = ledger.last("transition")
        return transition["target"] if transition else None
