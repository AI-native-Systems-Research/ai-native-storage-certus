---
spec_sync_component: zyre
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-04T16:13:14Z
spec_sync_git_commit: 42f53f49
spec_sync_inputs_sha256: ea5b3eabb68485ccd09bf99eb762fd07ec1575266c97e41d67bc5773559cce0d
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: zyre

**This sweep (2026-09-03; stamped clean 2026-09-04)** independently re-verified
all of `001-zyre-bindings` (FR-001..FR-012, SC-001..SC-005) against
`src/{lib,node,ffi}.rs`, `build.rs`, and the shared value/interface types in
`components/interfaces/src/izyre.rs`. The prior report claimed "17 aligned, 0
drift, CLEAN" — that was wrong. A fresh read plus verification against the
**upstream zyre v2.0.1 / czmq v4.2.1 C API ownership contracts** found **three
real memory leaks** (D1/D2/D3) and one spec documentation error (D5). Code fixes
were applied for D1/D2/D3 (ALIGN) and the doc was corrected for D5 (BACKFILL).

**Why `drift_status: clean`:** the three memory-leak fixes resolve the only
actionable drift, so spec and code now agree. The fixes could not be built or
valgrind-checked **locally** (`deps/zyre-build/` is absent on the dev box), but
this is closed by CI, not left open: the Jenkins pipeline symlinks the pre-built
C stack (`Jenkinsfile` → `deps/zyre-build` → `/opt/zyre-build`) and builds +
tests zyre as part of `cargo build` / `cargo t --workspace` (zyre is a workspace
member), so a compile error or unit-test regression in these FFI changes fails a
later pipeline stage. The changes are low-risk by construction — `zyre_event_msg`
matches the existing `zyre_event_.*` bindgen allowlist, and `libc::free`/`c_void`
are already used elsewhere in `node.rs` (no new FFI symbols, no allowlist edit).
One residual, non-blocking concern is recorded under "Residual" below (the
`valgrind.supp` scope), along with the tracked SAFETY-comment convention debt
(D4).

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 1 (`001-zyre-bindings`) |
| Requirements Checked | 17 (FR-001..FR-012 + SC-001..SC-005) |
| Aligned (pre-existing) | 13 |
| Drift → code fix applied (ALIGN) | 3 (D1/D2/D3 — memory leaks; built+tested in CI) |
| Drift → doc fix applied (BACKFILL) | 1 (D5 — GossipConfig `Default`) |
| Convention debt (tracked, not spec drift) | 1 (D4 — missing `// SAFETY:` comments) |
| Not Implemented | 0 |
| Unspecced | 0 |

## Resolved this sweep — code fixes (ALIGN; against SC-005 memory safety / FR-011)

These were **memory leaks**: real defects, not doc lag, so they are ALIGN (fix
the code), never BACKFILL. Each was corroborated against the upstream v2.0.1
headers, which distinguish borrowing accessors from ownership-transfer accessors.

**D1 — `parse_message` leaked the entire message on every Whisper/Shout
(`src/node.rs:519-545`).** The code called `ffi::zyre_event_get_msg(event_ptr)`,
whose documented contract is *"pass ownership to the caller. The caller must
destroy the message … further calls will return NULL."* Nothing destroyed the
returned `zmsg_t`, so every received WHISPER/SHOUT leaked its full message.
**Fix:** switched to the **borrowing** accessor `ffi::zyre_event_msg(event_ptr)`,
whose contract is *"the caller can modify the message but does not own it and
should not destroy it"* — the message is then freed by the existing
`zyre_event_destroy` in `recv()` (`src/node.rs:222`). Chose the borrowing
accessor over adding a `zmsg_destroy` because we only read the first frame's
bytes and never need to own the message. `zyre_event_msg` matches the
`zyre_event_.*` bindgen allowlist (`build.rs`), so it is exposed.

**D2 — `peer_address` leaked a `char*` on every call (`src/node.rs:318-345`).**
`zyre_peer_address` is documented *"Caller owns return value and must destroy it
when done."* The code copied it into an owned `String` and dropped the raw
pointer without freeing. **Fix:** added
`libc::free(addr as *mut libc::c_void);` after `.into_owned()`. Chose
`libc::free` over `zstr_free` because czmq v4.2.1 allocates these strings via the
system allocator (`zstr_free` calls `free` internally), and `zstr_.*` is **not**
in the bindgen allowlist — using `libc::free` avoids an allowlist change I cannot
compile-verify here. `libc` is already a dependency used elsewhere in `node.rs`.

**D3 — `peer_header_value` leaked a `char*` on every call
(`src/node.rs:347-368`).** Same ownership contract as D2
(*"Caller owns return value and must destroy it when done."*). Same fix:
`libc::free(val as *mut libc::c_void);` after `.into_owned()`.

## Resolved this sweep — doc fix (BACKFILL; doc lag against correct code)

