# Drift Resolution Proposals

Generated: 2026-10-14T17:38:10Z
Based on: drift-report from 2026-10-14T17:38:10Z (`85ae2a1f`, branch `fix/zyre-printf-format`)
Mode: batch. **Every proposal is `approved: false`.** Nothing has been applied.

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 7 (P3-P9) |
| Align (Spec → Code) | 2 (P1, P2) |
| Human Decision | 0 (P8 notes an ALIGN alternative) |
| New Specs | 0 |
| Supersede | 0 |
| Remove from Spec | 0 |

**Status of the 2026-07-14 proposals (replaced by this file):** P1-P4 (rewrite `plan.md`, task
bodies, FR-008 wording, `research.md` `InvalidConfig` line) are now in the spec text
(`plan.md:8,34,83`; `tasks.md:39-44`; `spec.md:94`; `research.md` error-variant list). P5
(observable `Stop`) was applied in code and docs (see `apply-report.md`). None remain open.

**This branch's fixes need no ALIGN.** The printf-format fix (`1aceacbb`) and the creation lock
(`99cf8f79`) bring the code into line with the spec. They are behaviours the spec does not yet
state, and P6/P7 backfill them.

---

### Proposal P1: 001-zyre-bindings / FR-012: interior NUL in config strings panics instead of `InvalidConfig`

**Type**: ALIGN (Spec → Code) · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-012, US2 AS2, `create_node` error contract.

**Spec says**:
- "The crate MUST provide typed errors via a `ZyreError` enum covering all failure modes (start failure, invalid configuration, network errors)." (`specs/001-zyre-bindings/spec.md:98`)
- "**When** any fallible operation is called, **Then** it returns a `Result<T, ZyreError>`" (`spec.md:38`)
- "`ZyreError::InvalidConfig(reason)` — config validation failed / `ZyreError::CreateFailed` — ..." (`contracts/izyre.md:52-54`)
- "`NodeConfig::validate()` runs inside `IZyre::create_node`, before any FFI, so an invalid config is rejected without allocating a node." (`research.md:56`)

**Code does**: `CString::new(..).unwrap()` panics on an interior NUL in a header key
(`components/zyre/src/node.rs:93`), header value (`:94`), `interface` (`:103`), `endpoint` (`:116`),
gossip `bind` (`:124`) or gossip `connect` (`:128`). The panic happens after `zyre_new` (`node.rs:56`).
The same file already maps a NUL to `InvalidConfig` for `name` (`node.rs:54-55`), `group`
(`:171-172`) and `peer` (`:205-206`). `NodeConfig::validate` (`components/interfaces/src/izyre.rs:313-347`)
does not check for NULs. This predates the branch (`origin/unstable` `node.rs:87,88,97,110,118,122`).

**Proposed code change**:
1. In `NodeConfig::validate` (`components/interfaces/src/izyre.rs:313`), return
   `ZyreError::InvalidConfig("<field> contains a NUL byte")` if any header key or value, `interface`,
   `endpoint`, `gossip.bind` or any `gossip.connect` entry contains `'\0'`. The `name` check can move
   there as well.
2. In `apply_config` (`node.rs:93-128`), replace each `.unwrap()` with
   `.map_err(|_| ZyreError::InvalidConfig(..))?` (defence in depth). Alternatively, keep them as
   `.expect("validated: no NUL")` with a comment pointing at `validate`.
3. Tests: unit tests in `izyre.rs` (validate rejects a NUL in each field) and in `node.rs`
   (`ZyreNode::new` returns `InvalidConfig`, no panic).

**Accompanying spec text** (part of the same change, `data-model.md:86`):
- Old: `- if \`gossip\` is set, \`endpoint\` must be set and the gossip config must have at least one of \`bind\`/\`connect\``
- New: `- if \`gossip\` is set, \`endpoint\` must be set and the gossip config must have at least one of \`bind\`/\`connect\``
  followed by a new line `- no string field (\`name\`, header keys/values, \`interface\`, \`endpoint\`, gossip \`bind\`/\`connect\`) may contain an interior NUL byte`

