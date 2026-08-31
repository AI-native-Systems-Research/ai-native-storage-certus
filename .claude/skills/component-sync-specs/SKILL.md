---
name: component-sync-specs
description: Ensure a component implementation is synchronized with its specifications.
argument-hint: "[component-name, component-name, ...]"
---

For each component identified in $ARGUMENTS, run the following:
1. /speckit-sync-analyze
2. /speckit-sync-propose --interactive
3. /speckit-sync-apply
4. **Stamp the drift report for the CI Spec-Sync Gate.**

## Step 4 — Stamp the drift report

The CI Spec-Sync Gate (see `Jenkinsfile`) does not run Claude. It trusts a
freshness stamp that this step writes into each component's committed
`drift-report.md`. Perform it for every component after its sync is applied.

For a component whose directory is `<dir>` (e.g. `components/memory-tier`,
`lib/component-framework`, `tools/rdma-test`):

1. Compute the input hash over the component's `src/` + `specs/` (with
   `components/interfaces/` folded in), using the committed helper so the value
   matches what CI will recompute:

   ```bash
   scripts/spec-sync-hash.sh <dir>
   ```

2. Decide `drift_status`:
   - `clean` — no actionable spec/implementation drift remains after apply
     (doc-only or cosmetic notes in the body are fine).
   - `drift` — actionable drift is still unresolved. Never stamp `clean` to get
     a PR through the gate; fix the drift or leave it `drift` and explain.

3. Write these flat keys as YAML frontmatter at the **very top** of
   `<dir>/.specify/sync/drift-report.md`, preserving the existing
   human-readable report body below the closing `---`. Flat, prefixed keys are
   deliberate so CI can parse them with `grep` and no YAML parser. Replace an
   existing `spec_sync_*` frontmatter block in place rather than adding a second
   one:

   ```
   ---
   spec_sync_component: <component-name>
   spec_sync_drift_status: clean
   spec_sync_synced_at: <UTC ISO-8601, e.g. 2026-08-31T14:03:00Z>
   spec_sync_git_commit: <output of `git rev-parse --short HEAD`>
   spec_sync_inputs_sha256: <digest from step 1>
   spec_sync_hash_tool: scripts/spec-sync-hash.sh
   ---
   # Drift Report: <component-name>
   ... existing body ...
   ```

Commit the updated `drift-report.md` together with the code and spec changes so
the stamp travels in the same commit as the inputs it certifies. If you edit any
`src/` or `specs/` file after stamping, re-run step 1 and update the digest —
otherwise CI will reject the report as stale.

