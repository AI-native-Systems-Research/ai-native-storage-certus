# Certus Knowledge Graph

## Purpose

This graph answers four questions at a glance:

- **Where am I?** — Level N in the dependency DAG
- **What can I touch?** — Levels below are your deps; levels above depend on you
- **What's broken?** — Bugs, spec drift, invariant violations
- **What's missing?** — Functions needed for the vLLM contract

## Why Dependency Depth (not Domain Categories)

Components are ordered by topological depth in the dependency DAG.
Level 0 has no dependencies. Level N depends only on levels < N.

This gives:

- **Guard rails**: don't ship level N until levels 0..(N-1) are green
- **Build order**: implement bottom-up
- **Impact analysis**: breaking level N affects all levels above it

Domain (SPDK, Cache, GPU, Storage) is a color tag for human orientation,
not the organizing principle. Dispatcher (Cache domain) depends on
gpu-services (GPU domain) and spdk-env (SPDK domain) — domain categories
don't capture this; dependency depth does.

## Structure

```
knowledge/
├── README.md            ← this file (purpose, structure, navigation)
├── _index.md            ← component table by depth level
├── _status.md           ← bugs, drift, missing functions, safe zones
├── build_graph.py       ← generates graph.html from component tree
├── graph.html           ← visual DAG (open in browser)
├── WIKI.md              ← legacy entry point
├── component-architecture/  ← reference: Szyperski, COM model
└── spdk/                    ← reference: SPDK internals
```

## Regeneration

```bash
python3 knowledge/build_graph.py
```

Reads directly from:
- `components/*/README.md` — descriptions
- `components/interfaces/src/*.rs` — actual trait method signatures
- `components/*/specs/*/contracts/*.md` — spec'd method signatures
- `knowledge/_status.md` — known issues and missing functions
- `certus-connector/README.md` — connector status

No manual upkeep of the graph — if code changes, regenerate.

## How to Read the Graph

```
Level 6: certus-connector    ← application (assembler)
Level 5: dispatcher          ← orchestration
Level 4: dispatch-map        ← index/cache logic
Level 3: extent-manager      ← storage allocation
Level 2: block-device, gpu   ← hardware abstraction
Level 1: spdk-env, interfaces← environment + contracts
Level 0: spdk-sys, framework ← foundation (FFI, macros)
```

- Top = application. Bottom = foundation.
- Each component only depends downward.
- A red "needs-work" badge means: do not depend on this component's
  broken paths until fixed.

## Machine Navigation

Each component is a node with typed attributes:

| Attribute | Type | Meaning |
|-----------|------|---------|
| `level` | int | Topological depth (0 = no deps) |
| `status` | enum | done, in-progress, needs-work |
| `domain` | string | Color category (SPDK, Cache, GPU, etc.) |
| `deps` | [string] | Components at lower levels this depends on |
| `rdeps` | [string] | Components at higher levels that depend on this |
| `drift` | [string] | Methods in code but not spec (or vice versa) |
| `missing` | [string] | Functions needed but not yet implemented |

### Agent Workflow

1. Read `_status.md` for current blockers and safe zones
2. Find the lowest-level component with `status != done`
3. Verify all its `deps` are `done`
4. Implement the `missing` functions for that component
5. Run `python3 knowledge/build_graph.py` — verify drift cleared
6. Move up to the next level

### Guard Rail Rules

- Never ship a component at level N if any level < N dep has `status = needs-work`
- Never call a function listed in `drift` from new code without checking the actual signature
- Functions in `missing` are blocking the vLLM contract — prioritize them
- `_status.md` "Safe to Commit" section lists what can ship independently

## Commands

| Command | When to use |
|---|---|
| `python3 knowledge/build_graph.py` | After any code change — regenerates graph.html, detects drift |
| `/graphify .` | First run on this repo, or after major structural changes (builds full knowledge graph) |
| `/graph-review .` | After implementing a component, before integration testing, when something feels off |

### `build_graph.py` — Certus-specific graph

This is the repo's own tool. It reads component READMEs, interface traits, and spec contracts
to generate `graph.html` with dependency levels, status, drift warnings, and missing functions.

Run after: modifying any component, adding methods to interfaces, updating specs.

### `/graphify` — Full knowledge graph (external tool)

Builds a graphify knowledge graph from the entire repo: AST extraction for Rust code,
semantic extraction for docs. Outputs to `graphify-out/` with interactive HTML, JSON,
and a GRAPH_REPORT.md showing god nodes and community structure.

```bash
/graphify .              # full pipeline
/graphify . --directed   # preserve edge direction
/graphify . --update     # incremental after code changes
```

### `/graph-review` — Graph-driven code review (external tool)

Uses a graphify graph to find hidden coupling and dead signal. Reads god nodes and
INFERRED edges, verifies each against source code, reports CONFIRMED+GAP findings.

```bash
/graph-review .            # build graph if missing, then review
/graph-review . --rebuild  # force-rebuild graph
```

Run after: implementing a new component, before integration testing, after a refactor.
