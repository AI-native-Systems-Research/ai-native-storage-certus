---
spec_sync_component: disk-partition-manager
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-02T21:27:23Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 2f235d5f0e88280835cd599eb58872851a775f79f4c6d9a65a8f8c0554cd6d1a
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

Generated: 2026-09-02T21:27:23Z (re-verification run; supersedes 2026-08-07/2026-08-20)

# Drift Report: disk-partition-manager

Spec: `.specify/specs/001-gpt-partition-management/spec.md` (Status: Backfilled)
Implementation: `src/lib.rs`, `src/gpt.rs`
Interface: `components/interfaces/src/ipartition_table.rs`

> **Spec-location quirk**: for this component the spec-kit tree lives **under the
> component** at `components/disk-partition-manager/.specify/specs/001-gpt-partition-management/`,
> not at a repo-level or component-level `specs/` directory. Consequently
> `scripts/spec-sync-hash.sh` (which walks only `<dir>/src` + `components/interfaces/{src,specs}`,
> since there is no `<dir>/specs`) hashes source + interfaces only — the spec `.md`
> files are intentionally **not** part of the digest for this component. This is
> expected; the digest below is whatever that tool produced.

## Summary

| Status          | Count |
|-----------------|-------|
| Aligned         | 20    |
| Drifted         | 0     |
| Not Implemented | 0     |
| Unspecced       | 2 (both already documented in spec Implementation Notes) |

Total requirements analyzed: 20 (11 FR + 3 IR + 2 PR + 4 SC).

**Conclusion**: The spec was fully backfilled to code reality by the 2026-08-20 run.
This re-verification confirms every requirement is still aligned with the current
implementation. No actionable drift remains → `drift_status: clean`. The only open
item is a test-coverage gap (SC-001/002/003 + the FR-003 signature-recovery scenario
have no automated tests), which is a normal test-authoring task, not spec drift.

## Per-Requirement Table

| ID     | Status   | Location | Notes |
|--------|----------|----------|-------|
| FR-001 | Aligned  | gpt.rs:9,11,12,180,186,187,207,216,329-354 | Rev 1.0 (0x00010000), 128×128B entries, primary header my_lba=1 written at LBA 1, backup at last LBA, protective MBR at LBA 0. |
| FR-002 | Aligned  | gpt.rs:107-129,171,426-427 | CRC32 (crc32fast = IEEE) on header and entry array, both read and write paths. |
| FR-003 | Aligned  | gpt.rs:66-96 (arms at 79-80), 357-368 | Backup fallback fires on **both** `CorruptTable` (CRC mismatch) and `NoPartitionTable` (signature damage from `parse_header`). Genuinely-blank disk still returns `NoPartitionTable` after both reads fail, preserving `initialize_or_format`'s reformat path. Fix confirmed present. |
| FR-004 | Aligned  | gpt.rs:250-255,277,280-284 | `size_bytes=0` → rest-of-disk absorbs remaining usable sectors. |
| FR-005 | Aligned  | gpt.rs:251-260 | `rest_count > 1` rejected with `LayoutError`. |
| FR-006 | Aligned  | gpt.rs:263-275 | Fixed partitions exceeding total usable rejected with `LayoutError`; per-partition remaining check at 286-291. |
| FR-007 | Aligned  | gpt.rs:329-354 (0xEE at 339) | Protective MBR type 0xEE at LBA 0, boot signature 0x55AA. |
| FR-008 | Aligned  | gpt.rs:564-574 (v4 at 571, variant at 572) | RFC 4122 v4 GUIDs. Caveat: `/dev/urandom`-failure zero-fallback documented in Implementation Notes (see Unspecced). |
| FR-009 | Aligned  | gpt.rs:576-587 (encode), 589-595 (decode) | UTF-16LE encode/decode capped at 36 chars. |
| FR-010 | Aligned  | lib.rs:30-36,91 | `set_ns_id()`/`get_ns_id()` (default 1) for initialize path; `config.ns_id` for format path. |
| FR-011 | Aligned  | gpt.rs:324-327,451-495,501-553 | Sector size fully parameterized (`self.sector_size`), no 512 hardcoding. No explicit `∈{512,4096}` validation — documented in Implementation Notes. |
| IR-001 | Aligned  | ipartition_table.rs:116-128; lib.rs:68,86,97,113 | `initialize`, `format`, `partition_info`, `num_partitions` all provided. |
| IR-002 | Aligned  | lib.rs:17-19 | `block_device: IBlockDevice` receptacle. |
| IR-003 | Aligned  | lib.rs:45-64 | `initialize_or_format()` returns `(PartitionTable, formatted:bool)`. |
| PR-001 | Aligned  | gpt.rs:204-216 | Format writes a fixed set of sectors (MBR + 2 headers + 2× entry arrays) independent of device size → O(1) vs device size. |
| PR-002 | Aligned  | gpt.rs:451-495,68 | Spec backfilled to reality: `read_bytes` issues one `ReadSync`/`recv` per sector; 16 KiB entry array = 32 round-trips @512B (4 @4096B) + 1 header. O(1) in device size, matching PR-001's intent. Spec text matches. |
| SC-001 | Aligned (no test) | gpt.rs:98-150 (read), 152-237 (write) | Write/read use identical LBA↔sector math; round-trip offsets identical. No automated test. |
| SC-002 | Aligned (no test) | gpt.rs:66-96 | CRC-corrupt AND signature-corrupt primary both transparently fall back to backup. No automated test. |
| SC-003 | Aligned (no test) | gpt.rs:576-595 | ASCII names round-trip UTF-8→UTF-16LE→UTF-8. No automated test. |
| SC-004 | Aligned  | dispatcher/src/lib.rs:1611; dispatcher-p2p/src/lib.rs:1025,1147 | Dispatchers wire `config.format_on_init` → `initialize_or_format(force_format, ...)`; `format_on_init=false` path does not reformat a non-Certus layout (dispatcher/src/lib.rs:1631,1764). |