**Rationale**: The spec is clear and consistent: errors are typed and validation comes before FFI.
A panic across the `IZyre` interface on input the type system accepts is a bug, not intended
behaviour. The code already handles the same case for `name`, `group` and `peer`.

**Action**: [ ] Approve · [ ] Reject · [ ] Modify

---

### Proposal P2: 001-zyre-bindings / FR-005: timeouts above `i32::MAX` pass validation and wrap negative

**Type**: ALIGN (Spec → Code) · **Confidence**: MEDIUM · **approved: false**

**Requirement**: FR-005 ("validated when a node is created").

**Spec says**: "a typed `NodeConfig` struct ... for node configuration (name, headers, port, interface,
timeouts, discovery mode), validated when a node is created." (`spec.md:91`). The validation rules
are listed at `data-model.md:81-86`: evasive > 0, expired > evasive.

**Code does**: the fields are `u32` (`izyre.rs:279`, `:281`). `validate` checks only `> 0` and
`expired > evasive` (`izyre.rs:319-328`). `apply_config` passes them as
`config.evasive_timeout_ms as libc::c_int` / `config.expired_timeout_ms as libc::c_int`
(`node.rs:107-108`), so e.g. `3_000_000_000` is accepted and reaches zyre as a negative timeout.
This predates the branch (`origin/unstable` `node.rs:101-102`).

**Proposed code change**: in `NodeConfig::validate`, return `InvalidConfig` when
`evasive_timeout_ms > i32::MAX as u32` or `expired_timeout_ms > i32::MAX as u32`. Optionally use
`c_int::try_from(..)` at `node.rs:107-108`. Add a unit test.

**Accompanying spec text** (`data-model.md:82-83`):
- Old: `- \`evasive_timeout_ms\` must be > 0` / `- \`expired_timeout_ms\` must be > \`evasive_timeout_ms\``
- New: `- \`evasive_timeout_ms\` must be > 0 and <= 2147483647 (\`i32::MAX\`, zyre takes a C \`int\`)` /
  `- \`expired_timeout_ms\` must be > \`evasive_timeout_ms\` and <= 2147483647`

**Rationale**: FR-005 promises that a config which passes validation is the config that gets
applied. A silent sign flip breaks that. Severity is low: such timeouts (~25 days) are not realistic.
Confidence is MEDIUM because the alternative, narrowing the field type to `i32`, would be an
interface change.

**Action**: [ ] Approve · [ ] Reject · [ ] Modify

---

### Proposal P3: 001-zyre-bindings / User Story 2: "iterator-based event streams" and "Send+Sync"

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · **approved: false**

**Requirement**: User Story 2 narrative and AS3.

**Spec says** (`spec.md:29`): "...RAII resource management, Result-based error handling,
iterator-based event streams, and Send+Sync safety where appropriate." And (`spec.md:39`):
"**When** a developer iterates over incoming events, ..."

**Code does**: there is no `Iterator`/`IntoIterator` for events in `components/zyre/src/` or
`components/interfaces/src/izyre.rs` (grep: no matches). Events come from `recv()`/`try_recv()`
(`izyre.rs:393`, `:400`; `node.rs:219-269`), per the "Direct recv" clarification (`spec.md:132`). The
handle is `Send` but deliberately not `Sync` (FR-011; `node.rs:39`, `izyre.rs:362`).

**Exact spec edit**: `spec.md:29`
- Old: `A Rust developer uses the Zyre bindings through an API that follows Rust conventions: RAII resource management, Result-based error handling, iterator-based event streams, and Send+Sync safety where appropriate. The developer should not need to understand the C API or manage raw pointers.`
- New: `A Rust developer uses the Zyre bindings through an API that follows Rust conventions: RAII resource management, Result-based error handling, typed events delivered by a blocking \`recv()\` and a non-blocking \`try_recv()\`, and thread safety expressed in the type system (the node handle is \`Send\` but not \`Sync\`). The developer should not need to understand the C API or manage raw pointers.`

