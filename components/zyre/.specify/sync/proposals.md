# Drift Resolution Proposals

Generated: 2026-07-14T19:52:47Z
Based on: drift-report from 2026-07-14T19:52:47Z

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 4 |
| Align (Spec → Code) | 0 |
| Human Decision | 1 |
| New Specs | 0 |
| Remove from Spec | 0 |
| Follow-ups (verification) | 3 |

**Context vs the 2026-07-09 proposals**: The prior run's P1 (drop `_multi`) and
P3 (document `ZyreEvent` accessors) were genuinely applied — the code has no
`_multi` methods and `data-model.md` documents the accessors. But prior **P2
(rewrite `tasks.md` to the factory design) was marked `applied: true` yet only
partially landed**: the superseded-design note and the T050 strike are present,
but the individual task bodies (T015, T027-T033, T043, T046, …) still name the
deleted `event.rs`/`builder.rs`/`error.rs`/`peer.rs` files and "builder pattern".
P2 is re-opened below. The dominant **new** item is `plan.md`, which the prior
run never touched and which is now the most stale artifact.

All proposals below are **BACKFILL** (code + the reviewed `spec.md`/
`data-model.md`/`contracts` are authoritative; the stale planning docs should be
updated to match) — except P5, which needs a human design decision.

---

## Proposal 1: 001-zyre-bindings / plan.md — Rewrite to the factory / no-builder design

**Direction**: BACKFILL (Code → Spec)  ·  **Confidence**: HIGH

**Current State**:
- `plan.md:8` — "presents a Rust-native API with RAII, typed events, **builder configuration**, and Result-based errors."
- `plan.md:34` — "6 focused modules (node, **event, builder, error, peer**, ffi)."
- `plan.md:57-70` — source tree lists `src/event.rs`, `src/builder.rs`, `src/error.rs`, `src/peer.rs` (none exist), omits that value types live in the `interfaces` crate, and lists only `tests/integration.rs`.
- Code does: no builder (`NodeConfig` is public-fields + `Default`, `izyre.rs:260`); the `zyre` crate has 3 files (`lib.rs`/`node.rs`/`ffi.rs`); value types + `IZyre`/`IZyreNode` traits live in `interfaces/src/izyre.rs`; tests are `integration.rs` + `api_safety.rs`.

**Proposed Resolution** (concrete edits):

- `plan.md:8` →
  > … and presents a Rust-native API with RAII, typed events, public-field configuration structs (`NodeConfig`/`GossipConfig`, validated at `create_node`), and Result-based errors. The `IZyre` component interface acts as a factory returning `Box<dyn IZyreNode>` handles.
- `plan.md:34` (Maintainability row Notes) →
  > Small crate — `lib.rs` (component + `IZyre` impl), `node.rs` (crate-private FFI-owning `ZyreNode` + event parsing), `ffi.rs` (bindgen re-export). Value types and the `IZyre`/`IZyreNode` traits live in the `interfaces` crate (`izyre.rs`) to avoid a crate cycle. Minimal public surface.
- `plan.md:57-70` (Source Code tree) → replace with:
  ```text
  components/zyre/
  ├── Cargo.toml
  ├── src/
  │   ├── lib.rs           # define_component! + IZyre impl; re-exports interface types
  │   ├── node.rs          # ZyreNode (crate-private) — safe wrapper over zyre_t + event parsing
  │   └── ffi.rs           # include! of bindgen-generated bindings
  ├── build.rs             # bindgen invocation, link configuration
  └── tests/
      ├── integration.rs   # Two-node localhost discovery/shout/whisper/gossip tests
      └── api_safety.rs    # Send / !Sync / no-unsafe compile-time assertions

  components/interfaces/src/
  └── izyre.rs             # IZyre + IZyreNode traits; NodeConfig, GossipConfig,
                           #   ZyreEvent, PeerId, ZyreError (here to avoid a crate cycle)
  ```
- `plan.md:82` (Structure Decision) → add a sentence: "The `IZyre`/`IZyreNode` traits and all value types live in the `interfaces` crate so `IZyre::create_node` can name them without a cycle; the concrete `ZyreNode` is crate-private in `zyre`."

**Rationale**: Documentation catch-up to shipped, tested code and the already-reviewed `spec.md`/`data-model.md`/`contracts/izyre.md`. No code impact. Highest value because CLAUDE.md directs contributors to read `plan.md` for structure.

**Action**: [ ] Approve  ·  [ ] Reject  ·  [ ] Modify

---

## Proposal 2: 001-zyre-bindings / tasks.md — Finish the factory rewrite (re-open prior P2)

**Direction**: BACKFILL (Code → Spec)  ·  **Confidence**: HIGH

**Current State**: The top-of-file superseded-design note (`tasks.md:39-44`) and the T050 strike (`:178`) are in place, but the task bodies were not rewritten:
- `tasks.md:67` (T015) — "in `components/zyre/src/event.rs`"
- `tasks.md:89,91` — phase goal / independent test reference "builder pattern" and "verify builder validates config"
- `tasks.md:95` (T027) — "`NodeConfig` builder validation … in `components/zyre/src/builder.rs`"
- `tasks.md:96` (T028) — doc tests in "`builder.rs`, `event.rs`"
- `tasks.md:102-104` (T031-T033) — "`event.rs`" / "`builder.rs`" / "`error.rs` and `peer.rs`"
- `tasks.md:143,149` (T043, T046) — `GossipConfig`/gossip docs "in `components/zyre/src/builder.rs`"
- `tasks.md:218,233,237` — example/parallelization notes citing `event.rs`/`builder.rs`

