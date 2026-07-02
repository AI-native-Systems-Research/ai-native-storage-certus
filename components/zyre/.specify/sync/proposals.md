# Drift Resolution Proposals

Generated: 2026-07-09
Based on: drift-report from 2026-07-09
Mode: interactive

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Align (Spec ↔ Code, remove feature) | 1 |
| Backfill (Code → Spec) | 2 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | (folded into P1) |

User decision (2026-07-09): a single ZeroMQ frame is bounded only by memory
(libzmq `ZMQ_MAXMSGSIZE` defaults to `-1`/unlimited; zyre sets no cap on the
peer mailbox, confirmed in `deps/zyre/src/zyre_peer.c` and
`deps/libzmq/src/options.cpp:191`). The single-frame `&[u8]` API therefore
already carries arbitrarily large payloads, so the `_multi` send methods are to
be **dropped** rather than fixing the lossy multi-frame receive path.

---

## Proposal 1: 001-zyre-bindings / FR-004 (D-1) — Drop the multi-frame send API

**Direction**: ALIGN (remove feature from both spec and code)

**Current State**:
- Spec says (`spec.md:90`, `data-model.md:54`): single-frame `&[u8]` primary API **plus** `_multi` variants; `ZyreEvent` Whisper/Shout have a "multi-frame variant carrying `Vec<Vec<u8>>`".
- Code does: `shout_multi`/`whisper_multi` exist (send side), but `parse_message` reads only `zmsg_first`, so multi-frame messages are truncated to frame 0 on receive; no `Vec<Vec<u8>>` variant was ever added.

**Proposed Resolution**:

*Code changes*
- Remove `shout_multi` and `whisper_multi` from the `IZyreNode` trait — `components/interfaces/src/izyre.rs:374-378` (methods + doc comments).
- Remove both method implementations — `components/zyre/src/node.rs:238-276`.
- No change to `parse_message` / `ZyreEvent` (already single-frame; now consistent).

*Spec changes*
- `spec.md:90` — reword **FR-004** to:
  > **FR-004**: The crate MUST support sending messages (whisper and shout) with a single-frame `&[u8]` payload. A single ZeroMQ frame is bounded only by available memory (libzmq imposes no size cap by default and zyre sets none), so this single-frame API carries arbitrarily large payloads; no multi-frame send variants are provided.
- `spec.md` Clarifications — add a Session 2026-07-09 entry superseding the 2026-07-01 payload-API clarification (line 125):
  > Q: Keep the `_multi` multi-frame send variants? → A: No. A single frame is bounded only by memory (`ZMQ_MAXMSGSIZE` default `-1`, no cap set by zyre), so the single-frame `&[u8]` API already sends arbitrarily large payloads. The `_multi` methods added a send/receive asymmetry (receive only ever surfaced the first frame) for no benefit and are removed. *(Supersedes the 2026-07-01 single-frame-plus-`_multi` decision.)*
- `spec.md:136` — remove the Assumptions bullet stating multi-frame messages are supported via `_multi`.
- `data-model.md:54` — replace the invariant with:
  > - `message` in Whisper/Shout is the payload of the (single) message frame. Payload size is bounded only by memory; there is no multi-frame representation.
- `contracts/izyre.md:75-76` — delete the two `_multi` trait lines.
- `quickstart.md:71` — replace the `shout_multi(...)` example with a single-frame `shout(...)` of a serialized payload.
- `tasks.md:169` — mark **T050** removed/obsolete (see Proposal 2).

**Rationale**: The primary API already covers every payload the `_multi` methods
could send, and the receive path can only ever reconstruct one frame. Removing
the send-side `_multi` methods eliminates the asymmetry, fully aligns **SC-004**
("no loss of information"), and is safe at v0.1.0 — `publish = false` and no
component binds these methods (verified by workspace grep).

**Confidence**: HIGH

**Action**: [x] Approve  (per user decision 2026-07-09)  ·  [ ] Reject  ·  [ ] Modify

---

## Proposal 2: 001-zyre-bindings / tasks.md (D-2) — Rewrite to the factory design

**Direction**: BACKFILL (Code → Spec)

**Current State**:
- `tasks.md` T008–T013, T048–T049 describe separate source files
  (`error.rs`/`peer.rs`/`event.rs`/`builder.rs`), a `NodeConfigBuilder` builder
  API, and a `ping()`-only `IZyre` with consumers calling `ZyreNode::new()`
  directly. T050 adds the `_multi` variants.
- Code + `spec.md`/`data-model.md`/`contracts` use the factory design (types in
  `interfaces/src/izyre.rs`, public-fields `NodeConfig`, `IZyre::create_node`,
  crate-private `ZyreNode`) per commit `b45418d`.

**Proposed Resolution**:
- Rewrite T008–T013 to reflect value types + traits in `interfaces/src/izyre.rs`
  (single file), `NodeConfig` public-fields + `Default` (drop "builder"), and the
  actual module layout (`ffi.rs`, `node.rs`, `lib.rs`).
- Rewrite T048–T049 to the factory `create_node` design (drop the "circular dep /
  use `ZyreNode::new()` directly" language).
- Remove/obsolete **T050** (multi-frame variants) per Proposal 1.

**Rationale**: Documentation catch-up to the already-approved spec artifacts and
shipped code. No code impact.

**Confidence**: HIGH

**Action**: [ ] Approve  ·  [ ] Reject  ·  [ ] Modify   *(pending user confirmation)*

---

## Proposal 3: 001-zyre-bindings / data-model.md — Document ZyreEvent accessors

**Direction**: BACKFILL (Code → Spec)

**Current State**: `ZyreEvent::peer()` / `peer_name()` / `group()` helper accessors
(`interfaces/src/izyre.rs:151-191`) are not mentioned in the spec's Key Entities.

**Proposed Resolution**: Add one line under the `ZyreEvent` entity in
`data-model.md` noting the convenience accessors and their `Option` return shape.

**Rationale**: Eliminates the minor unspecced-code finding; low risk.

**Confidence**: HIGH

**Action**: [ ] Approve  ·  [ ] Reject  ·  [ ] Modify   *(pending user confirmation)*

---

## Follow-ups (not drift proposals — verification tasks)

Carried from the drift report; require the Linux + C-deps environment:
- SC-001: add/adjust a timed round-trip assertion (test uses a 5s deadline, not the 2s bound).
- SC-003: run T042/T055 (clean build < 5 min).
- SC-005: run the suite under Miri/valgrind.
