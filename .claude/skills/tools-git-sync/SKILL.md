---
name: tools-git-sync
description: Stay in sync with origin/unstable (team repo) using the SSH remote setup in this repository. Covers fetching, rebasing or merging local branches, resolving conflicts with upstream-first strategy, and pushing personal work to my-fork.
argument-hint: "[branch-to-sync]"
---

## Remote Layout

This repo uses a two-remote model established in May 2026:

| Remote | URL | Role |
|---|---|---|
| `origin` | `git@github.com:AI-native-Systems-Research/ai-native-storage-certus.git` | Team repo — source of truth |
| `my-fork` | `git@github.com:cornelconst/ai-native-storage-certus.git` | Personal fork — where you push your work |

SSH is required. HTTPS pushes will fail with "Password authentication not
supported". Verify SSH works before any push:

```bash
ssh -T git@github.com
# Expected: Hi cornelconst! You've successfully authenticated...
```

---

## Branch Conventions

| Branch | Tracks | Purpose |
|---|---|---|
| `main` | `my-fork/main` | Personal main — mirrors origin/main when ready |
| `unstable-kani` | `origin/unstable` | Active integration — Kani work on top of team unstable |
| `kani_harnesses` | `my-fork/kani_harnesses` | Isolated Kani work before merging into unstable-kani |

For new feature work, always branch from `origin/unstable` (not `main`)
since `unstable` is where the team integrates ongoing development.

---

## Daily Sync: Pulling Team Changes into unstable-kani

Run this whenever teammates push to `origin/unstable`:

```bash
git fetch origin
git checkout unstable-kani
git merge origin/unstable
```

Prefer `merge` over `rebase` for `unstable-kani` because it has a
published merge commit history (`my-fork/unstable-kani`). Rebasing
would rewrite that history and force-push.

If the branch is clean (no local commits ahead of `origin/unstable`),
a fast-forward suffices:

```bash
git merge --ff-only origin/unstable
```

---

## Conflict Resolution Strategy

When merging `origin/unstable` into `unstable-kani`, conflicts arise
when the team changes files that also contain Kani additions.

**Rule: keep upstream logic, preserve Kani-specific code.**

In practice this means:
- Take the upstream version as the base for any logic conflict
- Layer our Kani additions back on top manually if they were lost

Example from the initial merge (May 2026):
- `lookup()` in `dispatch-map/v0/src/lib.rs`: upstream added
  `entry.tsc = rdtsc()`, Kani branch added `checked_add`. Resolution:
  apply `checked_add` first, then add `entry.tsc = rdtsc()` after.

Files most likely to conflict:
- `components/dispatch-map/v0/src/lib.rs` — ref-count logic + Kani harnesses
- `components/interfaces/src/idispatch_map.rs` — error enum + new interface methods
- `components/interfaces/src/spdk_types.rs` — DmaBuffer Kani stub

Kani-specific markers to watch for and preserve in conflicts:
- `#[cfg(kani)]` and `#[cfg(not(kani))]` guards on structs and impl blocks
- `#[cfg(kani)] mod verification { ... }` harness modules
- `checked_add / checked_mul / checked_sub` at arithmetic sites
- `RefCountOverflow` error variant and its `Display` arm
- `[lints.rust] unexpected_cfgs = ['cfg(kani)']` in Cargo.toml files

---

## Starting New Work on a Feature Branch

Always branch from the latest `origin/unstable`:

```bash
git fetch origin
git checkout -b my-feature --track origin/unstable
```

When ready to integrate back:

```bash
git checkout unstable-kani
git merge my-feature
# resolve conflicts with upstream-first rule above
git push my-fork unstable-kani
```

---

## Pushing Your Work

You never push directly to `origin` (team repo). All pushes go to `my-fork`:

```bash
# Push unstable-kani to your fork
git push my-fork unstable-kani

# Push a new branch for the first time
git push -u my-fork <branch-name>
```

To propose changes to the team repo, open a PR from
`cornelconst/<branch>` → `AI-native-Systems-Research/unstable` on GitHub.

---

## Syncing main with the Team

`main` in this repo is currently ahead of `origin/main` by ~110 commits
(personal work not yet merged upstream). To update `main` when
`origin/main` advances:

```bash
git fetch origin
git checkout main
git merge origin/main   # or rebase if main is clean
git push my-fork main
```

---

## Re-establishing the Remote Setup from Scratch

If you clone fresh and need to restore this two-remote layout:

```bash
# After cloning from my-fork:
git clone git@github.com:cornelconst/ai-native-storage-certus.git
cd ai-native-storage-certus

# Rename the default origin to my-fork
git remote rename origin my-fork

# Add the team repo as origin
git remote add origin git@github.com:AI-native-Systems-Research/ai-native-storage-certus.git

# Fetch team branches
git fetch origin

# Recreate unstable-kani tracking origin/unstable
git checkout -b unstable-kani --track origin/unstable
```

---

## Automated Sync via GitHub Actions

Two workflows in `.github/workflows/` handle the sync automatically:

| Workflow | Trigger | Action on pass | Action on fail |
|---|---|---|---|
| `kani-sync-verify.yml` | push to `unstable` | merge + `cargo kani` + push `unstable-kani` | open GitHub issue |
| `creusot-sync-verify.yml` | push to `unstable` | merge + Creusot verify + push `unstable-creusot` | open GitHub issue |

**What this means in practice:**
- Team members push to `unstable` as normal — no extra steps needed
- The CI automatically keeps `unstable-kani` and `unstable-creusot` in sync
- If a new `unstable` commit breaks a harness, a GitHub issue is opened
  with a link to the failing run and instructions for resolution
- If there is a merge conflict, the issue describes the manual resolution steps

**The Creusot workflow currently exits 0** (placeholder) until the Creusot
verification command is established in `tools/creusot/`. Update the
`Run Creusot verification` step in `creusot-sync-verify.yml` when ready.

**Kani version pinned to 0.67.0** with nightly-2025-11-21. Update both the
`kani-cache` key and the `install` step when upgrading Kani.

## Quick Reference

```bash
# Check remote layout
git remote -v

# Check all branch tracking
git branch -vv

# Fetch all remotes at once
git fetch --all

# See what's new on origin/unstable since last fetch
git log unstable-kani..origin/unstable --oneline

# See your commits not yet on origin/unstable
git log origin/unstable..unstable-kani --oneline

# Push unstable-kani to your fork
git push my-fork unstable-kani
```
