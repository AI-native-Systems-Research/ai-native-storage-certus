# Align Tasks: zyre

Generated: 2026-10-14T18:23:53Z (spec-sync apply, branch `fix/zyre-printf-format`, proposals from
2026-10-14T17:38:10Z at `85ae2a1f`)

These tasks come from the approved ALIGN proposals (spec → code). Both approvals read:

> "approved as an ALIGN task only: record in align-tasks.md, no code change in this PR"

So this apply changed no code. Each task belongs in a follow-up PR. Both drifts are older than
this branch (the same lines exist on `origin/unstable`).

There are no open items from unapproved proposals: every proposal in `proposals.json` is
`approved: true`.

---

## Task: Align 001-zyre-bindings/FR-012 (P1)

**Spec Requirement**: FR-012 (`spec.md`: "The crate MUST provide typed errors via a `ZyreError` enum
covering all failure modes"), US2 AS2 (every fallible operation returns `Result<T, ZyreError>`), the
`create_node` error contract (`contracts/izyre.md`: only `InvalidConfig` / `CreateFailed`), and
`research.md` R5 (validation runs before any FFI).

**Current Code**: `apply_config` calls `CString::new(..).unwrap()` on header keys and values, `interface`,
`endpoint`, gossip `bind` and gossip `connect` (`components/zyre/src/node.rs:93,94,103,116,124,128`).
An interior NUL byte in any of them panics inside `create_node`, after `zyre_new`. `NodeConfig::validate`
(`components/interfaces/src/izyre.rs:313-347`) does not check for NUL. `name`, `group` and `peer` already
map a NUL to `InvalidConfig` (`node.rs:54-55,171-172,205-206`).

**Required Change**:
1. `NodeConfig::validate`: return `ZyreError::InvalidConfig("<field> contains a NUL byte")` if any header
   key or value, `interface`, `endpoint`, `gossip.bind` or any `gossip.connect` entry contains `'\0'`.
   The `name` check may move there too.
2. `apply_config`: replace each `.unwrap()` with `.map_err(|_| ZyreError::InvalidConfig(..))?` (defence in
   depth), or use `.expect("validated: no NUL")` with a comment that points at `validate`.
3. In the same change, apply the accompanying spec text to `specs/001-zyre-bindings/data-model.md`
   (Validation list). Add after the gossip bullet:
   `- no string field (\`name\`, header keys/values, \`interface\`, \`endpoint\`, gossip \`bind\`/\`connect\`) may contain an interior NUL byte`

**Files to Modify**: `components/interfaces/src/izyre.rs`, `components/zyre/src/node.rs`,
`components/zyre/specs/001-zyre-bindings/data-model.md`

**Estimated Effort**: small

### Acceptance Criteria
- [ ] `NodeConfig::validate` rejects a NUL in each listed field with `InvalidConfig` (one unit test per field)
- [ ] `ZyreNode::new` / `IZyre::create_node` returns `InvalidConfig` and does not panic for such a config
- [ ] No `CString::new(..).unwrap()` remains on configuration input in `node.rs`
- [ ] `data-model.md` Validation list states the no-NUL rule

---

## Task: Align 001-zyre-bindings/FR-005 (P2)

**Spec Requirement**: FR-005 (`spec.md`: a typed `NodeConfig` "validated when a node is created"); the
Validation rules in `data-model.md` (evasive > 0, expired > evasive).

**Current Code**: `evasive_timeout_ms` / `expired_timeout_ms` are `u32` (`components/interfaces/src/izyre.rs:279,281`).
`validate` checks only `> 0` and `expired > evasive` (`izyre.rs:319-328`). `apply_config` casts both with
`as libc::c_int` (`components/zyre/src/node.rs:107-108`). As a result, a value above `i32::MAX`
(e.g. `3_000_000_000`) passes validation and reaches zyre as a negative timeout.

**Required Change**:
1. `NodeConfig::validate`: return `InvalidConfig` when `evasive_timeout_ms > i32::MAX as u32` or
   `expired_timeout_ms > i32::MAX as u32`.
2. Optional: use `c_int::try_from(..)` at `node.rs:107-108` in place of `as`.
3. In the same change, apply the accompanying spec text to `data-model.md` (Validation list):
   - `- \`evasive_timeout_ms\` must be > 0 and <= 2147483647 (\`i32::MAX\`, zyre takes a C \`int\`)`
   - `- \`expired_timeout_ms\` must be > \`evasive_timeout_ms\` and <= 2147483647`

   (Rejected alternative, noted in the proposal: narrow the fields to `i32`. That is an interface change.)

**Files to Modify**: `components/interfaces/src/izyre.rs`, `components/zyre/src/node.rs`,
`components/zyre/specs/001-zyre-bindings/data-model.md`

**Estimated Effort**: small

### Acceptance Criteria
- [ ] `validate` rejects `evasive_timeout_ms = i32::MAX as u32 + 1` and `expired_timeout_ms = i32::MAX as u32 + 1` with `InvalidConfig`
- [ ] `validate` still accepts `i32::MAX` for `expired_timeout_ms` (boundary)
- [ ] No `as libc::c_int` cast can produce a negative timeout
- [ ] `data-model.md` Validation list states the upper bound
