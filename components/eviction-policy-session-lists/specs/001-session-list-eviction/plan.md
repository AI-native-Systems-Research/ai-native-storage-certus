# Implementation Plan: Session-Lineage Eviction Policy

**Branch**: `feat/component-eviction-policy-session-lists` | **Date**: 2026-08-03 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/001-session-list-eviction/spec.md`

## Summary

Implement `eviction-policy-session-lists`, a Certus component that provides an alternative to LRU eviction by exploiting per-session block lineage. Each session is a single linear chain (stack) of cache blocks; only leaves (stack tops) are eligible for eviction, and the victim is the globally oldest-accessed leaf across all sessions in a pool. This protects session prefixes (heads/interior blocks) that plain LRU would drop.

Technical approach: an index-based arena per pool (mirroring the sibling `eviction-policy-lru` `LruList` pattern) where each node carries a parent/child link, an owning session, and a monotonic access stamp. A `BTreeSet` of `(stamp, node_index)` over the current leaves gives ordered victim selection that scales with the number of active sessions rather than total blocks. Session association is delivered by **extending the existing `IEvictionPolicy::track`** in `components/interfaces` with a by-value `semantics: BlockSemantics` argument (an extensible hint struct carrying a required `session_id`); existing implementors ignore it and session-unaware callers pass `BlockSemantics::default()`. The component implements this single, unified `IEvictionPolicy`, reading `session_id` to build lineage; recency-LRU behavior is available by assigning each block a distinct `session_id`.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75 (workspace-inherited)

**Primary Dependencies**: `component-framework`, `component-core`, `component-macros`, `interfaces` (all workspace crates); `criterion` (dev-dependency, benchmarks)

**Storage**: N/A — tracking state is in-memory only; not persisted (per Clarifications). Rebuilt by the caching subsystem on restart.

**Testing**: `cargo test` (unit + doc tests, single-threaded-clean for CI `--test-threads 1`); Criterion benches via `cargo bench`. No hardware dependency.

**Target Platform**: Linux (RHEL/Fedora), consistent with the constitution.

**Project Type**: Single Rust library crate — a Certus COM-style component (`define_component!`).

**Performance Goals**:
- Register / access-refresh / remove: cost independent of total tracked blocks; O(log S) in the number of active sessions S (a leaf transition updates the ordered leaf set), O(1) when touching an interior block.
- Victim selection (`identify_next_to_evict`): O(log S), independent of total block count, bounded on ≥1M-block pools (SC-003).
- `batch_touch` sustains the cache hot-path access rate (SC-005).
- Lineage invariants hold after any operation sequence (SC-006).

**Constraints**: `cargo fmt --check` clean; `cargo clippy -- -D warnings` clean; `cargo doc --no-deps` warning-free; no `unsafe` (none needed); all behavior exposed only through interfaces defined in `components/interfaces`.

**Scale/Scope**: Pools per instance ≈ 16 (memory-tier) + 1 (dispatch-map). Up to ≥1M blocks per pool; number of active sessions per pool is workload-dependent and expected to be far smaller than block count.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|-----------|------|--------|
| I. Code Quality & Maintainability | `fmt`/`clippy -D warnings`/`doc` clean; small modules; `// SAFETY:` on any unsafe | PASS — planned; no `unsafe`; logic split into a `session_list` module + component facade |
| II. Correctness Assurance | Behavior specified before impl; tests fail-if-regressed; property test for lineage invariants (SC-006) | PASS — unit + property + doc tests planned before/with impl |
| III. Comprehensive Testing | Every public API: correctness unit test + perf test where relevant + Rust doc test | PASS — public surface is the component + the extended `IEvictionPolicy`; doc tests added on the extended `track` and component; unit tests per method |
| IV. Performance Discipline | Criterion benches committed for perf-sensitive code with measurable targets | PASS — `benches/session_list_benchmark.rs` planned (track / touch / evict / batch_touch); targets in Performance Goals |
| V. Component-Framework Conformance & Interface Encapsulation | Conforms to `define_component!`/`define_interface!`; only interfaces exposed; interfaces live in `components/interfaces` | PASS — `SessionId` + `BlockSemantics` added and `IEvictionPolicy::track` extended in `components/interfaces`; component modules are `pub(crate)`; no component-local public fns |
| VI. Linux Platform Target | Builds/runs on Linux; Rust stable, edition 2021, MSRV 1.75 | PASS — pure-Rust, no platform-specific facilities |

