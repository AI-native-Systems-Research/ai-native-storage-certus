---
spec_sync_component: zyre
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-10-14T00:35:03Z
spec_sync_git_commit: 99cf8f79
spec_sync_inputs_sha256: 65a5a4f9220d0d641d12270c418871f4d6d85b316de604114d44425999653d96
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Sync 2026-10-14 (branch fix/zyre-printf-format, full re-analysis at `85ae2a1f`; supersedes the scoped 2026-10-13 review).** Re-checked all 17 requirements of `001-zyre-bindings` (FR-001..FR-012, SC-001..SC-005), plus the user stories, edge cases, `data-model.md` and `contracts/izyre.md`. Sources: `src/{lib,node,ffi}.rs`, `build.rs`, `deps/build_zyre.sh` and `components/interfaces/src/izyre.rs` (`interfaces` is unchanged on this branch). **The branch's two fixes agree with the spec and resolve two drifts:** (1) `1aceacbb` passes header values, the node endpoint and the gossip endpoints to libzyre's printf-style setters as a `%s` argument (`src/node.rs:87-90,95,117,125,129`). Before it, a `%` was mangled (FR-005, "metadata shared during discovery", `data-model.md:72`) and `%s`/`%n` was undefined behaviour (SC-005). The fix covers every printf-style zyre call the crate makes (`zyre_set_interface` and `zyre_join` take plain strings). `src/repair_test_header_percent.rs:8-51` checks it end to end. (2) `99cf8f79` serialises node creation with a process-wide mutex (`node.rs:51,137-140`), so the guarantee "`create_node` may be called from any thread" (`contracts/izyre.md:57`) now holds. **15/17 aligned, 2 drifted.** FR-012 (moderate): an interior NUL in a header, `interface`, `endpoint` or gossip endpoint makes `create_node` panic (`node.rs:93,94,103,116,124,128`); the contract allows only `InvalidConfig`/`CreateFailed` (`contracts/izyre.md:52-54`). FR-005 (minor): `evasive_timeout_ms`/`expired_timeout_ms` above `i32::MAX` pass `validate` (`izyre.rs:319-328`) and are cast with `as c_int` to a negative value (`node.rs:107-108`). Both are older than this branch (same lines on `origin/unstable`). **Also found, in docs only:** US2 promises "iterator-based event streams" (`spec.md:29`), but the code has no iterator. The lifecycle line in `data-model.md:26` is stale. `contracts/izyre.md:49` says there is no network activity before `start()`, but gossip mode binds the endpoint at creation (`node.rs:115-121`). **4 unspecced behaviours.** Tests were not run: `deps/zyre-build/` is absent here, so verification is by reading the code. 9 proposals, all `approved: false`. Verdict: drift.

