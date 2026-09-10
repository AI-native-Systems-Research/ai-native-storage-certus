# Verified properties — disk-partition-manager (Kani)

**Role 2 (Kani lane), CLEAN-SLATE RE-RUN 2026-09-09 under TIGHT knobs.** Verified from the
property inventory `disk-partition-manager_property_inventory.md` (Role 1, N=4 methods, M=35)
against code `src/gpt.rs` + `src/lib.rs`. Harnesses: `#[cfg(kani)] mod verification` in both
files. Toolchain **Kani 0.67.0 / CBMC**; command
`cargo kani -Z unstable-options --output-format=terse --harness-timeout 90s -j 24 --default-unwind 3`.
Branch `verif/kani/disk-partition-manager-rerun` (baseline `31f92e61`). Consumed by `id`; no
property re-extracted or invented. **Under-claims:** only a green harness marks an id proved.

## Backing-structure scout (done FIRST)

GPT state = 92-byte header + 128×128B entry array **as byte slices** (`[u8]`); CRC32 over byte
ranges; layout/LBA logic is **integer arithmetic**; a UTF-16LE name codec. **No BTreeMap/HashMap/
raw-pointer** in gpt logic; `Vec` bounded ≤128. Arithmetic core ≈ 0.03–0.17 s / ~170 MB. Four
intrinsic Kani/CBMC walls are captured by `wall_*`/gate harnesses (not asserted): `_xgetbv` from
`crc32fast`; integer `format!` non-termination; the 128-slot entry-array zero-pad (unwind≥129);
and macro-component construction (std HashMap getrandom/SipHash seeding).

## Scoreboard (per method; globals counted in each bundle)

| method | bundle | proved (green) ids | strict | relaxed |
|---|:--:|---|:--:|:--:|
| `initialize` | 12 | G6 (offset roundtrip, arithmetic core `✓~`) | ✗ | **✓** |
| `format` | 24 | ERR-MULTI-REST, DISK-GUID-V4, PART-GUID-V4, NAME-LEN-36, PART-SIZE-CEIL, PART-NONOVERLAP, PART-WITHIN-USABLE, RESTOFDISK-REMAINING, BACKUP-MIRRORS-PRIMARY, G6 core | ✗ | **✓** |
| `partition_info` | 6 | — (gate wall G2; populated-state rows I/O-walled) | ✗ | ✗ |
| `num_partitions` | 4 | — (gate wall G2; RETURNS-LEN I/O-walled) | ✗ | ✗ |

**Headline: strict 0/4, relaxed 2/4. 10 unique inventory ids proved** (baseline 11; G7 regressed
to ⊘ under the 90 s cap — see below). Status markers: `✓` green · `✓~` arithmetic-core/bounded ·
`⊘` tool-boundary (real run + reproducible signature) · `✎` not-expressible bounded · `⊘pre`
assumed precondition.

## Global invariants (§1)

- `DPM-ROUNDTRIP-OFFSETS` (G6) **[Invariant]** `✓~` — the offset↔sector inverse
  (`ending = start + n − 1 ⟺ n = ending − start + 1`) proved symbolically with overflow guarded —
  `verify_ending_lba_inverse` (SUCCESSFUL, anti-vacuity anchored). Full byte+CRC read-back is ⊘
  (`_xgetbv` + block I/O).
- `DPM-ROUNDTRIP-NAME` (G7) **[Invariant]** `⊘` — **REGRESSED from baseline SUCCESSFUL.** The
  identical `verify_name_roundtrip_ascii` (ASCII, ≤5 code units, `#[kani::unwind(6)]`) now
  **times out at the 90 s harness cap (rc=124)**; prior run: 592.9 s CBMC / 19.5 GB. Symbolic
  `from_utf16_lossy` cost, not unwind depth. Route: raise this harness's timeout, or Creusot.
