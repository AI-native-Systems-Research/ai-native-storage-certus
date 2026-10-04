---
name: component-repair
description: Repair a Certus code defect that formal verification found, with a human choosing the fix and approving the PR. Runs the vendored repair-agent; acceptance is decided by repair_accept.sh, never by the agent.
argument-hint: "<component> <OBLIGATION-ID> --callees <mods> --base <rev>"
---

# component-repair

Fixes the CODE behind an in-scope obligation (spec+code or code-only) that verification refuted, or
writes an escalation when it cannot be fixed locally (e.g. a shared interface must change). It never
edits the obligation, the spec, or a proof that already exists.

## What is here
- `spec.yaml` — the agent spec (agent_builder schema). Validates with 0 errors / 0 warnings.
- `repair_accept.sh` — the acceptance check, run by code. A repair passes only if it is
  PROVED (callee modules + a new verify_<id> prove; its mutant fails with the prover having run),
  REAL (src/ changed), RED-FIRST (its own repair_test_*.rs fails on the unfixed code), TESTED
  (cargo test passes) and HARMLESS (no module that proved before stops proving).
- `repair_oracle.sh` — the proof part of that check.
- `evals/` — held-out tests the agent never sees, run against the real code.
- `agent/` — the generated agent and the workbench runtime kit, VENDORED (see `agent/VENDORED.yaml`
  for the exact workbench commit). We own this copy; workbench changes do not reach it.

## Run it (attended)
```
export AGENT_RESOURCE_AI_NATIVE_STORAGE_CERTUS=<your certus checkout>
export AGENT_RESOURCE_CREUSOT=<creusot tree>      # for creusot-std
.claude/skills/component-repair/agent/agents/repair-agent/run.sh run \
  --input repository=<checkout> --input component=<c> --input obligation_id=<ID> \
  --input bundle=components/<c>/verif/unified_properties.yaml \
  --input callee_modules=<mods> --input base_rev=<rev> \
  --input oracle_dir=.claude/skills/component-repair
```
The agent proposes up to 3 fixes and **you choose one**. After the acceptance check passes it asks
for **approval; approving pushes a branch and opens a PR**. Set `REPAIR_AGENT_NO_PR=1` to keep the
verified commit local instead. Evaluation runs never push or open a PR.

## Rules
- Only in-scope obligations (spec+code, code-only). A divergent or spec-only record is spec<->code
  synchronisation, not a defect; do not hand it to this agent.
- The operator names `callee_modules`; the agent must not choose its own evidence.
- A bug appears on a component's scoring page only once its fix is merged.
