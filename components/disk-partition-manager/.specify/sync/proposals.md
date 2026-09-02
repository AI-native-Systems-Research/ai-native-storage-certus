# Spec-Sync — Proposals: disk-partition-manager

**Generated**: 2026-09-02 (re-verification run)
**Based on drift report**: `drift-report.json` (generated 2026-09-02T21:27:23Z)
**Spec**: `001-gpt-partition-management`
**Spec location**: `components/disk-partition-manager/.specify/specs/001-gpt-partition-management/spec.md` (under-component quirk — see drift report).

> **Outcome**: The 2026-08-20 Phase B run already backfilled the spec to code
> reality (FR-003 signature fallback, PR-002 per-sector reads, both unspecced items
> in Implementation Notes). This run re-verified all 20 requirements against the
> current source and found **no actionable drift**. The only proposal is a metadata
> provenance refresh of the spec's "Last Synced" line. No requirement-level BACKFILL,
> no ALIGN, no HUMAN_DECISION.

## Classification summary

| # | Requirement | Direction | Severity | Approved | Applied |
|---|-------------|-----------|----------|----------|---------|
| P1 | Spec metadata ("Last Synced") | BACKFILL (metadata) | Info | yes | yes (edited) |
| P2 | FR-001..FR-011, IR-001..003, PR-001..002, SC-001..004 | RESOLVED (already aligned) | — | yes | n/a (no edit needed) |
| P3 | Unspecced — hardcoded entry LBA 2 on read (gpt.rs:68) | BACKFILL-UNSPECCED | Low | yes | already reflected in Impl. Notes |
| P4 | Unspecced — `generate_guid` zero-fallback (gpt.rs:564-569) | BACKFILL-UNSPECCED | Low | yes | already reflected in Impl. Notes |

- BACKFILL (metadata): 1
- RESOLVED (already aligned, no action): 20 requirements
- BACKFILL-UNSPECCED: 2 (already present)
- ALIGN: 0
- HUMAN_DECISION: 0

---

## P1 — Spec metadata refresh · BACKFILL (metadata) · approved

**Direction**: BACKFILL (metadata only).

**Rationale**: The spec's "Last Synced" line was dated 2026-08-20. This run
re-verified every FR/IR/PR/SC against the current `src/gpt.rs` + `src/lib.rs` on
commit `2fc1cd3c` and confirmed full alignment. Updating the provenance line keeps
the spec honest about when it was last checked and records the "no actionable drift"
result plus the residual test-coverage gap.

**Change**: Prepend a 2026-09-02 re-verification note to "Last Synced" while
preserving the prior 2026-08-20 note verbatim. No requirement text changed.

---

## P2 — All 20 requirements · RESOLVED (already aligned) · approved

**Direction**: No action. Each requirement is aligned with the implementation with
file:line evidence recorded in `drift-report.md`. FR-003's signature-fallback fix is
confirmed present at `src/gpt.rs:79-80` (matches both `CorruptTable(_)` and
`NoPartitionTable(_)`, falls through to backup at 86-95). PR-002's spec text already
describes the actual per-sector read behavior. No spec edit or code change required.

---

## P3 — Unspecced: hardcoded primary entry LBA on read · BACKFILL-UNSPECCED · approved

**Location**: `src/gpt.rs:68` (`try_read_gpt_at(1, 2)`), ignoring parsed
`header.partition_entry_lba` (`gpt.rs:380`).

**Direction**: BACKFILL-UNSPECCED — remains documented in Implementation Notes; not
promoted to an FR (scope limitation, not a working feature warranting a MUST). Already
present in the current `spec.md`. No edit required this run.

---

## P4 — Unspecced: `generate_guid` zero-fallback · BACKFILL-UNSPECCED · approved

**Location**: `src/gpt.rs:564-569` (`generate_guid`).

**Direction**: BACKFILL-UNSPECCED — remains documented in Implementation Notes as a
known behavior/latent caveat (in tension with FR-008's unconditional "random"), not
promoted to an FR. Already present in the current `spec.md`. No edit required this run.
Fixing it (error on `/dev/urandom` failure) would be a future code change, tracked as a
normal task rather than spec-sync drift.
