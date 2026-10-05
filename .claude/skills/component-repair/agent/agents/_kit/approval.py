"""Operator approval for actions code performs on the model's behalf.

Irreversible tools are never handed to a model session. The model asks for the outcome; code
asks the operator, records the decision, and only then performs the action.
"""
from __future__ import annotations

from typing import Callable, Optional

from .ledger import Ledger

Approver = Callable[[str], bool]


def console_approver(prompt: str) -> bool:
    import sys

    if not sys.stdin.isatty():
        return False
    return input(f"{prompt} [y/N]: ").strip().lower() in ("y", "yes")


def deny_all(prompt: str) -> bool:
    return False


def request_approval(what: str, approver: Approver, ledger: Optional[Ledger] = None, **context) -> bool:
    approved = bool(approver(what))
    if ledger is not None:
        ledger.append("approval", what=what, approved=approved, **context)
    return approved


def perform_after_approval(
    what: str, action: Callable[[], object], approver: Approver, ledger: Optional[Ledger] = None,
) -> bool:
    if not request_approval(what, approver, ledger):
        return False
    action()
    if ledger is not None:
        ledger.append("action_performed", what=what)
    return True