`spec.md:39`
- Old: `3. **Given** the Rust API, **When** a developer iterates over incoming events, **Then** they can use a typed enum (\`ZyreEvent\`) with pattern matching rather than string comparison.`
- New: `3. **Given** the Rust API, **When** a developer receives incoming events (\`recv()\` / \`try_recv()\`), **Then** they can use a typed enum (\`ZyreEvent\`) with pattern matching rather than string comparison.`

**Rationale**: The narrative predates the reviewed decisions (direct `recv`, `Send`-not-`Sync`). The
code follows those decisions, which FR-008 and FR-011 encode. No code impact.

**Action**: [ ] Approve · [ ] Reject · [ ] Modify

---

### Proposal P4: 001-zyre-bindings / data-model: stale lifecycle line and imprecise Drop line

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-002 (lifecycle) as described in `data-model.md`.

**Spec says**: "**Lifecycle**: Created → Configured → Started → Running → Stopped (via drop)"
(`data-model.md:26`) and "**Drop behavior**: Calls `zyre_stop()` then `zyre_destroy()`."
(`data-model.md:30`). These contradict the same file's field row (`:19`) and state diagram (`:126-142`).

**Code does**: `enum State { Created, Running, Draining, Done }` (`node.rs:13-18`). Config is applied
inside `new` (`node.rs:71`), so there is no Configured state. `stop()` issues `zyre_stop` only from
`Running` (`node.rs:162-165`), and `Drop` calls `stop()` then `zyre_destroy` (`node.rs:382-387`).

**Exact spec edit**: `data-model.md:26`
- Old: `**Lifecycle**: Created → Configured → Started → Running → Stopped (via drop)`
- New: `**Lifecycle**: Created → Running → Draining → Done (configuration is applied during creation; \`stop()\` or drop leaves \`Running\` — see State Transitions below)`

`data-model.md:30`
- Old: `**Drop behavior**: Calls \`zyre_stop()\` then \`zyre_destroy()\`.`
- New: `**Drop behavior**: Calls \`stop()\` (which issues \`zyre_stop()\` only if the node is \`Running\`), then \`zyre_destroy()\`.`

**Rationale**: Internal inconsistency in the data model. The diagram at `:126-142` was updated in the
2026-07-14 apply and this line was missed. No code impact.

**Action**: [ ] Approve · [ ] Reject · [ ] Modify

---

### Proposal P5: 001-zyre-bindings / contract: gossip endpoint is bound at `create_node`

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · **approved: false**

**Requirement**: `create_node` postconditions and errors.

**Spec says**: "No network activity occurs until `start()` is called" (`contracts/izyre.md:49`);
"`ZyreError::InvalidConfig(reason)` — config validation failed" (`contracts/izyre.md:53`); "so an
invalid config is rejected without allocating a node" (`research.md:56`).

**Code does**: in gossip mode `apply_config` calls `zyre_set_endpoint` during `create_node`, and a
nonzero return becomes `InvalidConfig("failed to bind node endpoint '…'")` (`node.rs:115-121`). The
node's data socket is therefore bound before `start()`, and this `InvalidConfig` is raised after
`zyre_new` (`node.rs:56`). The node is then dropped (`zyre_destroy`, `node.rs:385`) before the error
returns.

**Exact spec edit**: `contracts/izyre.md:49`
- Old: `- No network activity occurs until \`start()\` is called`
- New: `- No discovery or messaging occurs until \`start()\` is called. Exception: in gossip mode the node's data \`endpoint\` is bound during \`create_node\`, so its listening socket exists before \`start()\``

`contracts/izyre.md:53`
- Old: ``- `ZyreError::InvalidConfig(reason)` — config validation failed``
- New: ``- `ZyreError::InvalidConfig(reason)` — config validation failed, or (gossip mode) the node's data `endpoint` could not be bound``

`research.md:56` (last sentence)
- Old: `` `NodeConfig::validate()` runs inside `IZyre::create_node`, before any FFI, so an invalid config is rejected without allocating a node.``
- New: `` `NodeConfig::validate()` runs inside `IZyre::create_node`, before any FFI, so a config that fails validation is rejected without allocating a node. An `endpoint` that cannot be bound (gossip mode) is detected after allocation; the node is destroyed before `create_node` returns `InvalidConfig`.``

