"""Shared runtime kit for workbench agents built by agent_builder.

Deterministic, tested pieces that generated agents import instead of re-implementing:

Gates (every tier that has them):
    policy     GatePolicy: oracle, protected paths, forbidden patterns/commands, output schemas
    oracle     run the success check
    guard      rule checks for a single tool call and for a whole diff
    workspace  git worktree isolation, changed files, added lines, commit
    verify     authoritative post-session verification (oracle + diff guard + schemas)
    hooks      Claude Code Stop / PreToolUse hook entry points (`python -m agents._kit.hooks`)
    session    hook settings and Claude/Codex session commands
    approval   operator approval for actions code performs on the model's behalf
    decisions  one way to ask a person: fixed options, answered in the terminal or the Builder Studio

Agent runtime (what a generated agent's run.py calls):
    agent      AgentHome plus `run` / `case` / `--self-check` for T0, T1, T2
    engine     T3 runtime: the architecture's state machine with default handlers
    graph      work graph: validation, dependency order, readiness, and blocking by code
               (engine also runs parallel waves: NEXT_WAVE -> RUN_WAVE -> MERGE)
    search     T4 runtime: measured search with code-owned acceptance, frontier, and stopping
    stats      median, robust spread, repeat counts, and the acceptance rule

Loop and engine (T2 lifecycle, T3 engines):
    budget     attempt/model-call/wall-clock budgets and a no-progress breaker
    ledger     append-only JSONL ledger with a single locked writer and resume
    machine    transitions-table state machine, loadable from architecture.yaml
    lifecycle  the T2 gated-session lifecycle: PREPARE -> SESSION -> VERIFY -> PUBLISH
    llm        schema-validated headless model call with a bounded repair retry
"""
