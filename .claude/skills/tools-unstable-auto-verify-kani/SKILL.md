---
name: tools-unstable-auto-verify-kani
description: Documents the automated Kani verification workflow that keeps unstable-kani in sync with unstable. Covers the normal flow, how to interpret CI results, and the exact steps to resolve merge conflicts and Kani failures.
---

## Overview

`unstable-kani` is the Kani verification overlay of the team's `unstable`
branch. It contains everything in `unstable` plus:

- `#[cfg(kani)]` DmaBuffer stub in `components/interfaces/src/spdk_types.rs`
- `RefCountOverflow` error variant in `DispatchMapError`
- `checked_add` overflow fixes in `dispatch-map/v0/src/lib.rs`
- Six Kani harnesses in `#[cfg(kani)] mod verification`
- `components/dispatch-map/v0/VERIFICATION.md`

The branch is kept in sync automatically by GitHub Actions. **Colleagues
push to `unstable` as normal — no extra steps are required from them.**

---

## Normal Flow (Fully Automated)

```
Colleague pushes commit to unstable
              │
              ▼
  kani-sync-verify.yml triggers
              │
    ┌─────────┴──────────┐
    │  merge origin/unstable  │
    │  into unstable-kani     │
    └─────────┬──────────┘
              │
       merge clean?
       /           \
     YES             NO
      │               │
  cargo kani      open GitHub issue
  (dispatch-map)  "Merge conflict"
      │
  all pass?
  /        \
YES          NO
 │            │
push        open GitHub issue
unstable-   "Kani verification
kani        failure"
```

**Workflow file:** `.github/workflows/kani-sync-verify.yml`
**Kani version:** 0.67.0 — nightly-2025-11-21
**Verification target:** `components/dispatch-map/v0/Cargo.toml`

---

## Your Daily Workflow

You work directly on `unstable-kani` on the upstream team repo.
No fork needed.

```bash
# Pull the latest (after CI has auto-synced)
git fetch origin
git pull origin unstable-kani
```

You only need to act when a **GitHub issue appears** in the repo.
Issues are labelled either `kani` + `verification-failure` or indicate
a merge conflict. Check open issues at:

```
https://github.com/AI-native-Systems-Research/ai-native-storage-certus/issues
```

---

## Case 1 — Merge Conflict

**Symptom:** GitHub issue titled  
`Merge conflict: unstable → unstable-kani (xxxxxxx)`

**Cause:** A colleague changed a file in `unstable` that also contains
Kani-specific additions in `unstable-kani`. The bot cannot auto-resolve.

**Files most likely to conflict:**

| File | Why |
|---|---|
| `components/dispatch-map/v0/src/lib.rs` | ref-count logic sits next to Kani harnesses |
| `components/interfaces/src/idispatch_map.rs` | error enum has our `RefCountOverflow` variant |
| `components/interfaces/src/spdk_types.rs` | DmaBuffer has our `#[cfg(kani)]` stub |
| `components/dispatch-map/v0/Cargo.toml` | our `[lints.rust]` section |
| `components/interfaces/Cargo.toml` | our `[lints.rust]` section |

**Resolution steps:**

```bash
git fetch origin
git checkout unstable-kani
git merge origin/unstable
# Git will report the conflicting files
```

For each conflict, the rule is:
**keep upstream logic, preserve Kani-specific code.**

Kani markers to always preserve in conflicts:
- `#[cfg(kani)]` and `#[cfg(not(kani))]` guards on structs and impl blocks
- `#[cfg(kani)] mod verification { ... }` harness modules
- `checked_add / checked_mul` at arithmetic sites
- `RefCountOverflow` variant and its `Display` arm
- `[lints.rust] unexpected_cfgs = ['cfg(kani)']` in Cargo.toml files

After resolving:

```bash
git add <resolved-files>
git commit  # merge commit message is pre-filled by git
git push origin unstable-kani
```

CI will re-trigger on the push and run Kani to confirm the resolution
is correct.

---

## Case 2 — Kani Verification Failure