**Rationale**: Binding early is the right design, because it lets `create_node` report an unusable
endpoint synchronously. Only the contract wording is too absolute. No code impact.

**Action**: [ ] Approve · [ ] Reject · [ ] Modify

---

### Proposal P6: 001-zyre-bindings / data-model: configured strings are passed verbatim (this branch's fix)

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-005 / `NodeConfig.headers` ("Metadata shared during discovery"); SC-005.

**Spec says**: "| headers | `HashMap<String, String>` | empty | Metadata shared during discovery |"
(`data-model.md:72`). The spec says nothing about how header, endpoint or gossip strings are passed
to zyre.

**Code does**: a constant `"%s"` format (`node.rs:87-90`) is passed to the printf-style
`zyre_set_header` (`:95`), `zyre_set_endpoint` (`:117`), `zyre_gossip_bind` (`:125`) and
`zyre_gossip_connect` (`:129`), with the user string as the argument. A `%` is announced verbatim.
Regression test: `src/repair_test_header_percent.rs:8-51`, which asserts that `"100%%"` arrives
unchanged (`:42-47`).

**Exact spec edit**: `data-model.md:72`
- Old: `| headers | \`HashMap<String, String>\` | empty | Metadata shared during discovery |`
- New: `| headers | \`HashMap<String, String>\` | empty | Metadata shared during discovery; keys and values are announced to peers byte-for-byte |`

`data-model.md:86-87`: insert after the last validation bullet (`- if \`gossip\` is set, ...`):
- Old: *(no text; a blank line follows line 86)*
- New:
  ```
  **Verbatim strings**: header keys/values, `endpoint`, and the gossip `bind`/`connect`
  endpoints are passed to zyre verbatim. Several zyre setters are printf-style, so the
  binding always supplies a constant `"%s"` format with the string as its argument; a `%`
  in configuration data has no special meaning and is never interpreted as a format
  directive.
  ```

**Rationale**: The code is authoritative and tested. Writing the guarantee into the spec stops it
being lost again: before the fix, `%` was mangled and `%s`/`%n` was undefined behaviour. No code
impact.

**Action**: [ ] Approve · [ ] Reject · [ ] Modify

---

### Proposal P7: 001-zyre-bindings / contract: `create_node` is serialised process-wide (this branch's fix)

**Type**: BACKFILL (Code → Spec) · **Confidence**: MEDIUM · **approved: false**

**Requirement**: `create_node` thread safety.

**Spec says**: "- `IZyre` is `Send + Sync`; `create_node` may be called from any thread."
(`contracts/izyre.md:57`)

**Code does**: `static ZYRE_NEW_LOCK: std::sync::Mutex<()>` (`node.rs:137-140`) is acquired in
`ZyreNode::new` (`node.rs:51`), recovering from poisoning. The guard lives until `new` returns, so it
covers `zyre_new` *and* `apply_config` (`node.rs:52-72`). The reason given in the comments is that
CZMQ's first-use global init (`zsys_init`) is unlocked, and concurrent creation could abort the
process.

**Exact spec edit**: `contracts/izyre.md:57`
- Old: `- \`IZyre\` is \`Send + Sync\`; \`create_node\` may be called from any thread.`
- New: `- \`IZyre\` is \`Send + Sync\`; \`create_node\` may be called from any thread. Node creation (\`zyre_new\` plus applying the config) is serialised by a process-wide lock, because CZMQ initialises process-global state on first use without a lock; concurrent \`create_node\` calls therefore run one at a time.`

**Rationale**: The lock is what makes the existing contract sentence true. Documenting it records the
CZMQ constraint and the (small) serialisation cost. Confidence is MEDIUM because some reviewers will
call this an implementation detail. A side note, not part of the proposal: the code comments at
`node.rs:50`/`:137` say the lock covers only `zyre_new`.

**Action**: [ ] Approve · [ ] Reject · [ ] Modify

---

### Proposal P8: 001-zyre-bindings / data-model: `endpoint` is ignored in beacon mode

**Type**: BACKFILL (Code → Spec) · **Confidence**: MEDIUM · **approved: false**

