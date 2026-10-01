# Kani (Role 2) progress — disk-partition-manager @ d14dfb9a

Worktree `/home/cornel/cv-disk-partition-manager-20260930`, cargo-kani 0.67.0.
Inventory: `verif/unified_properties.yaml`, **M = 102 verifiable** (+25 NV).
Harness file: `verif-kani/harnesses.rs`, wired as a **child module of `crate::gpt`** so it reaches
the private GPT items without any visibility change.

## src/ change (the only one)

`components/disk-partition-manager/src/gpt.rs`, 4 added lines at the end (593–596):

```rust

#[cfg(kani)]
#[path = "../verif-kani/harnesses.rs"]
mod verification;
```

Nothing else in `src/` was touched. `deps/spdk` and `deps/spdk-build` were symlinked into the
worktree (both are `.gitignore`d) because `spdk-sys`' build script refuses to run without them and
`interfaces` is pulled in with `features = ["spdk"]`; without that symlink **no** Kani harness in
this component can be compiled at all.

## Environment findings (step 0, beyond what the task described)

* The component itself has zero FFI, but its dependency closure does not. Building it under Kani
  requires SPDK present (`lib/spdk-sys/build.rs` panics otherwise).
* `crc32fast::hash` reaches `std::arch::x86_64::_xgetbv` →
  `unsupported_construct: call to foreign "C" function llvm.x86.xgetbv`. Defeated with a
  **faithful CRC-32/IEEE stub** (`crc32_model`, nibble-table form) whose conformance is *checked*
  by `verify_crc32_stub_conforms` against published vectors (`""`→0, `"a"`→0xE8B7BE43,
  `"123456789"`→0xCBF43926), not merely asserted.
* `generate_guid` opens `/dev/urandom`. `std::fs::File::open` is stubbed with a **conforming
  failure** (`open_fails`): `File::open` may legitimately fail and `generate_guid` deliberately
  ignores that failure, so the stub exercises a real production path.

## Fixture notes — four measured walls

1. **`core::mem::zeroed::<ClientChannels>()` is rejected** — Kani's valid-value check fires
   ("attempted to zero-initialize type `interfaces::ClientChannels`, which is invalid": `Sender`
   holds a `NonNull`). The fixture therefore builds **real** SPSC channels.
2. **Nothing may be dropped.** Dropping a `Receiver<Command>` releases the last `Arc` to the ring
   buffer, whose drop glue reaches `DmaBuffer::drop` and an **indirect call through an
   `extern "C"` free function**. Measured: >4 min, no verdict. Every harness ends with
   `core::mem::forget(m)` and the channel objects are forgotten inside the fixture.
3. **A symbolic `Vec` length segfaults CBMC.** `vec![0u8; kani::any()]` and a symbolic push count
   produced `CBMC failed with status 139` after 24 s, which Kani renders as
   `VERIFICATION:- FAILED` — i.e. a *tool crash laundered into a verdict*, exactly the hazard the
   gate's `tool_crash()` exists for. All such lengths were made concrete and the sampling is
   documented in each harness.
4. **`String::from_utf16_lossy` is a wall.** `decode_utf16le_name` ends in it, and symbolic UTF-16
   code units blow up `char::decode_utf16` + `String` growth (two symbolic ASCII units: no verdict
   in 240 s; a 4-character round trip: no verdict in 240 s). Decode-side harnesses use concrete
   code units; the encode side stays affordable.
5. **`compute_partition_layout` is a hard wall** — see below.

## The `compute_partition_layout` wall, and why the escalation order cannot clear it

`compute_partition_layout` ends with the GPT padding loop

```rust
while entries.len() < GPT_MAX_ENTRIES as usize {   // 128 iterations
    entries.push(GptEntry { /* 128 bytes */ });
}
```

so every call builds a 128-element `Vec<GptEntry>` (16 KiB) by `push`, with the reallocation chain
that implies. Measured on a **fully concrete** config: no verdict at 330 s.

Walking the mandated escalation order:

1. **`#[kani::unwind(n)]` with checks ON** — the attribute is already `unwind(200)`; the bound is
   not the problem, the SAT formula is. The scorer's own sweep tries 4/8/16/32, all **below 129**,
   so each will raise a spurious unwinding assertion rather than prove anything.
2. **`-Z loop-contracts`** — **unavailable to this role.** `#[kani::loop_invariant(...)]` must be
   attached to the loop itself, and that loop is in `src/gpt.rs`. The task's hard constraint is
   that `src/` may take "only additive `#[cfg(kani)]` module wiring", so the strongest lever in
   the escalation order cannot be applied to a production loop by a Role-2 agent at all. This is a
   genuine gap between the methodology and the role contract, not a judgement call.
3. **Shrink the problem** — `GPT_MAX_ENTRIES` is a production constant mandated by the GPT
   standard; shrinking it would be a src edit and would make the claim about a different format.