**Symptom:** GitHub issue titled  
`Kani verification failure after unstable commit xxxxxxx`

**Cause:** A colleague's new code in `unstable` introduced a change that
breaks one or more Kani harnesses — typically a new arithmetic operation
without overflow protection, or a state transition that violates a proved
invariant.

**Step 1 — Identify the failing harness**

Click the workflow run link in the issue. In the "Run Kani harnesses"
step, look for lines like:

```
Failed Checks: attempt to multiply with overflow
 File: "src/lib.rs", line N, in SomeFunction
```

**Step 2 — Decide: bug or intentional change?**

| The new code... | Action |
|---|---|
| Introduced a real arithmetic bug (missing guard) | Fix the bug in `unstable`, then let CI re-sync |
| Changed existing logic intentionally (e.g. wider type, new field) | Update the Kani harnesses in `unstable-kani` to match |

**Step 3a — Fix the bug in unstable**

```bash
git checkout unstable
# fix the arithmetic — use checked_add / checked_mul / checked_sub
git commit -m "fix: guard arithmetic overflow in <function>"
git push origin unstable
# CI will re-sync unstable-kani and re-run Kani automatically
```

**Step 3b — Update harnesses for intentional change**

```bash
git checkout unstable-kani
git fetch origin
git merge origin/unstable   # bring in the intentional change
# update the affected harness in #[cfg(kani)] mod verification
cargo kani --manifest-path components/dispatch-map/v0/Cargo.toml
# confirm all harnesses pass locally before pushing
git add components/dispatch-map/v0/src/lib.rs
git commit -m "kani: update harnesses for <description of change>"
git push origin unstable-kani
```

---

## Case 3 — CI Fails to Push Back (Permission Error)

**Symptom:** Workflow run shows push step failed with a permission error.

**Cause:** Branch protection rules on `unstable-kani` require status
checks or PR review before pushes, blocking the bot.

**Resolution:** A repo admin must either:
- Add `github-actions[bot]` as a bypass actor for `unstable-kani`
  branch protection rules, **or**
- Reduce protection on `unstable-kani` (it is a verification branch,
  not a release branch — strict protection is not required)

---

## Case 4 — Kani Version or Toolchain Mismatch

**Symptom:** CI fails at the "Install Kani verifier" or "Run Kani
harnesses" step with a toolchain or compatibility error.

**Resolution:** Update the pinned versions in the workflow file:

```yaml
# .github/workflows/kani-sync-verify.yml

- name: Install Rust nightly
  with:
    toolchain: nightly-YYYY-MM-DD   # ← update here

- name: Cache Kani toolchain
  with:
    key: kani-X.Y.Z-${{ runner.os }} # ← update here

- name: Install Kani verifier
  run: cargo install kani-verifier --version X.Y.Z --locked
```

Also update the unwind values in the harnesses if a new Kani version
changes how loops are bounded.

---

## Adding Harnesses for New Components

When a new component is added to `unstable`, add its Kani harnesses to
`unstable-kani` following the `tools-verify-kani` skill. Then update the
workflow to include the new verification target:

```yaml
# .github/workflows/kani-sync-verify.yml
- name: Run Kani harnesses — dispatch-map
  run: |
    cargo kani --manifest-path components/dispatch-map/v0/Cargo.toml

# Add a new step for each additional component:
- name: Run Kani harnesses — <new-component>
  run: |
    cargo kani --manifest-path components/<new-component>/vN/Cargo.toml
```

---

## Quick Reference

```bash
# Check CI status of latest unstable push
# → go to GitHub Actions tab on the repo

# Pull latest auto-synced unstable-kani
git fetch origin && git pull origin unstable-kani

# Run Kani locally before pushing a harness update
cargo kani --manifest-path components/dispatch-map/v0/Cargo.toml

# Manually trigger a sync (force-merge unstable into unstable-kani)
git fetch origin
git checkout unstable-kani
git merge origin/unstable
git push origin unstable-kani

# Check open verification-failure issues
gh issue list --label kani --repo AI-native-Systems-Research/ai-native-storage-certus
```
