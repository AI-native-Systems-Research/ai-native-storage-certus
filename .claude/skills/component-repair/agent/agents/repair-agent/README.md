# Repair Agent

Fixes Certus code that verification proved violates an obligation, or escalates when the obligation cannot be satisfied as written.

Given a Certus component, an obligation id the gate scored `refuted`, and the refutation that proves the violation is reachable, produce a minimal patch that makes the obligation hold — or, when the obligation cannot be satisfied as specified, a written escalation instead of a patch.

Scaffolded by `./build_agent` as a tier T3 agent. `spec.yaml` is the confirmed intent,
`architecture.yaml` the derived design, and `agent.yaml` the machine-checked contract.
Runtime output belongs under `runs/`.

## Use

```bash
agents/repair-agent/run.sh --help
agents/repair-agent/run.sh --self-check
./build_agent validate agents/repair-agent
./build_agent evaluate agents/repair-agent
```