4. **`--no-unwinding-checks`** — cutting the pad loop at 4 or 8 leaves `entries.len() < 128`, so
   every shape assertion legitimately fails. It cannot rescue these harnesses either.

A solver swap (minisat vs cadical) was probed directly (`probe_layout_minisat` /
`probe_layout_cadical`).

## Harness inventory

Explicit `#[kani::proof] fn` definitions only — no `macro_rules!` anywhere, so every harness is
visible to the gate's source regex.

See `verif/kani_advisory.yaml` for the per-property mapping and the measured run table.

## Measured verdicts

The authoritative table (verdict, wall clock, peak RSS, UNDETERMINED count, check count for every
harness that was run) is the `measured:` block at the end of `verif/kani_advisory.yaml`.

**Every harness that returned a verdict reported `UNDETERMINED = 0`.** Nothing in this run passed
with undetermined checks hiding behind it.

Proved (base SUCCESSFUL **and** `__mutant` twin correctly FAILED, both at 0 undetermined):

| property | base | mutant |
|---|---|---|
| DPM-NAME-ENCODE | 6.91 s / 334 MB | FAILED 5.57 s |
| DPM-NAME-LENGTH-LIMIT | 27.96 s / 976 MB | FAILED 18.15 s |
| DPM-READ-ERR-SHORT-HEADER | 28.36 s / 424 MB | FAILED 16.21 s |
| DPM-INV-ENTRY-ARRAY-SHAPE | 30.67 s / 495 MB | FAILED 11.75 s |
| DPM-INV-ENTRY-SECTORS-CONSISTENT | 13.44 s / 411 MB | FAILED 11.15 s |
| DPM-INV-CRC-SELF-CONSISTENT | 20.90 s / 436 MB | FAILED 17.67 s |
| DPM-FORMAT-POST-HEADER-PAIR | 26.78 s / 639 MB | FAILED 36.41 s |
| DPM-FORMAT-POST-ENTRY-ARRAY | 28.37 s / 526 MB | FAILED 42.07 s |
| DPM-INV-BACKUP-MIRRORS-PRIMARY | 42.96 s / 914 MB | FAILED 30.84 s |

Stub-integrity harnesses (not inventory properties, but every CRC proof rests on them):
`verify_crc32_stub_conforms` SUCCESSFUL 9.74 s, `verify_crc32_stub_is_applied` SUCCESSFUL 0.97 s.

Refuted and reproduced (the refutation harness itself SUCCEEDS, demonstrating the defect):

| property | harness | verdict |
|---|---|---|
| DPM-FORMAT-SUM-OVERFLOW | `refute_dpm_format_sum_overflow` | SUCCESSFUL 18.66 s — `attempt to add with overflow`, "encountered one or more panics as expected" |
| | `refute_dpm_format_sum_overflow__discriminator` | SUCCESSFUL 0.96 s |
| DPM-INV-SECTOR-SIZE-DOMAIN | `refute_dpm_inv_sector_size_domain` | SUCCESSFUL 10.89 s — `index out of bounds` |

Walls hit at the 330 s cap: the whole `compute_partition_layout` group (including the
DPM-FORMAT-PRE-MAX-128 refutation and its discriminator), the `decode_utf16le_name` group, the
`parse_header` SUCCESS path when it is the only thing in the harness, and
`DiskPartitionManager::new_default()`.

### An unexpected, actionable asymmetry

`verify_dpm_read_err_short_header__split_boundary` — nothing but `mk_mgr_pure` plus one
`parse_header` on a valid 92-byte header — times out at 330 s, while
`verify_dpm_inv_crc_self_consistent` and `verify_dpm_format_post_header_pair`, which BOTH call
`parse_header` successfully on a 512-byte buffer produced by the real `serialize_header_with_crc`,
finish in 21–27 s. So the cheap route to the remaining `parse_header` properties
(DPM-READ-MYLBA-UNCHECKED, DPM-INV-SIGNATURE-REVISION, DPM-READ-STRIDE-MISMATCH) is to build the
input with the real serialiser and set the field of interest in the `GptHeader`, rather than hand-
assembling a byte array. That is the first thing to try on the next pass.

## Where this run stands

* 9 of the 102 verifiable properties have a reproducing proof with a correctly failing mutant twin.
* 2 more are REFUTED with a succeeding refutation harness (one with a discriminator).
* 2 further refutations are written but blocked by the `compute_partition_layout` wall.
* 13 more properties have harnesses that did not return a verdict inside 330 s.
* 76 have no artifact at all. That is unfinished work, not a tool boundary — see
  `verif/kani_advisory.yaml` `unfinished:` for the three blocking groups and the designed route.

`scorer_kani.py` was deliberately NOT run: it writes `verif/unified_properties.yaml`, which this
role is forbidden to touch and which a concurrent Creusot agent and the orchestrator own. Gate
visibility was instead confirmed by calling the scorer's own `find_harness_names()` — all harnesses
are discovered (no `macro_rules!` anywhere).