**Result**: PASS (initial). No violations → Complexity Tracking left empty.

**Post-Design Re-check (after Phase 1)**: PASS (unchanged). The design keeps `unsafe`-free code split into a `session_list` module plus a thin facade (I), specifies behavior and lineage invariants with property tests before implementation (II, SC-006), places correctness + doc tests on every public method and a Criterion suite on the hot paths (III, IV), and confines the shared-crate change to adding `SessionId` + `BlockSemantics` and extending `IEvictionPolicy::track` in `components/interfaces`, with no component-local public functions (V). The extension requires mechanical, behavior-preserving updates to `eviction-policy-lru` (ignored param) and callers (`dispatch-map`, `memory-tier` pass `BlockSemantics::default()`) — these are edits to existing crates, not new public surface, and introduce no new violations.

**Note on shared-crate change**: Session awareness is added by extending `IEvictionPolicy::track` with a by-value `semantics: BlockSemantics` argument (plus new `SessionId`/`BlockSemantics` types), all in `components/interfaces` per Principle V. This modifies the shared trait, so the existing implementor (`eviction-policy-lru`) gains an ignored `_semantics` parameter and existing callers (`dispatch-map` ×3, `memory-tier` ×1, plus test/bench sites) pass `BlockSemantics::default()`. The change is mechanical and behavior-preserving — no existing eviction behavior changes — and keeps a single unified interface rather than proliferating a second one. This is interface evolution within the methodology, not a violation; the wider-but-mechanical blast radius is an accepted, directed trade-off (see research.md Decision 1).

## Project Structure

### Documentation (this feature)

```text
specs/001-session-list-eviction/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output (interface contract)
│   └── ieviction_policy_track.md
└── checklists/
    └── requirements.md  # From /speckit-specify + /speckit-clarify
```

### Source Code (repository root)

```text
components/interfaces/src/
├── ieviction_policy.rs       # MODIFIED: add `SessionId` + `BlockSemantics`; extend track() with by-value `semantics: BlockSemantics`
└── lib.rs                    # re-export SessionId, BlockSemantics

components/eviction-policy-lru/src/
└── lib.rs                    # MODIFIED: track() gains ignored `_semantics` param; internal test call sites pass BlockSemantics::default()

components/dispatch-map/src/lib.rs    # MODIFIED: pass BlockSemantics::default() at 3 track() call sites
components/memory-tier/src/lib.rs     # MODIFIED: pass BlockSemantics::default() at 1 track() call site

components/eviction-policy-session-lists/
├── Cargo.toml                # add criterion dev-dependency + [[bench]]
├── src/
│   ├── lib.rs                # component: provides [IEvictionPolicy] (session-aware track)
│   └── session_list.rs       # NEW: index-based arena + per-session chains + leaf ordering
├── benches/
│   └── session_list_benchmark.rs  # NEW: Criterion suite
└── tests/
    └── lineage_properties.rs # NEW: property/invariant tests (SC-006)
```

**Structure Decision**: Single library crate following the sibling `eviction-policy-lru` layout: a private `session_list` module holds the data structure and its own unit tests; `lib.rs` is the thin component facade implementing the (now session-aware) `IEvictionPolicy` over `RwLock<EvictionState>` with per-pool `Mutex<Pool>`. New shared types go in `components/interfaces` per Principle V. Extending `track` is a mechanical, behavior-preserving change to the existing implementor (`eviction-policy-lru`) and callers (`dispatch-map`, `memory-tier`), which pass `BlockSemantics::default()`. A Criterion bench crate and an integration-level property test round out the required testing surface.

## Complexity Tracking

> No Constitution Check violations — this section is intentionally empty.
