# Align Tasks — disk-partition-manager

**Phase B run**: 2026-08-20
**Source**: `drift-report.{json,md}` (generated 2026-08-07T15:30:30Z)
**Policy**: `.specify/sync/PHASE_B_POLICY.md`

## No ALIGN tasks

This Phase B run produced **zero ALIGN tasks**. Reading the referenced source for
each drifted requirement showed no correct-spec-vs-buggy-code case remaining:

- **FR-003** — the 2026-08-07 sweep recorded this as an ALIGN item (backup-fallback on
  primary *signature* corruption) and drafted a code fix on branch
  `sync/spec-drift-sweep-20260807`. That fix is now present in `src/gpt.rs:66-96`:
  `read_gpt` falls through to the backup-header attempt on **both** `CorruptTable(_)`
  and `NoPartitionTable(_)`, and a genuinely unformatted disk still returns
  `NoPartitionTable` after both reads fail. Because the code now implements the required
  behavior, FR-003 is resolved by **BACKFILL** (spec text updated to describe the
  implemented behavior) rather than an ALIGN task. See `proposals.md` P1.
- **PR-002** — intended O(1)-in-device-size per-sector read behavior; resolved by
  **BACKFILL** (already reflected in `spec.md`). See `proposals.md` P2.
- Both unspecced items (hardcoded entry LBA 2 on read; `generate_guid` zero-fallback)
  are low-severity caveats resolved by **BACKFILL-UNSPECCED** (Implementation Notes).

---

### Historical note (superseded)

The prior (2026-08-07) `align-tasks.md` carried one HIGH task —
"FR-003 — attempt backup GPT on primary *signature* corruption" — with a code fix drafted
on the feature branch and a remaining test-gap item:

> **REMAINING (test gap)**: the component has NO unit tests. Add a test that writes a
> valid GPT, corrupts the primary header *signature* bytes (not just the CRC), and
> asserts `read_gpt`/`initialize` recovers from the backup and does NOT reformat.

That code fix has since landed in `src/gpt.rs`, so the ALIGN task itself is closed
(reclassified to BACKFILL). The **test gap** it identified is not tracked here as an
ALIGN item (no source/spec-requirement violation remains), but it persists as a known
coverage gap: SC-001/SC-002/SC-003 and this signature-recovery scenario have no automated
tests (see `drift-report.json` `conflicts` and `tasks.md`). It should be picked up as a
normal test-authoring task, not a spec-sync ALIGN.