**Proposed Resolution**:
- Replace every `src/event.rs`, `src/builder.rs`, `src/error.rs`, `src/peer.rs` reference with the actual location — value types + traits in `components/interfaces/src/izyre.rs`, component glue in `components/zyre/src/{lib,node,ffi}.rs`.
- Replace "builder pattern" / "builder validates config" (`:89`,`:91`,`:95`) with "public-field `NodeConfig` + `Default`, validated by `create_node`".
- Leave the `:39-44` note and the T050 strike as-is (already correct).

**Rationale**: Completes the documentation catch-up the prior P2 claimed but did not fully apply. No code impact.

**Note**: Correct the prior `proposals.json` P2 status — it was recorded `applied: true` but the body edits did not land.

**Action**: [ ] Approve  ·  [ ] Reject  ·  [ ] Modify

---

## Proposal 3: 001-zyre-bindings / FR-008 wording — recv wraps `zyre_event_new`

**Direction**: BACKFILL (Code → Spec)  ·  **Confidence**: HIGH

**Current State**:
- `spec.md:94` (FR-008) — "provide a direct `recv()` method that blocks the calling thread (thin wrapper over `zyre_recv`) …"
- Code does: `recv()` uses `zyre_event_new`/`zyre_event_destroy` (`node.rs:180-191`), the higher-level typed-event API (which internally calls `zyre_recv`).

**Proposed Resolution**: Reword the parenthetical in FR-008 to:
  > … (wraps `zyre_event_new`, the typed-event API, which internally receives via `zyre_recv`) …

**Rationale**: The code choice is correct (typed parsing); only the spec wording is imprecise. Trivial edit.

**Action**: [ ] Approve  ·  [ ] Reject  ·  [ ] Modify

---

## Proposal 4: 001-zyre-bindings / research.md:84 — Stale "builder" phrasing

**Direction**: BACKFILL (Code → Spec)  ·  **Confidence**: HIGH

**Current State**: `research.md:84` — "`InvalidConfig(String)` — builder validation failure". There is no builder.

**Proposed Resolution**: Change to "`InvalidConfig(String)` — config validation failure (from `NodeConfig::validate`)".

**Rationale**: One-word correction; keeps the error-taxonomy section consistent with the no-builder decision recorded two lines above (`research.md:56`).

**Action**: [ ] Approve  ·  [ ] Reject  ·  [ ] Modify

---

## Proposal 5: 001-zyre-bindings / ZyreEvent::Stop reachability — HUMAN DECISION

**Direction**: HUMAN_DECISION

**Current State**: `ZyreEvent::Stop` is representable (`izyre.rs:148`) but undeliverable: `recv`/`try_recv` return `NotStarted` once `stop()` flips `started` (`node.rs:114-119` vs `:180-183`). No test or doc addresses this.

**Options**:
- **A — Document as internal-only** (spec/code comment): STOP is not surfaced through `recv`; callers detect shutdown via their own control flow. Smallest change; keeps `stop()` semantics simple.
- **B — Make STOP observable**: have `stop()` not gate `recv` immediately (or drain the pending STOP event before flipping `started`), so a caller polling `recv` sees `ZyreEvent::Stop`. Larger behavioral change; needs a test.

**Questions for the human**:
1. Is any consumer expected to observe `ZyreEvent::Stop` via `recv`, or is STOP purely a C-layer artifact the binding can hide?
2. If hidden, should the `Stop` variant remain in the public enum (for completeness / SC-004) or be removed?

**Confidence**: MEDIUM (both interpretations valid; leaning A given the single-threaded `&mut self` model).

**Action**: [ ] Choose A  ·  [ ] Choose B  ·  [ ] Other

---

## Follow-ups (verification tasks) — RESOLVED 2026-07-14

All three SC gaps from the drift report were addressed:

- **SC-001** ✓ — added `round_trip_within_two_seconds` (integration), asserting a real A→B→A exchange completes within the 2 s bound. Also made the discovery/whisper tests resend-in-loop and added a `ZYRE_TEST_TIMEOUT_SCALE` knob so they survive valgrind's slowdown.
- **SC-003** ✓ — measured a from-scratch `cargo build -p zyre` (incl. bindgen) at 2.27 s, far under the 5-minute budget. The one-time C-deps build (`build_zyre.sh`) is separate and unchanged.
- **SC-005** ✓ — Miri can't cross FFI, so added a committed valgrind harness (`run-valgrind.sh` + `valgrind.supp`). memcheck over the lib + integration suites reports 0 errors / 0 bytes lost attributable to the bindings (only C-library-internal reports suppressed, documented).

Still open: the whisper-to-departed-UUID fire-and-forget test (`spec.md:81`) — optional, not an SC gate.