- `DPM-SECTOR-SIZE-512-4096` (G1) **[Precondition]** `⊘pre` — assumed `∈{512,4096}` in every
  harness; code is sector-size-generic (inventory R3), so a precondition, not an obligation.
- `DPM-STATE-CACHED` (G3), `DPM-IO-PROPAGATE` (G4), `DPM-NAMESPACE-ID` (G9) `⊘` — reached only via
  the initialize/format I/O path (block I/O effects Kani cannot model).
- `DPM-CRC-INTEGRITY` (G8) **[Invariant]** `⊘` — `wall_crc32_xgetbv`: `crc32fast::hash` →
  `unsupported constructs: InlineAsm, caller_location, catch_unwind, foreign function` (`_xgetbv`).
- `DPM-INIT-GATE` (G2) **[Precondition]** `⊘` — `verify_init_gate_num_partitions` (lib.rs):
  `DiskPartitionManager::new_default()` construction times out at 90 s (rc=124) on the `InterfaceMap`
  std-HashMap getrandom/SipHash seeding. **Attempted + captured** (baseline had left it unauthored).
  Route: Creusot.
- `DPM-COUNT-INDEX-AGREE` (G5) `⊘` — query logic reachable only via a populated table (I/O wall).

## initialize
- `DPM-ROUNDTRIP-OFFSETS` (G6) `✓~`; `DPM-ROUNDTRIP-NAME` (G7) `⊘` (was ✓ — 90 s cap).
- `DPM-INIT-REQUIRES-BLOCKDEV` `⊘` (receptacle binding, trusted boundary §7).
- `DPM-INIT-BACKUP-FALLBACK`, `-BOTH-CORRUPT-NOPTBL`, `-RETURNS-CORRECT-LAYOUT` `⊘` (CRC/`format!`
  read path). Supporting green (not M ids): `verify_parse_header_too_short` (PARSE-HDR-TOO-SHORT),
  `verify_parse_entries_bounds` (PARSE-ENTRY-BOUNDS) — both real-call, anti-vacuity anchored; the
  three R4 harnesses discharge the backup-LBA / ending-LBA underflows the read path assumes.
- `DPM-INIT-IO-2RT` **[Frame]** `✎` — ≤2 read round-trips: resource/frame, not bounded-expressible.

## format
- `DPM-FORMAT-ERR-MULTI-REST` `✓` — `verify_layout_multi_rest_err` (real `compute_partition_layout`,
  early-return before the 128-pad, `#[kani::unwind(4)]`; anchored in prod: guard `>1`→`>2` flips it).
- `DPM-FORMAT-DISK-GUID-V4` / `-PART-GUID-V4` `✓` — `verify_guid_v4_nibbles` (symbolic `[u8;16]`
  version/variant masking; anchored: `|0x40`→`|0x30` flips it).
- `DPM-FORMAT-NAME-LEN-36` `✓` — `verify_name_len_36` (SUCCESSFUL, 41.3 s). **Disclosed: structural
  tautology** (`[u8;72]` return type), not independently anti-vacuity-anchorable.
- `DPM-FORMAT-PART-SIZE-CEIL`, `-PART-NONOVERLAP`, `-PART-WITHIN-USABLE` `✓` —
  `verify_layout_placement_arithmetic` (placement arithmetic mirroring gpt.rs:270-296; anchored).
- `DPM-FORMAT-RESTOFDISK-REMAINING` `✓` — `verify_layout_restofdisk_arithmetic` (same-class anchored).
- `DPM-FORMAT-BACKUP-MIRRORS-PRIMARY` `✓` — `verify_backup_mirror` (anchored).
- `DPM-FORMAT-REQUIRES-BLOCKDEV` `⊘` (receptacle); `-DEVICE-TOO-SMALL` `⊘` (guard arithmetic proved
  by R4; live path crosses write/`format!`); `-ERR-OVERSUBSCRIBED` `⊘` (error-arm `format!`
  non-termination).
