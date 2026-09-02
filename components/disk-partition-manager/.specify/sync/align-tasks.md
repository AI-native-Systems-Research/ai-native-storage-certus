# Align Tasks — disk-partition-manager

**Latest run**: 2026-09-02 (re-verification)
**Source**: `drift-report.{json,md}` (generated 2026-09-02T21:27:23Z)

## No ALIGN tasks

This run produced **zero ALIGN tasks**. Every drifted requirement from earlier runs
has been resolved, and re-reading the current source confirms no
correct-spec-vs-buggy-code case remains:

- **FR-003** — backup-fallback on primary *signature* corruption is implemented in
  `src/gpt.rs:66-96`: the primary-read arm matches **both** `CorruptTable(_)` and
  `NoPartitionTable(_)` (gpt.rs:79-80) and falls through to the backup-header attempt
  (gpt.rs:86-95); a genuinely unformatted disk still returns `NoPartitionTable` after
  both reads fail. Resolved by BACKFILL (2026-08-20); confirmed still present.
- **PR-002** — intended O(1)-in-device-size per-sector read; spec text already reflects
  reality. Resolved by BACKFILL.
- Both unspecced items (hardcoded entry LBA 2 on read; `generate_guid` zero-fallback)
  remain low-severity caveats documented in spec Implementation Notes (BACKFILL-UNSPECCED).

## Standing (non-ALIGN) note: test-coverage gap

The component still has **no unit tests** (`[dev-dependencies]` empty, Cargo.toml:15).
SC-001/SC-002/SC-003 and the FR-003 signature-recovery scenario are functionally aligned
but have zero automated verification. This is a normal test-authoring task (see
`tasks.md` "Add Unit Tests"), **not** a spec-sync ALIGN item, because no
source-vs-spec-requirement violation exists. Recommended tests:

- format → initialize round-trip (offsets identical).
- Corrupt primary header **CRC** → backup fallback.
- Corrupt primary header **signature** (zeroed bytes) → backup fallback, no reformat.
- Layout errors: fixed partitions exceed capacity; >1 `size_bytes=0`.
- UTF-16LE name encode/decode (ASCII, max length, empty).
- Protective MBR written correctly (type 0xEE, boot signature).

---

### Historical note (superseded)

The 2026-08-07 sweep carried one HIGH ALIGN task — "FR-003 — attempt backup GPT on
primary *signature* corruption" — with a code fix drafted on branch
`sync/spec-drift-sweep-20260807`. That fix landed in `src/gpt.rs`, so the ALIGN task was
closed and reclassified to BACKFILL (2026-08-20). No ALIGN tasks have been generated since.