**Requirement**: FR-006 / `NodeConfig.endpoint`.

**Spec says**: "| endpoint | `Option<String>` | None | Node's own data endpoint (required with gossip) |"
(`data-model.md:78`). FR-006 says: "In gossip mode, the node requires an explicit data endpoint ...
set via the config's `endpoint` field" (`spec.md:92`). Beacon mode is not addressed.

**Code does**: `zyre_set_endpoint` is called only inside `if let Some(ref gossip) = config.gossip`
(`node.rs:111-122`). A configured `endpoint` with `gossip: None` is silently ignored, and validation
accepts it (`izyre.rs:334-345` checks only the gossip case).

**Exact spec edit**: `data-model.md:78`
- Old: `| endpoint | \`Option<String>\` | None | Node's own data endpoint (required with gossip) |`
- New: `| endpoint | \`Option<String>\` | None | Node's own data endpoint (required with gossip; only applied in gossip mode — ignored when \`gossip\` is \`None\`) |`

**Rationale**: The current behaviour is deliberate: the comment at `node.rs:112-114` frames the
endpoint as a gossip-mode requirement. It is harmless, but a reader of the spec could expect the
field to take effect. **ALIGN alternative** for the reviewer: make `validate` reject
`endpoint.is_some() && gossip.is_none()` with `InvalidConfig`. That is stricter and removes the
silent no-op, but it is a behaviour change for existing callers.

**Action**: [ ] Approve (backfill) · [ ] Choose ALIGN alternative · [ ] Reject

---

### Proposal P9: 001-zyre-bindings / FR-010: `ZYRE_BUILD_DIR` override and rpath

**Type**: BACKFILL (Code → Spec) · **Confidence**: HIGH · **approved: false**

**Requirement**: FR-010 (with FR-009's `deps/zyre-build/` location).

**Spec says**: "- **FR-010**: The crate MUST generate FFI bindings via `bindgen` in a build script,
linking against the locally-built libraries." (`spec.md:96`)

**Code does**: `build.rs:5-18` uses `$ZYRE_BUILD_DIR` if set, else `<workspace>/deps/zyre-build`, and
reruns on change (`build.rs:39`). It embeds `lib`/`lib64` as DT_RPATH (`--disable-new-dtags`,
`build.rs:26-34`) so the transitive `libczmq` → `libzmq` resolve at runtime without
`LD_LIBRARY_PATH`. If `zyre.h` is missing, it exits with a hint to run `deps/build_zyre.sh`
(`build.rs:45-66`).

**Exact spec edit**: `spec.md:96`
- Old: `- **FR-010**: The crate MUST generate FFI bindings via \`bindgen\` in a build script, linking against the locally-built libraries.`
- New: `- **FR-010**: The crate MUST generate FFI bindings via \`bindgen\` in a build script, linking against the locally-built libraries (default \`deps/zyre-build/\`, overridable with the \`ZYRE_BUILD_DIR\` environment variable) and embedding their library directories as an rpath (DT_RPATH) so the transitive \`libczmq\`/\`libzmq\` resolve at runtime without \`LD_LIBRARY_PATH\`.`

**Rationale**: These are working, intentional build features (CI relies on a pre-built stack that is
symlinked or pointed to). Without them in the spec, a reader of FR-009/FR-010 cannot tell how CI
builds zyre. No code impact.

**Action**: [ ] Approve · [ ] Reject · [ ] Modify

---

## Not proposed (recorded in drift-report, need verification or are not spec drift)

- **L1**: `zyre_set_interface` may be process-global in zyre v2.0.1 (`zsys_set_interface`). It is
  unverified here, so no proposal is made until it is checked against the pinned source.
- The edge case "nonexistent interface → start error" (`spec.md:80`) and US1 AS4 (EXIT after drop,
  `spec.md:23`) have no tests. Recommended tests are listed in the drift report.
- `tests/api_safety.rs:20-26` does not actually assert `!Sync`. This is a test gap; FR-011 holds by
  construction.
- `src/repair_test_header_percent.rs` does real network I/O but is not gated by the
  `integration-tests` feature or by `SERIAL_LOCK`. This is test hygiene.