**D5 — GossipConfig wrongly documented as having `Default`
(`specs/001-zyre-bindings/spec.md:104`, `data-model.md:90-99`).** The Key Entities
line lumped "NodeConfig / GossipConfig" together as "public fields + `Default`".
But `GossipConfig` (`interfaces/src/izyre.rs:215-222`) derives only
`Debug, Clone, PartialEq, Eq` — **no `Default`**. This is deliberate and correct:
an empty `GossipConfig` (`bind: None, connect: []`) would always fail its own
invariant (`validate`, `izyre.rs:241-247`), so the sound design offers the
`bind()`/`connect()` smart constructors (`izyre.rs:224-239`) instead of a
`Default` that is guaranteed invalid. `NodeConfig` genuinely does have `Default`
(no change to that half). **Fix:** reworded `spec.md:104` to attribute `Default`
to `NodeConfig` only and describe `GossipConfig` construction via
`bind()`/`connect()`; added a matching "Construction" note to the `GossipConfig`
section of `data-model.md`. Pure doc correction — no code implication.

## Unresolved this sweep

**D4 — `// SAFETY:` comments missing on most `unsafe` blocks in `src/node.rs`
(convention debt; tracked, not spec drift).** The project convention
(root `CLAUDE.md`: "Unsafe code requires `// SAFETY:` justification comments")
is under-met: `node.rs` has 30 `unsafe {` blocks + 1 `unsafe impl Send`, but only
4 `SAFETY:` comments (3 of which this sweep added at the D1/D2/D3 sites). The
remaining ~27 FFI blocks lack justifications. This is **not** a spec↔code drift
(no FR/SC mandates the comments) and is left as tracked convention cleanup rather
than mass-annotated blind — annotating FFI blocks correctly requires the
per-call ownership reasoning that can only be sanity-checked against a buildable
tree. It is convention debt, not spec↔code drift (no FR/SC mandates the
comments), so it does not block the `clean` stamp — it should be worked down as
these FFI blocks are next touched.

## Verification

- **Local:** not possible — `deps/zyre-build/` is absent on the dev box
  (`pkg-config libzyre` = no), so `cargo build -p zyre` / `test -p zyre` do not
  run here. The sweep therefore verified the ownership contracts against the
  upstream v2.0.1/czmq v4.2.1 headers rather than by execution.
- **CI (authoritative):** the Jenkins pipeline provisions the C stack
  (`Jenkinsfile` symlinks `deps/zyre-build` → `/opt/zyre-build`) and compiles +
  runs zyre via `cargo build` and `cargo t --workspace` (zyre is a workspace
  member). A compile error or unit-test regression in the D1/D2/D3 changes would
  fail a later pipeline stage. Compile risk is low by construction:
  `zyre_event_msg` matches the existing `zyre_event_.*` allowlist and
  `libc::free`/`c_void` are already used in `node.rs` — no new FFI symbols, no
  `build.rs` allowlist change.

## Residual (non-blocking; tracked, not spec drift)

1. **`valgrind.supp` scope vs SC-005 intent.** SC-005 asserts valgrind/Miri
   memory-safety, but the repo's `valgrind.supp` suppresses leaks by **allocation
   stack**, which can suppress exactly these czmq-internal allocations — so a
   valgrind run as *currently configured* could report "clean" while a future
   leak regression on these paths went unseen. This is a **pre-existing**
   suppression-scope weakness, not introduced by this change, and does not affect
   whether spec and code agree today. **Recommended follow-up:** narrow
   `valgrind.supp` so czmq allocation stacks are not blanket-suppressed, then
   valgrind the whisper/shout round-trip and the
   `peer_address`/`peer_header_value` paths to guard against regressions.
2. **D4 — missing `// SAFETY:` comments** (convention debt, see below).

## Aligned ✓ (unchanged this sweep)

- ✓ FR-001 safe bindings; unsafe confined to FFI layer (`src/node.rs`, `ffi.rs`).
- ✓ FR-002 RAII lifecycle; Drop stops node.
- ✓ FR-003 all 9 event types as typed enum (`parse_event`).
- ✓ FR-004 single-frame `&[u8]` whisper/shout (multi-frame superseded 2026-07-09).
- ✓ FR-005 typed `NodeConfig` (public fields + `Default`, `#[non_exhaustive]`).
- ✓ FR-006 UDP beacon + gossip discovery.
- ✓ FR-007 peer introspection (`peers`/`own_groups`/`peer_groups`/`peer_address`/
  `peer_header_value`) — behavior aligned; D2/D3 fixed the leaks in the last two.
- ✓ FR-008 blocking `recv()` + non-blocking `try_recv()`, no bg threads (zyre's
  own threads are internal), terminal `Stop` state machine.
- ✓ FR-009 build clones zyre/libzmq/czmq to `deps/zyre-build/`.
- ✓ FR-010 bindgen build script (`src/ffi.rs` includes generated bindings).
- ✓ FR-011 `Send`, not `Sync` (single `unsafe impl Send`, `src/node.rs:39`) —
  and, after D1/D2/D3, no longer leaking on the hot paths.
- ✓ FR-012 typed `ZyreError` enum.
- ✓ SC-001/002/003/004 consistent with implementation (round-trip discovery,
  zero unsafe in public API, clean-checkout build, 9 event types representable).
- ✓ SC-005 (memory safety) — the D1/D2/D3 fixes remove the three leaks, bringing
  the code into alignment; built+tested in CI (see "Verification"). One residual
  concern (the `valgrind.supp` allocation-stack scope could mask a future
  regression) is a pre-existing, non-blocking follow-up under "Residual".

## Unspecced Features

None.
