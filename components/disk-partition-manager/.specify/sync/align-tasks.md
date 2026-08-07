# Align Tasks — disk-partition-manager

Generated: 2026-08-07 (branch `sync/spec-drift-sweep-20260807`)
Source: `drift-report.{json,md}` (generated 2026-08-07T15:30:30Z)

These are spec→code (ALIGN) items. Per the user's decision for HIGH code
defects this sweep, the fix is **drafted on the feature branch** (not committed
to `unstable`) for review.

---

## Task 1: FR-003 — attempt backup GPT on primary *signature* corruption  [HIGH]

**Spec Requirement**: FR-003 / US2 scenario 2 — "if the primary header is
corrupt, the backup header at the last LBA MUST be attempted."

**Problem (before)**: `GptManager::read_gpt` (`src/gpt.rs`) matched only the
`CorruptTable` (CRC-mismatch) error to fall through to the backup. A damaged or
zeroed primary header *signature* is reported by `parse_header` as
`NoPartitionTable`, which was propagated immediately — the backup was never
tried. Because `initialize_or_format` (`src/lib.rs`) maps `NoPartitionTable` to
"unformatted → format", a disk with a torn/partial-write-damaged primary but a
perfectly valid backup GPT would be **destructively reformatted** instead of
recovered. This is a data-loss defect, hence treated as HIGH despite the
drift-report's "Medium" label.

**Change (drafted on branch)** — `src/gpt.rs`, `read_gpt`:
Both `CorruptTable(_)` and `NoPartitionTable(_)` from the primary read now fall
through to the backup-header attempt. A genuinely unformatted disk still ends up
returning `NoPartitionTable` (now "neither primary nor backup GPT header is
valid") after both reads fail, so `initialize_or_format`'s reformat path for
truly-blank disks is preserved; only the damaged-primary/valid-backup case
changes — it now recovers.

**Files Modified**:
- `components/disk-partition-manager/src/gpt.rs`

**Estimated Effort**: small.

### Acceptance Criteria
- [x] `cargo build -p disk-partition-manager` — clean.
- [ ] **REMAINING (test gap)**: the component has NO unit tests (drift report /
      `tasks.md` confirm). Add a test that writes a valid GPT, corrupts the
      primary header *signature* bytes (not just the CRC), and asserts
      `read_gpt`/`initialize` recovers from the backup and does NOT reformat.
      This should live alongside the round-trip/backup-fallback tests already
      enumerated in `tasks.md`.
- [ ] **REVIEW**: confirm the extra backup read on a genuinely-unformatted disk
      (one additional sector read before formatting) is acceptable overhead.

---

## Informational — NOT drafted as code this sweep (documented in spec instead)

Per the user's decision, the following were resolved by **backfilling the
spec** (see `apply-report.md`), not by code changes:

- **PR-002** (read does ~33 round-trips vs the stated "at most 2"): backfilled
  to describe the actual per-sector read. Batching the entry-array read into a
  single multi-sector I/O remains an optional future perf task.
- **Read path hardcodes LBA 2** instead of honoring `header.partition_entry_lba`
  (misreads externally-created GPTs): documented in Implementation Notes.
  Honoring `partition_entry_lba` is a future robustness change.
- **`generate_guid` zero-fallback** on `/dev/urandom` failure: documented in
  Implementation Notes. Erroring out instead of zero-filling is a future change.
- **Sector-size validation** (FR-011 accepts any size): documented. Explicit
  `{512, 4096}` validation is a future decision.
