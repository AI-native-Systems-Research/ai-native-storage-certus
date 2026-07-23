# Spec Sync — Align Tasks
Project: extent-manager
Source drift cycle: 2026-07-22T22:37:58Z (`drift-report.json` / `drift-report.md`)

Items below were deferred during AUTO-BACKFILL apply (2026-07-22) because resolving them
requires either a code change (outside the scope of a Markdown-only spec-sync apply) or a
decision beyond what is verifiable from `src/` and existing specs alone.

---

## Task: Align 001-extent-manager-v2/FR-030

**Severity**: Major (DEFECT)

**Spec Requirement**: FR-030 states: "The component supports a `volatile_write_cache`
feature gate that controls whether `BlockDeviceClient::flush()` calls are issued to the
metadata device. When enabled, flush calls in checkpoint and format paths are
conditionally compiled out, improving performance on devices with volatile write caches
where the caller accepts the associated durability risk." I.e., the *intended* design is:
default (feature disabled) = durable/flushing behavior; opt-in `volatile_write_cache`
feature = flush calls compiled out for performance, accepting durability risk.

**Current Code**: The implementation has the cfg-polarity inverted relative to the spec's
intent. `#[cfg(feature = "volatile_write_cache")]` guards in
`components/extent-manager/src/checkpoint.rs:99-103` and
`components/extent-manager/src/lib.rs:308-310` cause the `flush()` call to be compiled
**IN** only when the feature is **enabled**, and compiled out (absent entirely) when the
feature is **disabled (the default)**. This is the opposite of what FR-030 describes: today,
the default build never flushes the metadata device after a checkpoint write, and the
"risky" opt-in feature is actually what restores the flush/durability behavior. Separately,
the spec's claim that flushes are also conditionally compiled out of the `format()` path is
inaccurate under either polarity — `format()` (`src/lib.rs:383-512`) contains no `flush()`
call at all, feature-gated or otherwise; only the checkpoint path is affected.

**Required Change**: This is a correctness/durability question, not a wording nit, and must
not be resolved by silently editing the spec to match the buggy code. A maintainer must
decide and then implement one of:
1. Fix the code so that flush() is issued by default (feature disabled) and is skipped only
   when `volatile_write_cache` is explicitly enabled — matching FR-030's documented intent
   (recommended, since the current default silently sacrifices metadata durability).
2. Or, if the current code's polarity is actually the intended behavior (e.g. because the
   crate's dependents always run with a real NVMe write-cache flush already handled at a
   lower layer), rewrite FR-030 to accurately describe that opt-in-to-flush semantics — but
   this changes the durability contract for every caller and needs explicit sign-off from
   crate consumers (`dispatcher`, `dispatcher-p2p`, `dispatch-map`).
Either way, also correct FR-030 (or a new FR) to state plainly that no flush call exists in
the `format()` path under any feature configuration, if that remains true after the fix.

**Files to Modify**: `components/extent-manager/src/checkpoint.rs`,
`components/extent-manager/src/lib.rs` (code fix, option 1); or
`components/extent-manager/specs/001-extent-manager-v2/spec.md` (FR-030 rewrite, option 2) —
pending maintainer decision. Not modified by this apply pass.

---

## Task: Align 001-extent-manager-v2/FR-016 (README default-interval doc string)

**Severity**: Low (ALIGN — documentation drift outside spec-sync edit scope)

**Spec Requirement**: FR-016 (already updated by this apply pass) discloses that both the
`IExtentManager` interface doc comment and this component's `README.md` incorrectly state
a "five minutes" default checkpoint interval, when the actual default is 30 seconds
(`ExtentManager::new_inner`, `src/lib.rs:109-112`).

**Current Code**: `components/extent-manager/README.md:12` — "Background periodic
checkpoint thread (configurable interval, default 5 minutes)" — is stale.
`components/interfaces/src/iextent_manager.rs:244` doc comment on
`set_checkpoint_interval` similarly says "The default is five minutes."

**Required Change**: Update `README.md`'s feature bullet to say "default 30 seconds," and
update the `set_checkpoint_interval` doc comment in `iextent_manager.rs` to say 30 seconds
(or remove the specific number and point readers to `ExtentManager::new_inner()`). Both are
non-Markdown-spec or out-of-component files, so they are outside the edit scope of this
spec-sync apply pass (which is restricted to `components/extent-manager/specs/**` and
`.specify/sync/**`).

**Files to Modify**: `components/extent-manager/README.md`,
`components/interfaces/src/iextent_manager.rs` (doc comment only).

---

## Task: Align 001-extent-manager-v2/Key Entities (Superblock) — README format-version doc string

**Severity**: Low (ALIGN — documentation drift outside spec-sync edit scope)

**Spec Requirement**: The spec's Key Entities section and On-Disk Format Reference
correctly state the superblock format version is `6` (`FORMAT_VERSION: u32 = 6`,
`src/superblock.rs:6`), matching the code.

**Current Code**: `components/extent-manager/README.md:19` still describes the on-disk
format as "format version (5)" — stale from an earlier format revision, and inconsistent
with both the current spec and the current code.

**Required Change**: Update `README.md`'s on-disk format description to say "format
version (6)". Out of scope for this spec-sync apply pass (README.md is not under
`components/extent-manager/specs/**` or `.specify/sync/**`).

**Files to Modify**: `components/extent-manager/README.md`.

---
