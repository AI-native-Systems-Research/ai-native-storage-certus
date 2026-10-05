# Agent runtime kit

Tested, deterministic pieces that agents built by `./build_agent` import instead of
re-implementing. See `__init__.py` for the module map and `../agent_builder/PLAN.md` for why.

The rule it enforces: code owns *decisions* (did it succeed, is it allowed, when to stop, what
is canonical); the model owns its *working process*. So the kit is mostly gates around an
open-ended session, plus a small engine for work that does not fit one session.

```python
from agents._kit.lifecycle import GatedTask, run_gated
from agents._kit.policy import GatePolicy

policy = GatePolicy.load("gate_policy.yaml")        # oracle, protected paths, forbidden patterns
result = run_gated(GatedTask(name="repair", prompt=task_prompt, policy=policy, repo=repo),
                   run_dir, publisher=lambda worktree, report: worktree.commit("fix"))
```

A generated agent's `run.py` calls `agents._kit.agent.main`, which reads the agent's
configuration and runs it: `run --input name=value [--apply]`, `case <id> --case-dir DIR`, or
`--self-check`. T3 engines run through `engine.run_engine` with the agent's `handlers.py`.

Work graphs (`BUILD_GRAPH`): `build_items` returns items with `deps`. `graph.order_items`
rejects unknown ids, self-dependencies, and cycles, then orders the items (ties keep the
`build_items` order). An item starts only when all its deps are `done`. When an item fails,
its descendants are recorded as `blocked` with the reason, and an unattended run continues
with independent branches. A work queue (`BUILD_QUEUE`) rejects `deps` rather than silently
ignoring them.

Fan-out (waves): when the spec says items are parallelizable, the architecture declares an
`item_lifecycle` and a `parallelism` (`shape.max_parallel`, default 3).
- `NEXT_WAVE` takes up to that many ready items.
- `RUN_WAVE` runs each item's gated attempt in its own thread and worktree, all from the same
  base commit. Worktree bookkeeping and operator prompts are serialized.
- `MERGE` cherry-picks the verified commits onto the base in item order, then re-runs the gate
  policy on the merged tree.
  - A conflicting item is requeued once on the merged base; an optional
    `handlers.merge(ctx, item, worktree, commit) -> bool` can resolve it instead.
  - If the merged tree fails verification, the wave's merges are discarded and the run drops to
  one item per wave.

Operator choice (`shape.operator_choice`): `SELECT_ITEMS` lists the built items (for example,
problems parsed from a compiler, linter, or scanner report in `build_items`) and skips those the
operator leaves out. `PROPOSE_OPTIONS` asks the model for alternative fixes per item without
changing files. `CHOOSE` records the pick, and the session is told to implement only that
approach. Unattended runs take every item and the first option, recorded as `automatic`. A T3
agent without a work list runs its whole task as one item named `task`.

Decisions (`decisions.py`): every point where a person decides uses `ask()`.
- Covered: confirm spec and architecture, promote, select items, choose a fix, approve,
  escalate.
- The question is written to `<run_dir>/decisions/<id>.pending.json` with fixed options.
- The terminal and the Builder Studio can both answer; the first valid answer wins, and it
  must name an offered option.
- The resolved question stays on disk as `<id>.json`, with `source: terminal | studio |
  default`.
- Unattended runs (evaluation cases, no terminal and no studio) get the default at once.

Search (T4, `search.py`): the lifecycle comes from `architecture.yaml`.
- The core loop is always BASELINE → PROPOSE → MATERIALIZE → EVALUATE → SELECT → STOP_CHECK.
- PILOT (noisy metric), DIAGNOSE (a diagnostic command) and SCREEN (expensive evaluation)
  appear only when the spec's facts call for them.
- Code measures, applies `stats.accept` (improvement beyond the threshold, or beyond twice the
  pilot noise when noisy, with every SLO respected), keeps the frontier, blocks families that
  keep failing, and stops on budget, goal, or patience.
- In a `parameters` space code also proposes (local search, then seeded random restarts), and
  no model is called per candidate. Otherwise one schema-checked model call proposes, and a
  gated session implements it.
- `--apply` fast-forwards the repository to the best code candidate; parameter searches write
  `best_params.json`.

Items that all write the same output file (e.g. one `report.json`) conflict on every merge.
Write per-item files (`out/items/<id>.json`) and aggregate them in a domain handler instead.

Every run leaves a record in its run directory:

| File | Written by | Contains |
|------|-----------|----------|
| `ledger.jsonl` | lifecycle/engine (single writer) | transitions, sessions, every VERIFY result with failures and changed files, approvals, publish |
| `hook_events.jsonl` | Stop/PreToolUse hooks | every block, allow, and give-up, with the tool call or failures that caused it |
| `gate_policy.json`, `session_settings.json` | lifecycle | the exact gates the session ran under |

Design notes:

- Hooks are fast feedback; VERIFY after the session is authoritative, because a shell command
  can change files a PreToolUse hook never sees.
- The guard fails closed: a hook that cannot load its policy blocks.
- Tool output (bytecode, caches, `target/`, `node_modules/`) is excluded from the diff guard by
  `ignore_paths`, and the oracle never writes Python bytecode, so stale caches cannot fake a
  result.
- Codex sessions have no hooks; they rely on VERIFY alone.
- Sessions never run inside the workbench. Worktrees and live-evaluation fixtures live under a
  scratch root (`$AGENT_KIT_SCRATCH`, default `$TMPDIR/agent-kit`), and the guard's
  `hidden_paths` blocks reading or recursively searching the agent's `spec.yaml`, `spec.md`,
  `architecture.yaml`, `evals/`, and the builder's run records, which hold the answers to the
  agent's own evaluation cases. Best-effort, like every hook: not an OS sandbox.

Run `python3.12 -m pytest agents/_kit/tests -q` after changes.
