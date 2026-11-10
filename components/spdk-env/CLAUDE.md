# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is the `spdk-env` component of the **Certus** project — a generative domain-specific filesystem for inferencing workloads. This component provides the SPDK (Storage Performance Development Kit) environment and integration for the Certus system.

Certus uses a component-based architecture inspired by COM, where components are developed independently and later integrated. The parent repo is a Rust workspace rooted at `../../Cargo.toml`.

## Repository Context

- This component is not yet a workspace member — it has no `Cargo.toml` or source code yet. It currently contains only speckit tooling configuration (`.specify/`, `.claude/skills/`).
- SPDK build/install scripts live in `../../deps/`:
  - `../../deps/build_spdk.sh` — clones, configures (`--without-crypto`), builds, and installs SPDK to `../../deps/spdk-build/`
  - `../../deps/install_deps.sh` — installs system dependencies via `dnf` (fuse3, numactl, ninja-build, CUnit, etc.)
  - `../../deps/requirements.txt` — Python deps: `meson`, `pyelftools`

## Build Commands

```bash
# Install system dependencies (requires sudo, RHEL/Fedora)
../../deps/install_deps.sh

# Install Python dependencies
pip install -r ../../deps/requirements.txt

# Build and install SPDK (clones to deps/spdk, installs to deps/spdk-build)
../../deps/build_spdk.sh

# Build the full Certus workspace (from repo root)
cargo build
cargo test --all
```

## Architecture Notes

- The parent Certus project is a Rust workspace (edition 2021, MSRV 1.75) using a component framework with interfaces, receptacles, actors, lock-free channels, and NUMA support.
- Components should have low coupling. When developing this component, limit dependencies to the few other components it must bind to.
- The `.specify/` directory contains speckit integration for AI-assisted specification and planning workflows. The `.claude/skills/` directory has speckit-related Claude Code skills.
- SPDK source is cloned into `../../deps/spdk/` and build artifacts go to `../../deps/spdk-build/`. Both are gitignored.

## Recent Changes
- 002-spdk-env-vfio-init: Added [if applicable, e.g., PostgreSQL, CoreData, files or N/A]
