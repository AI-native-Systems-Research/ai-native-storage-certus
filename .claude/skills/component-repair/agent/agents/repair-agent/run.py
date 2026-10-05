#!/usr/bin/env python3
"""Entry point for this agent. All runtime logic lives in agents/_kit (tested once, shared);
this agent's behavior is configured by agent.yaml, spec.yaml, gate_policy.yaml, prompts/,
evals/, and the optional checks.py, actions.py, and handlers.py modules."""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
WORKBENCH = next((p for p in ROOT.parents if (p / "agents" / "_kit").is_dir()), None)
if WORKBENCH is None:
    raise SystemExit("agents/_kit not found above " + str(ROOT))
sys.path.insert(0, str(WORKBENCH))

from agents._kit.agent import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main(ROOT))