## Detailed Findings

No drifted or not-implemented requirements this run. Every FR/IR/PR/SC was checked
against the current `src/gpt.rs` and `src/lib.rs` with the file:line evidence in the
table above. The FR-003 signature-fallback fix (the sole HIGH item from the original
2026-08-07 sweep) is confirmed present at `gpt.rs:79-80`, where the primary-read arm
matches both `CorruptTable(_)` and `NoPartitionTable(_)` and falls through to the
backup-header attempt at `gpt.rs:86-95`.

## Unspecced Code

| Item | Location | Notes |
|------|----------|-------|
| Hardcoded primary entry LBA on read | gpt.rs:68 | `try_read_gpt_at(1, 2)` reads the primary entry array from a hardcoded LBA 2 instead of honoring the parsed `header.partition_entry_lba` (parsed at gpt.rs:380). Correct for every Certus-written GPT (always LBA 2); an externally-written GPT with a different entry offset would be read incorrectly. **Already documented** in spec Implementation Notes. Low severity. |
| `generate_guid` silent zero-fallback | gpt.rs:564-569 | If `/dev/urandom` cannot be opened, the GUID stays all-zero except version/variant bits — non-random and collision-prone, in tension with FR-008's unconditional "random". **Already documented** in spec Implementation Notes as a known behavior rather than an FR-level guarantee. Low severity; erroring out would be a future code change. |

## Referenced-but-Nonexistent Files / Proofs / Tests

- Confirmed no `#[test]`/`#[cfg(test)]` anywhere under the component and `[dev-dependencies]`
  is empty (Cargo.toml:15). SC-001/SC-002/SC-003 and the FR-003 signature-recovery scenario
  are functionally aligned but have zero automated verification.
- No spec statement references a concrete file/dir/proof that is missing; `plan.md`'s
  disk-layout and module-structure descriptions match the code.

## Recommendations

1. Add the unit tests already enumerated in `tasks.md` (round-trip format→initialize,
   backup fallback incl. **signature** corruption, layout error cases, UTF-16LE name
   encoding, protective MBR) so SC-001..SC-003 and the FR-003 recovery scenario become
   verifiable. This is the only remaining open item and is a normal test-authoring task,
   not spec-sync drift.
2. (Optional, future) Honor `header.partition_entry_lba` on read instead of hardcoded
   LBA 2, and consider validating `sector_size ∈ {512, 4096}` or broadening FR-011 to
   "any sector size" — both currently captured as Implementation Notes, not drift.
