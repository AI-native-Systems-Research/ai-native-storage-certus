# Align Tasks — dispatcher

Generated: 2026-08-31

No ALIGN tasks this run.

All drift resolved this cycle was BACKFILL (spec → matches the working, intentional code):
the code/architecture is authoritative in every case (gRPC→shmq transport change in commit
`97e26738`; the shipped tier-event counter subsystem). No code change is required.

## Out-of-scope follow-ups (informational, not align tasks)

- `src/lib.rs:2983`, `:3016` — reword the two residual "gRPC handler" code comments to
  "shmq serve layer / null-stream caller". Source comments are outside this sync's editable
  scope; handle in a normal code-comment pass.