- `DPM-FORMAT-WRITES-5-STRUCTURES`, `-ENTRY-ARRAY-128x128`, `-TYPEGUID-PRESERVED` `⊘` —
  `wall_layout_happy_path_pad`: the 128-slot zero-pad trips an **unwinding assertion** at
  `--default-unwind 3` (would SAT-explode at unwind≥129). Route: Creusot.
- `DPM-FORMAT-HEADER-CONSTANTS` `⊘` (CRC-blocked write path).
- `DPM-FORMAT-IO-O1` **[Frame]** `✎` — write count O(1): resource/frame, not bounded-expressible.
- Supporting green: `verify_entry_sectors_real` (512→32, 4096→4; anchored in prod).

## partition_info / num_partitions
No green id. Both bundles are `Mutex<Option<PartitionTable>>` / `Vec` logic reachable only through a
constructed `DiskPartitionManager` (gate wall G2, `⊘`) or a populated table via initialize/format
(I/O+CRC wall). `DPM-PINFO-INDEX-RANGE`, `-RETURNS-ENTRY`, `-READONLY`, `DPM-NUMP-RETURNS-LEN`,
`DPM-COUNT-INDEX-AGREE` (G5) all `⊘` for that reason.

## R4 — latent unguarded-underflow obligations (panic-freedom; inventory R4, not M ids)
Kani **confirmed all three underflows exist** and proved the guarded code safe above the implied
preconditions: gpt.rs:148 `verify_r4_last_usable_underflows_below_min` (anchored) +
`_safe_above_min`; gpt.rs:77-79 `verify_r4_backup_lba_underflows_tiny`; gpt.rs:129
`verify_r4_ending_minus_starting` (also found `+1` overflow at `diff==u64::MAX`). Recommendation:
`checked_sub`/`checked_add` or hoist the too-small guard above gpt.rs:148.

## Assumptions / bounds
- `kani::assume(sector_size ∈ {512,4096})` — mirrors inventory G1 (not code-enforced, R3).
- `#[kani::unwind(6)]` on name harnesses; `(4)` on parse/layout early-return harnesses;
  `--default-unwind 3` elsewhere.
- Associated fns (`entry_sectors`, `parse_header`, `parse_entries`, `compute_partition_layout`)
  called directly — no `GptManager`/`ClientChannels` instance (I/O construction is a wall).
- SPDK build accommodation: gitignored `deps/spdk`, `deps/spdk-build` symlinks; no code changed.

## Not verified — attempted, tool boundary hit (⊘, with signatures)
- **G7 DPM-ROUNDTRIP-NAME** — timeout `rc=124` at the 90 s cap (was 592.9 s SUCCESSFUL).
- **G8 DPM-CRC-INTEGRITY** — `unsupported constructs` (`_xgetbv`: InlineAsm + foreign function).
- **PARSE-SIGNATURE / DPM-INIT-BOTH-CORRUPT-NOPTBL** — `format!` non-termination, `rc=124` at 90 s.
- **DPM-FORMAT-ENTRY-ARRAY-128x128 / -TYPEGUID-PRESERVED / -WRITES-5** — unwinding assertion (128-pad).
- **G2 DPM-INIT-GATE** — `new_default` HashMap/getrandom construction wall, `rc=124` at 90 s.
- I/O-effect ids (G3, G4, G9, INIT-REQUIRES-BLOCKDEV, FORMAT-REQUIRES-BLOCKDEV, FORMAT-DEVICE-TOO-SMALL,
  FORMAT-ERR-OVERSUBSCRIBED, WRITES-5, HEADER-CONSTANTS, INIT-BACKUP-FALLBACK, INIT-RETURNS-LAYOUT) —
  block I/O / CRC / `format!` paths. Route: Creusot + fault-injection/integration tests.

## Not verified — not expressible for a bounded checker (✎)
- `DPM-INIT-IO-2RT`, `DPM-FORMAT-IO-O1` — resource/round-trip-count frame properties. Route: cost
  model / integration test.