> **Sync 2026-10-13 (branch fix/zyre-printf-format, fix: pass header/endpoint/gossip strings to libzyre as a `%s` argument, never as the printf format).** Scope: `ZyreNode::apply_config` (`src/node.rs:84-131`) plus new test `src/repair_test_header_percent.rs`. Findings: the fix resolves unreported drift. Before it, a `%` in a header value, endpoint or gossip endpoint was parsed as a format directive, so peers did not see the configured headers ("metadata shared during discovery", `specs/001-zyre-bindings/data-model.md:72`; FR-005, `spec.md:91`). Values now go through verbatim (`node.rs:88,93,115,123,127`), and the test checks this end to end. The fix adds no error variant and changes no return value. Remaining drift, older than this fix but inside the changed function: `CString::new(..).unwrap()` (`node.rs:91,92,101,114,122,126`) panics when a header, interface or endpoint contains an interior NUL. The contract says `create_node` fails only with `InvalidConfig` or `CreateFailed` (`contracts/izyre.md:52-53`), and `name` already maps a NUL to `InvalidConfig` (`node.rs:52-53`). Also on this branch, after this review: `ZyreNode::new` now serialises `zyre_new` with a process-wide lock (CZMQ's first-use global init is not thread-safe, FND-24); no error, return value or spec-visible behaviour changes. Verdict: drift.

> **Re-stamp 2026-10-09 (transitive: `components/interfaces` changed, branch `fix/evict-blocked-by-pin`).** The only change to this component's hashed inputs is a doc-comment correction on `IDispatchMap::try_evict_to_block` in `components/interfaces/src/idispatch_map.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and its previous stamp was re-verified to match `origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

> **Re-stamp 2026-10-05 (transitive: `components/interfaces` changed, branch `fix/poller-cpu-placement-logging`).** The only change to this component's hashed inputs is a doc-comment correction on `DispatcherConfig::poller_base_cpu` in `components/interfaces/src/idispatcher.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and it does not reference `poller_base_cpu` (verified by grep over its src/ and specs/). Its previous stamp was re-verified to match `git archive origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

> **Re-stamp 2026-10-01 (transitive: `components/interfaces` changed).** The spec-sync digest
> folds `components/interfaces/{src,specs}` into **every** component's hash, so an interface
> change invalidates all of them at once -- which is the design, not a defect. The interface
> change is the `served_by` attribution work: new `ServedBy` and `LookupOutcome` types, and
> `IDispatcher::batch_lookup` widened to return `Vec<LookupOutcome>`.
>
> **No re-analysis was performed for this component, and the digest bump asserts only what was
> actually checked**: the interface change is additive except for `batch_lookup`'s return type,
> which the compiler enforces across all implementors, and this component's own `src/**` and
> `specs/**` are unchanged. Components that implement `IDispatcher` (`dispatcher`,
> `dispatcher-p2p`) had their specs updated substantively; this one did not need it. If a later
> sweep finds drift here, this stamp is not evidence against it.

> **Re-stamp 2026-09-15 (interfaces-fold; no drift).** Branch `fix-reserve-batch-deadline` (`bec6c6ec`) added a `deadline: Option<std::time::Instant>` parameter to `IDispatcher::reserve_memory` in `components/interfaces/src/idispatcher.rs` (shared batch backpressure deadline for the shm-queue OP_RESERVE handler). `scripts/spec-sync-hash.sh` folds the whole `components/interfaces` tree into every component's hash, so this component's digest moved even though its own spec/implementation did not change. This component does not describe or call `reserve_memory`; the interface delta cannot affect its spec↔implementation alignment. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

> **Re-stamp 2026-09-09 (merged-branch interfaces-fold; no drift).** Branch
> `fix-dispatcher-store-backpressure` was merged into `unstable` at `3411518a`; that
> branch adds `DispatcherConfig::store_backpressure_ms` plus two `TierEventStats`
> counters (`store_backpressure_events`, `store_drops_on_full`) to
> `components/interfaces/src/idispatcher.rs`. `scripts/spec-sync-hash.sh` folds
> the whole `components/interfaces/` tree into every component's hash, so this
> component's digest moved even though its own `src/`+`specs/` are byte-for-byte
> unchanged and it references none of those new dispatcher symbols (verified by
> grep across `components/` and `lib/`). The merge's conflict resolution had
> reverted this stamp to unstable's `b220a1c8` value; the digest is recomputed
> here at merged HEAD. Drift status remains `clean`; the report body stands
> verbatim.

# Spec Drift Report

Generated: 2026-10-14T17:38:10Z
Project: zyre (`components/zyre`, spec-kit root `components/zyre/`)
Commit analysed: `85ae2a1f` (branch `fix/zyre-printf-format`; component source last changed at `99cf8f79`)
Branch delta vs `origin/unstable` (component `src/`): `src/node.rs` (+printf fix, +`ZYRE_NEW_LOCK`), new `src/repair_test_header_percent.rs`.

Inputs read in full: `specs/001-zyre-bindings/{spec,data-model,research,plan,quickstart}.md`,
`contracts/izyre.md`, `src/lib.rs`, `src/node.rs`, `src/ffi.rs`, `src/repair_test_header_percent.rs`,
`build.rs`, `Cargo.toml`, `tests/{integration,api_safety}.rs`, `deps/build_zyre.sh`,
`components/interfaces/src/izyre.rs`.

**Verification limits.** `deps/zyre-build/` is absent in this worktree, so `cargo build/test -p zyre`
cannot run here. Every verdict below comes from reading the code (file:line cited). Upstream
zyre v2.0.1 / czmq v4.2.1 sources are also not available locally, so claims about C behaviour are
marked as such. SC-001 (2 s round trip), SC-003 (build time) and SC-005 (valgrind) were **not
re-executed** in this sweep.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 (`001-zyre-bindings`) |
| Requirements Checked | 17 (FR-001..FR-012, SC-001..SC-005) |
| ✓ Aligned | 15 (88%) |
| ⚠️ Drifted | 2 (12%): FR-012 moderate, FR-005 minor |
| ✗ Not Implemented | 0 (0%) |
| ⚠️ Drift outside the FR/SC list (stories, data model, contract) | 3 |
| 🆕 Unspecced Code | 4 |
| Drift resolved by this branch | 2 (printf format → FR-005/SC-005; `create_node` race → `contracts/izyre.md:57`) |

## What this branch changed, checked against the spec

### R1: printf-format fix (`1aceacbb`): resolves drift against FR-005 / SC-005

- libzyre's `zyre_set_header(self, name, format, ...)`, `zyre_set_endpoint(self, format, ...)`,
  `zyre_gossip_bind(self, format, ...)` and `zyre_gossip_connect(self, format, ...)` are printf-style.
  On `origin/unstable` the code passed the user string *as the format*
  (`git show origin/unstable:components/zyre/src/node.rs`, lines 89, 111, 119, 123). The effects:
  - a `%` in a header value was rewritten (e.g. `"100%%"` was announced as `"100%"`). This breaks
    "headers: metadata shared during discovery" (`data-model.md:72`) and FR-005 (`spec.md:91`);
  - `%s` / `%n` / `%x` in a header or endpoint made the C library read (or write, for `%n`) varargs that
    were never passed. That is undefined behaviour and breaks SC-005 (`spec.md:118`, memory safety).
- Now: a constant `"%s"` format is built once (`src/node.rs:90`) and passed at all four call sites
  (`node.rs:95`, `:117`, `:125`, `:129`), with the user string as the argument. The format pointer
  points at a `'static` byte literal, and each `CString` outlives its call.
- **Completeness:** these are the only printf-style zyre calls the crate makes. `zyre_set_interface`
  (`node.rs:104`), `zyre_join`/`zyre_leave` (`:173`, `:182`) and `zyre_new` (`:56`) take plain
  `const char*`. `zyre_shouts`/`zyre_whispers` (printf-style) are not used: `shout`/`whisper` go through
  `zmsg` (`node.rs:191-195`, `:207-211`).
- **Test:** `src/repair_test_header_percent.rs:8-51`. Node A announces header `X-Load = "100%%"` over
  loopback gossip, and node B asserts that its `Enter` event carries exactly `"100%%"` (`:42-47`). This
  fails on the old code (it would read `"100%"`). Only the header path is exercised; the three endpoint
  call sites are checked by reading only.
- No error variant, return value or signature changed. **No spec text covers this behaviour**: see
  U1 / proposal P6.

### R2: `ZYRE_NEW_LOCK` (`99cf8f79`): resolves drift against `contracts/izyre.md:57`

- The contract says: "`IZyre` is `Send + Sync`; `create_node` may be called from any thread"
  (`contracts/izyre.md:57`). According to the commit message and the comment at
  `tests/integration.rs:9-17`, two threads creating their first nodes at once could abort the process
  inside CZMQ's unlocked first-use init (`zsys_init`, `assert (s_open_sockets == 0)`). That broke the
  contract.
- Now: `static ZYRE_NEW_LOCK: Mutex<()>` (`node.rs:137-140`) is taken in `ZyreNode::new` (`node.rs:51`),
  and a poisoned lock is recovered with `into_inner`.
- **Accuracy note (code comment, not spec):** the guard is bound to `_czmq_init`, not `_`, so it lives
  until `new` returns. It therefore serialises `apply_config` too (`node.rs:71`), not only `zyre_new` as
  the comments at `node.rs:50` and `:137` say. This is harmless, and arguably helps (see L1), but the
  comment understates what is locked.
- No spec text says that creation is serialised: see U2 / proposal P7.

## Detailed Findings

### Spec: 001-zyre-bindings - Zyre Rust Bindings

#### Aligned ✓

- **FR-001** safe bindings, no `unsafe` downstream. `mod ffi` is private (`src/lib.rs:28-29`). The public
  surface is the re-exported safe types plus `ZyreComponent` (`lib.rs:32`, `:37-44`), and `ZyreNode` is
  `pub(crate)` (`node.rs:30`). `tests/api_safety.rs:28-46` builds every value type without `unsafe`.
- **FR-002** RAII lifecycle. `Drop` calls `stop()` then `zyre_destroy` (`node.rs:382-387`). Configuration
  is applied at creation (`node.rs:71`, `:86-134`); `start` is at `:144-154`, `stop` at `:157-166`.
  The data-model wording lags (see D-DM).
- **FR-003** all 9 event types, typed. `ZyreEvent` (`izyre.rs:112-156`) and the `parse_event` dispatch
  (`node.rs:450-516`). An unknown type maps to `RecvFailed` (`:515`).
- **FR-004** single-frame `&[u8]`. Send is one `zmsg_addmem` frame (`node.rs:192-194`, `:208-210`).
  Receive reads the first frame via the borrowing `zyre_event_msg` (`node.rs:530-556`). There are no
  `_multi` methods (`izyre.rs:362-425`).
- **FR-006** beacon and gossip. Beacon is the default (no gossip branch). Gossip sets the endpoint, then
  bind/connect (`node.rs:111-131`). An endpoint is required in gossip mode (`izyre.rs:334-345`).
  Test: `tests/integration.rs:172-213`. (In beacon mode the endpoint is silently ignored: see U3.)
- **FR-007** peer introspection. `peers`, `peers_by_group`, `own_groups`, `peer_groups`, `peer_address`
  and `peer_header_value` are at `node.rs:289-379`, with C-owned strings freed (`:348`, `:375`) and lists
  destroyed (`:293`, `:304`, `:314`, `:324`).
- **FR-008** `recv` wraps `zyre_event_new` (`node.rs:228`). `try_recv` uses a zero-timeout `zpoller`
  (`node.rs:255-264`) and spawns no threads. The `Stop` sentinel moves the node to `Done` (`node.rs:235-237`).
  After that, `recv` returns `Stopped` (`:225`) and `try_recv` returns `Ok(None)` (`:247`).
  Test: `tests/integration.rs:215-259`.
- **FR-009** `deps/build_zyre.sh:14-24` pins libzmq v4.3.5, czmq v4.2.1 and zyre v2.0.1, and clones them
  (`:28-36`). It builds with `CMAKE_INSTALL_PREFIX=deps/zyre-build` (`:12`, `:45-51`). Sources land in
  `deps/{libzmq,czmq,zyre}` and the install in `deps/zyre-build`, all under `deps/` (US3 AS3).
- **FR-010** bindgen in `build.rs:68-91`, links `zyre`/`czmq`/`zmq` (`build.rs:35-37`), included by
  `src/ffi.rs:7`. (Has an unspecced override knob: U4.)
- **FR-011** `unsafe impl Send` (`node.rs:39`), no `Sync` impl. `Cell<State>` (`node.rs:32`) makes the
  type `!Sync`, and `IZyreNode: Send` has no `Sync` supertrait (`izyre.rs:362`). Test gap:
  `api_safety.rs:20-26` ("is_not_sync") only asserts `Send` and checks nothing about `!Sync`.
  The guarantee holds by construction (see Recommendations).
- **SC-001** test `round_trip_within_two_seconds` (`tests/integration.rs:271-338`) asserts a < 2 s
  A→B→A round trip. It is gated by the `integration-tests` feature (`integration.rs:1`, `Cargo.toml:15-16`).
  Not re-run here.
- **SC-002** zero `unsafe` in the public API (same evidence as FR-001).
- **SC-003** build time was measured at 2.27 s in the 2026-07-14 pass (prior `proposals.md`). Not
  re-measured.
- **SC-004** 9 variants, and `Stop` is reachable (`node.rs:514`, `:235-237`).
- **SC-005** the printf fix (R1) removes a UB path, and the D1-D3 leak fixes from 2026-09 are still in
  place (`node.rs:530-556`, `:340-355`, `:369-378`). The valgrind harness `run-valgrind.sh` and
  `valgrind.supp` exist; not re-run here. The suppression-scope caveat from the 2026-09-03 sweep still
  applies.

Acceptance scenarios and edge cases checked:
- Join before `start()` returns an error: `ensure_running` → `NotStarted` (`node.rs:78-84`, `:170`). ✓
- Same names coexist: no uniqueness check anywhere. ✓
- Whisper to a departed UUID returns `Ok`: `tests/integration.rs:345-387`. ✓
- US2 AS4 typed `NodeConfig` with public fields + `Default`: `izyre.rs:267-304`. ✓
- *Unverified* "network interface does not exist → start returns an error" (`spec.md:80`). The code maps
  a nonzero `zyre_start` to `StartFailed` (`node.rs:148-151`), but whether zyre v2.0.1 returns nonzero
  for a missing interface could not be checked here, and no test covers it.
- *Unverified by test* US1 AS4 "stopped and dropped → peers receive EXIT" (`spec.md:23`). Drop does call
  `zyre_stop` (`node.rs:384`, `:162-163`), but no test waits for `Exit`.

#### Drifted ⚠️

- **FR-012** (`spec.md:98`, "typed errors ... covering all failure modes (... invalid configuration ...)"),
  together with US2 AS2 (`spec.md:38`, "any fallible operation ... returns a `Result`") and
  `contracts/izyre.md:52-54` (`create_node` errors: `InvalidConfig` | `CreateFailed`).
  The code instead **panics** when a configured string contains an interior NUL:
  `CString::new(..).unwrap()` for header key/value (`node.rs:93`, `:94`), `interface` (`:103`),
  `endpoint` (`:116`), gossip `bind` (`:124`) and gossip `connect` (`:128`). The same file already maps a
  NUL to `InvalidConfig` for `name` (`node.rs:54-55`), `group` (`:171-172`) and `peer` (`:205-206`).
  `NodeConfig::validate` (`izyre.rs:313-347`) does not check for NULs. As a result `research.md:56`
  ("an invalid config is rejected without allocating a node") does not hold for these fields: the panic
  comes after `zyre_new`, and unwinding destroys the node.
  - Severity: **moderate**. A panic crosses the `IZyre::create_node` interface on input the API accepts
    by type. It predates this branch (`origin/unstable` `node.rs:87,88,97,110,118,122`).
  - Resolution: ALIGN (P1).
- **FR-005** (`spec.md:91`, config "validated when a node is created"; `data-model.md:75-76`, `:81-86`).
  `evasive_timeout_ms` and `expired_timeout_ms` are `u32` (`izyre.rs:279-281`). `validate` checks only
  `evasive > 0` and `expired > evasive` (`izyre.rs:319-328`). `apply_config` then casts both with
  `as libc::c_int` (`node.rs:107-108`), so a value above `i32::MAX` (e.g. `3_000_000_000`) passes
  validation and reaches zyre as a negative timeout.
  - Severity: **minor**, since such timeouts are not realistic, but a config that validation accepts
    silently produces a different configuration. Predates this branch (`origin/unstable` `node.rs:101-102`).
  - Resolution: ALIGN (P2).

#### Drift outside the FR/SC list ⚠️

- **D-US2** (`spec.md:29`): "...Result-based error handling, **iterator-based event streams**, and
  **Send+Sync safety where appropriate**." No `Iterator`/`IntoIterator` exists for events in
  `src/` or `izyre.rs` (grep: no matches). Events come from `recv()`/`try_recv()` (`izyre.rs:393`,
  `:400`). The node handle is deliberately `Send` but not `Sync` (FR-011). The story text predates the
  "Direct recv" clarification (`spec.md:132`). Minor, documentation only. BACKFILL (P3).
- **D-DM** (`data-model.md:26`): "**Lifecycle**: Created → Configured → Started → Running → Stopped
  (via drop)". This contradicts both the field row two lines above (`data-model.md:19`) and the state
  diagram (`data-model.md:126-142`), which match the code: `enum State { Created, Running, Draining, Done }`
  (`node.rs:13-18`). There is no separate Configured state (config is applied in `new`, `node.rs:71`), and
  `Stop` is reached by `stop()`, not only by drop. `data-model.md:30` ("Drop behavior: Calls `zyre_stop()`
  then `zyre_destroy()`") is also imprecise: `zyre_stop` runs only from `Running` (`node.rs:162`, `:384`).
  Minor. BACKFILL (P4).
- **D-CTR** (`contracts/izyre.md:49`): "No network activity occurs until `start()` is called". In gossip
  mode `apply_config` calls `zyre_set_endpoint` during `create_node` and treats a nonzero return as
  "failed to bind node endpoint" (`node.rs:115-121`). So the data endpoint is bound (a listening TCP
  socket is opened) before `start()`, and a bind failure is reported by `create_node` as `InvalidConfig`.
  The contract's error list describes `InvalidConfig` only as "config validation failed" (`:53`).
  Minor. BACKFILL (P5).

#### Not Implemented ✗

None.

### Unspecced Code 🆕

| # | Feature | Location | Lines | Suggested Spec |
|---|---------|----------|-------|----------------|
| U1 | Configured strings (header key/value, endpoint, gossip bind/connect) are passed to zyre **verbatim**; `%` has no special meaning. Regression-tested. | `src/node.rs:87-90,95,117,125,129`; `src/repair_test_header_percent.rs:1-51` | ~55 | `001-zyre-bindings` data-model `NodeConfig` (P6) |
| U2 | Node creation is serialised process-wide (`ZYRE_NEW_LOCK`, held across `zyre_new` and `apply_config`) because CZMQ's first-use global init is not thread-safe | `src/node.rs:50-51,137-140` | 7 | `contracts/izyre.md` Thread Safety (P7) |
| U3 | `NodeConfig.endpoint` is applied only in gossip mode; in beacon mode a configured `endpoint` is silently ignored | `src/node.rs:111-122` | 12 | `data-model.md:78` (P8) |
| U4 | `ZYRE_BUILD_DIR` env override of the `deps/zyre-build` location, and DT_RPATH embedding so the transitive libczmq/libzmq resolve without `LD_LIBRARY_PATH` | `build.rs:5-18,26-34,39` | ~25 | FR-010 (P9) |

## Inter-Spec Conflicts

- Within `data-model.md`: line 26 (lifecycle) contradicts lines 19 and 126-142 (D-DM).
- `research.md:56` ("rejected without allocating a node") vs the code's NUL panic and endpoint-bind
  failure, both after `zyre_new` (FR-012 drift, D-CTR).
- Otherwise none. `spec.md`, `plan.md`, `tasks.md`, `research.md` and `quickstart.md` agree on the factory
  / no-builder design. The 2026-07-14 proposals P1-P4 (plan/tasks/FR-008/research wording) are applied in
  the current text (`plan.md:8,34,83`; `tasks.md:39-44`; `spec.md:94`; `research.md` `InvalidConfig` line),
  and P5 (observable `Stop`) is applied in code and docs (see `apply-report.md`).

## Leads not verified (upstream source unavailable here)

- **L1: `zyre_set_interface` may be process-global.** In zyre v2.0.1 it is, as far as recalled, a
  forward to czmq's `zsys_set_interface` ("implemented by zsys global for now"). If so,
  `NodeConfig.interface` is not per-node: the last `create_node` sets it for every node in the process,
  and beacon threads of nodes that are already running could read it while it is being freed. The spec
  presents it as per-node (`data-model.md:74`). **Check against the pinned `zyre.c` before acting.** No
  proposal is raised because this is unverified.
- Whether `zyre_start` returns nonzero for a nonexistent interface (edge case `spec.md:80`).

## Recommendations

1. Fix FR-012 (P1): reject interior NULs in `NodeConfig::validate` so `create_node` returns
   `InvalidConfig` before any FFI call. This also makes `research.md:56` true.
2. Fix FR-005 (P2): bound the two `u32` timeouts at `i32::MAX` in `validate`, and add the rule to
   `data-model.md` validation.
3. Apply the doc backfills P3-P9.
4. Tests: add a real `!Sync` assertion (e.g. a `static_assertions::assert_not_impl_any!` or a trybuild
   compile-fail) in place of the no-op `api_safety.rs:20-26`. Add an integration test for US1 AS4 (peer
   sees `Exit` after drop) and one for the missing-interface edge case.
5. Test hygiene (not spec drift): `src/repair_test_header_percent.rs` does real loopback network I/O on
   fixed ports 19986-19988. Unlike `tests/integration.rs` it is **not** gated by the `integration-tests`
   feature, does not take that file's `SERIAL_LOCK`, and does not scale its 10 s deadline by
   `ZYRE_TEST_TIMEOUT_SCALE`. Consider moving it into `tests/integration.rs`, or gating it the same way.
6. Correct the `ZYRE_NEW_LOCK` comments (`node.rs:50`, `:137`) to say the lock covers creation *and*
   configuration.
7. Resolve L1 against the pinned zyre source.
