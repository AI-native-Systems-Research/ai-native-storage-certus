# Spec-Sync Phase B — Proposals: disk-partition-manager

**Generated**: 2026-08-20 (Phase B)
**Based on drift report**: `drift-report.json` (generated 2026-08-07T15:30:30Z)
**Policy**: `.specify/sync/PHASE_B_POLICY.md`
**Spec**: `001-gpt-partition-management`

> **Supersedes the 2026-08-07 sync run.** That run classified FR-003 as an ALIGN
> item and drafted a code fix on branch `sync/spec-drift-sweep-20260807`. Reading
> the current `src/gpt.rs:66-96` shows that fix is now present in the code, so
> under Phase B policy FR-003 is a "described-as-drafted item that is actually
> implemented" → **BACKFILL**. No ALIGN tasks remain for this component.

## Classification summary

| # | Requirement | Direction | Severity | Applied |
|---|-------------|-----------|----------|---------|
| P1 | FR-003 | BACKFILL | Medium | yes (edited) |
| P2 | PR-002 | BACKFILL | Medium | yes (already reflected) |
| P3 | Unspecced — hardcoded entry LBA 2 on read | BACKFILL-UNSPECCED | Low | yes (already in Impl. Notes) |
| P4 | Unspecced — `generate_guid` zero-fallback | BACKFILL-UNSPECCED | Low | yes (already in Impl. Notes) |

- BACKFILL: 2 (FR-003, PR-002)
- BACKFILL-UNSPECCED: 2
- ALIGN: 0
- RESOLVED: 0
- HUMAN_DECISION: 0

---

## P1 — FR-003 · BACKFILL

**Requirement**: FR-003 (primary/backup GPT redundancy).

**Direction**: BACKFILL (was ALIGN in the 2026-08-07 sweep).

**Rationale**: The drift report was generated against the pre-fix code snapshot and
described `read_gpt` as falling through to the backup only on `CorruptTable` (CRC),
propagating a bad primary *signature* as `NoPartitionTable` and allowing
`initialize_or_format` to destructively reformat a disk with a valid backup. The
**current** `src/gpt.rs:66-96` shows the fix is present: the primary-read arm matches
**both** `Err(CorruptTable(_))` **and** `Err(NoPartitionTable(_))` and falls through to
the backup-header attempt; a genuinely unformatted disk still returns `NoPartitionTable`
after both reads fail. The code is the working, intended reality; the spec merely lagged
(it still said the fix was "drafted on branch" and referenced an ALIGN task). Per Phase B
policy direction 1, a described-as-drafted item that is actually implemented → BACKFILL.

**Before** (spec text):
> System MUST support primary/backup GPT redundancy — if the primary header is corrupt,
> the backup header at the last LBA MUST be attempted. "Corrupt" includes **both** a
> CRC mismatch **and** a damaged/zeroed primary signature. *(Sync 2026-08-07: … A fix
> routing signature failures through the backup path is drafted on branch
> `sync/spec-drift-sweep-20260807` — see `.specify/sync/align-tasks.md`.)*

**After** (spec text):
> System MUST support primary/backup GPT redundancy — if the primary header is corrupt,
> the backup header at the last LBA MUST be attempted. "Corrupt" includes **both** a
> CRC mismatch **and** a damaged/zeroed primary signature. *(Sync 2026-08-20 —
> implemented and backfilled to reality: `read_gpt` (`src/gpt.rs:66-96`) falls through to
> the backup-header attempt on **both** `CorruptTable` (CRC) and `NoPartitionTable`
> (signature damage). A genuinely unformatted disk still returns `NoPartitionTable` after
> both reads fail, so the reformat path for truly-blank disks is preserved; only the
> damaged-primary/valid-backup case is affected, and it now recovers. Supersedes the
> 2026-08-07 ALIGN task; the fix is present, so FR-003 is resolved by spec backfill.)*

**Acceptance scenario added** — US2 scenario 4:
> **Given** a drive whose primary GPT header *signature* is damaged or zeroed (torn write)
> but whose backup GPT is valid, **When** `initialize()` is called, **Then** the backup GPT
> is used, the partition table is returned successfully, and the disk is NOT reformatted.

---

## P2 — PR-002 · BACKFILL

**Requirement**: PR-002 (read I/O round-trips).

**Direction**: BACKFILL.

**Rationale**: `read_bytes` (`src/gpt.rs:451-495`) loops one synchronous
`Command::ReadSync` + completion `recv` per sector, so the 128×128 B = 16 KiB entry
array is 32 round-trips at 512 B (4 at 4096 B) plus 1 header sector — not the "at most 2"
originally stated. This is the intended, working behavior and is O(1) in device size
(same intent as PR-001); the "≤2" was an over-specification. Spec-lag → BACKFILL.
Batching the entry-array read into a single multi-sector I/O is a deliberate future perf
task, not tracked as drift. The current `spec.md` PR-002 text already reflects this
reality (backfilled 2026-08-07); no further edit required this run.

**Before**: "Initialize (read) operation MUST require at most 2 I/O round-trips (header +
entries) for the happy path."

**After**: "Initialize (read) operation reads a fixed, device-size-independent set of
sectors … `read_bytes` issues one synchronous `Command::ReadSync` per sector … i.e. 33
(5) round-trips total … This is O(1) in device size, satisfying the same intent as
PR-001. Batching … is a deliberate optimization left for a future perf task, not tracked
as drift."

---

## P3 — Unspecced: hardcoded primary entry LBA on read · BACKFILL-UNSPECCED

**Location**: `src/gpt.rs:68` (`try_read_gpt_at(1, 2)`), ignoring parsed
`header.partition_entry_lba` (`gpt.rs:370`).

**Direction**: BACKFILL-UNSPECCED (documented in Implementation Notes, not promoted to an
FR).

**Rationale**: The read path reads the primary entry array from a hardcoded LBA 2 rather
than honoring the parsed `partition_entry_lba`. Every Certus-written GPT uses LBA 2, so
round-tripping our own tables is correct; a GPT written by an external tool with a
different entry offset would be read incorrectly. This is a scope limitation (not a
working feature warranting a MUST-level requirement), so it is captured in Implementation
Notes and US2 is scoped in practice to Certus-written/standard-layout tables. Honoring
`partition_entry_lba` is a future robustness change.

**After**: documented in Implementation Notes ("Read path assumes Certus-written
layout …"). Already present in current `spec.md`.

---

## P4 — Unspecced: `generate_guid` zero-fallback · BACKFILL-UNSPECCED

**Location**: `src/gpt.rs:564-574` (`generate_guid`).

**Direction**: BACKFILL-UNSPECCED (documented in Implementation Notes).

**Rationale**: `generate_guid` opens `/dev/urandom` and ignores read errors; if the file
cannot be opened the GUID stays all-zero except the version/variant bits — non-random and
collision-prone, contradicting FR-008's unconditional "random GUIDs". This is a
known-behavior/latent-defect caveat rather than a desirable feature, so it is documented
in Implementation Notes rather than promoted to an FR. Erroring out on `/dev/urandom`
failure would be a future code change.

**After**: documented in Implementation Notes. Already present in current `spec.md`.
