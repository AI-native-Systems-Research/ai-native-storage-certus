"""Authoritative verification, run by code after (and independent of) any model session."""
from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, List, Optional

from .guard import check_changes
from .oracle import OracleResult, run_oracle
from .policy import GatePolicy
from .workspace import Worktree


@dataclass
class VerifyReport:
    passed: bool
    failures: List[str] = field(default_factory=list)
    oracle: Optional[OracleResult] = None
    changed_files: List[str] = field(default_factory=list)

    def signature(self) -> str:
        """Stable fingerprint of what failed, for no-progress detection (numbers normalized)."""
        parts = list(self.failures)
        if self.oracle and not self.oracle.passed:
            parts.append(self.oracle.tail(5))
        return re.sub(r"\d+(\.\d+)?", "#", "\n".join(parts))

    def feedback(self) -> str:
        lines = ["Verification failed:"] + [f"- {failure}" for failure in self.failures]
        if self.oracle and not self.oracle.passed:
            lines += ["", "Oracle output (tail):", self.oracle.tail()]
        return "\n".join(lines)


def check_outputs(policy: GatePolicy, workdir: Path) -> List[str]:
    failures = []
    for output, schema_path in policy.output_schemas.items():
        target = Path(workdir) / output
        if not target.is_file():
            failures.append(f"required output missing: {output}")
            continue
        try:
            import jsonschema

            schema = json.loads(Path(schema_path).read_text(encoding="utf-8"))
            instance = json.loads(target.read_text(encoding="utf-8"))
            errors = sorted(jsonschema.Draft202012Validator(schema).iter_errors(instance), key=str)
        except (OSError, ValueError) as exc:
            failures.append(f"{output}: cannot validate: {exc}")
            continue
        failures += [f"{output}: {error.message}" for error in errors[:5]]
    return failures


ExtraCheck = Callable[[Path, Optional[Worktree]], List[str]]


def verify(
    policy: GatePolicy,
    workdir: Path,
    worktree: Optional[Worktree] = None,
    extra: Optional[ExtraCheck] = None,
) -> VerifyReport:
    """Diff guard, output schemas, extra checks, then the oracle. Reports every failure.

    `extra` is an agent's additional checks (rules the policy cannot express). It can only add
    failures; a crash in it counts as a failure rather than a pass.
    """
    failures: List[str] = []
    changed: List[str] = []
    if worktree is not None:
        changes = worktree.changes()
        changed = [path for path, _ in changes]
        failures += check_changes(changes, policy)
    failures += check_outputs(policy, workdir)
    if extra is not None:
        try:
            failures += list(extra(Path(workdir), worktree))
        except Exception as exc:  # an extra check that crashes must not count as passing
            failures.append(f"extra check crashed: {exc}")
    result = None
    if policy.oracle is not None:
        result = run_oracle(policy.oracle, workdir)
        failures += result.reasons
    return VerifyReport(not failures, failures, result, changed)
