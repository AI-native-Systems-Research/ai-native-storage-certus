# Sync Apply Report

Applied: 2026-07-09
Based on: proposals from 2026-07-09
Backups: `.specify/sync/backups/2026-07-09/`

## Changes Made

### Code changed (P1 — ALIGN, feature removal)

| File | Change |
|------|--------|
| `components/interfaces/src/izyre.rs` | Removed `shout_multi` / `whisper_multi` from the `IZyreNode` trait |
| `components/zyre/src/node.rs` | Removed both `shout_multi` / `whisper_multi` implementations |

### Specs updated

| File | Requirement | Change |
|------|-------------|--------|
| `spec.md` | FR-004 | Modified — single-frame only; memory-bounded; no `_multi` |
| `spec.md` | Clarifications | Added Session 2026-07-09 entry; marked 2026-07-01 payload-API answer superseded |
| `spec.md` | Assumptions | Modified — single-frame payload, no multi-frame variants |
| `data-model.md` | ZyreEvent invariant | Modified — single message frame, memory-bounded, no multi-frame representation |
| `data-model.md` | ZyreEvent (P3) | Added — documented `peer()`/`peer_name()`/`group()` accessors |
| `contracts/izyre.md` | IZyreNode trait | Modified — removed the two `_multi` lines |
| `quickstart.md` | example | Modified — replaced `shout_multi` example with single-frame `shout` |
| `tasks.md` | T008–T013 (P2) | Modified — rewritten to the factory design; added a supersede note |
| `tasks.md` | T048–T049 (P2) | Modified — rewritten to `create_node` factory + interfaces export |
| `tasks.md` | T050 (P2) | Marked removed (multi-frame variants dropped) |

### New specs created

- (none)

## Verification

- `cargo build -p zyre` → **Finished** (clean)
- `cargo test -p interfaces` → **28 passed, 0 failed**
- `cargo test -p zyre --lib` → **5 passed, 0 failed**
- `cargo test -p zyre --doc` → **1 passed, 0 failed**
- `grep shout_multi|whisper_multi` over code + specs → only the intentional strikethrough in `tasks.md` T050 remains

Networked integration tests (`tests/integration.rs`) were not run in this pass;
they exercise real localhost discovery and do not touch the changed API surface.

## Not Applied

| Proposal | Reason |
|----------|--------|
| (none) | All three approved proposals applied |

## Drift status after apply

- **D-1** (multi-frame receive) — RESOLVED (feature removed; send/receive now symmetric).
- **D-2** (stale `tasks.md`) — RESOLVED (rewritten to factory design).
- **D-3** (stale prior report) — RESOLVED by the earlier report regeneration.
- **SC-004** — now fully aligned (no multi-frame path → no information loss).
- Unspecced accessors — RESOLVED (documented in `data-model.md`).

## Next Steps

1. Review the diff:
   `git diff components/interfaces/src/izyre.rs components/zyre/src/node.rs components/zyre/specs/001-zyre-bindings/`
2. Commit:
   `git add components/interfaces components/zyre && git commit -m "sync: drop multi-frame send API; align zyre specs with factory design"`
3. Remaining verification follow-ups (need Linux + C deps; unchanged by this pass):
   - SC-001: tighten the round-trip test to assert the 2s bound.
   - SC-003: run T042/T055 (clean build < 5 min).
   - SC-005: run the suite under Miri/valgrind.
