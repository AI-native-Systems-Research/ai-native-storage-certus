# extent-manager — Creusot proof ledger (skill shape)

**Role 2 output** (`tools-verify-creusot-with-properties`, clean-slate re-run). Every id in the
inventory (`extent-manager_property_inventory.md`, M=47 over N=14 methods) reaches exactly one
end-state; "contract not written" is never an outcome. A property is **P** only where the contract
was authored **and** its VCs discharged in *this* worktree.

- Worktree / branch: `/home/cornel/em-creusot-rerun` on `verif/creusot/extent-manager-rerun`, from baseline `verif/creusot/extent-manager@5dcadff1`. Baseline branch untouched.
- `cargo creusot`: **Proved (56 files) ✔** — 56 `.coma` modules, 92 top-level VC goals, **170 leaf goals** after `split_vc` (alt-ergo 154, z3 12, cvc5 4; cvc4 unused).
- Toolchain `~/.local/share/creusot/bin`; provers alt-ergo/z3/cvc5/cvc4, tactics `compute_specified`+`split_vc`, `time=8 depth=6`.
- **`components/interfaces/` NOT modified** — no shared-crate change was needed.

Legend: **P** proved natively · **⊘** tool boundary (byte-serialization / CRC / device I/O / std-container / RAII / feature-gated — precise reason given) · **⤴** delegated across a real boundary (concurrency → Loom).

## Headline
- **Per-obligation:** strict **37/47 P**; relaxed **38/47** (crediting the 1 ⤴); **9 ⊘**, **1 ⤴**.
- **Per-method (N=14):** strict **10/14** (all obligations native-P); relaxed **14/14** (every obligation reaches a disclosed end-state; no method is all-boundary, no open code-vs-spec gap).

## Global-invariant ledger (prove-once; propagate by id)

| rank | id | end-state | evidence (module :: fn) |
|---|---|---|---|
| 1 | EM-INIT-GATE (8) | **P** | `gate.rs` :: gate_result (Shape 1 → Err(NotInitialized)), gate_scalar (Shape 2 → 0), gate_count (Shape 3 → empty/no-op); F-6 three shapes. FI-1 RED (also cascaded to get_instance_id — propagation confirmed). |
| 2 | EM-KEYVEC-MEMBERSHIP (4) | **P** | `keyvec.rs` :: member, new (all FREE_KEY→!member), get_key, set_key (frame + member↔key≠FREE_KEY), free_key, num_members (recursive), count_members (loop = num_members). FI-2 RED. |
| 3 | EM-SECTOR-ALIGN (3) | **P** | `align.rs` :: align_to_sector_size (multiple/covers/minimal); `slab_geom.rs` :: slot_offset. FI-11 RED. |
| 4 | EM-DEFERRED-FREE (3) | **P** | `deferred_free.rs` :: deferred_remove (bitmap bit stays set → not reusable), flush_free (checkpoint clears bit), lifecycle_no_reuse_before_checkpoint (proof_assert crash-safety). FI-3 RED. |
| 5 | EM-DATABASE-ROUNDTRIP (2) | **P** | `config.rs` :: LbaCell new/set/get, roundtrip (get∘set = id, default 0). FI-4 RED. |
| 6 | EM-USED-LE-CAP (2) | **P** | `accounting.rs` :: sum_usable/sum_used (recursive), lemma_used_le_usable (induction), lemma_sum_usable_mono, capacity_bytes, used_bytes (`Σ(usable−free) ≤ Σ usable`). FI-5 RED. |

## Per-method obligation table (every inventory id)

| method | obligation id | end-state | evidence / boundary reason |
|---|---|---|---|
| **format** | EM-FORMAT-VALIDATE-SECTOR | P | `format_validate.rs` :: validate_format_params (sector==0→CorruptMetadata). FI-6 RED |
| | EM-FORMAT-VALIDATE-SLABMULT | P | validate_format_params (slab%sector≠0) |
| | EM-FORMAT-VALIDATE-MAXEXT | P | validate_format_params (max_ext>slab) |
| | EM-FORMAT-VALIDATE-REGPOW2 | P | `format_validate.rs` :: check_region_count_pow2 (`n&(n-1)` mask, #[bitwise_proof]). FI-7 RED |
| | EM-FORMAT-META-TOOSMALL | P | check_checkpoint_region_size (==0→CorruptMetadata) |
| | EM-FORMAT-NO-DATA-SPACE | P | check_usable_data_space (==0→CorruptMetadata) |
| | EM-FORMAT-META-NOT-CONNECTED | P | `gate.rs` :: gate_result (same Option gate → NotInitialized) |
| | EM-FORMAT-DATA-LAYOUT | P | `format_validate.rs` :: data_start_offset (shared vs separate device) |
| | EM-FORMAT-WRITES-SUPERBLOCK | ⊘ | byte serialization + CRC32 over bytes 0-91 + LBA-0 device write; no extern spec for `crc32fast`/device I/O |
| **initialize** | EM-INIT-VALIDATE-MAGIC | P | `recovery_model.rs` :: validate_magic |
| | EM-INIT-VALIDATE-CRC | P | `recovery_model.rs` :: validate_crc (comparison guard; CRC byte computation itself ⊘, noted) |
| | EM-RECOVER-ROUNDTRIP | P | `recovery_model.rs` :: bitmap_from_keys — BITMAP-REFLECTS-KEYS core (`bit[i] ↔ keys[i]≠FREE_KEY`). FI-9 RED. Full disk-read residual noted below |
| | EM-RECOVER-FRESH-EMPTY | P | `recovery_model.rs` :: recovered_extent_count (seq==0→0). FI-14 RED |
| | EM-RECOVER-LAST-CKPT-CONSISTENCY | P (shared core) | direct reading of bitmap_from_keys: recovered state is a pure function of the *persisted* key vector, so post-checkpoint (non-persisted) publishes are definitionally absent. No separate VC |
| | EM-RECOVER-DUALCOPY | ⊘ | device reads of active/inactive copies + CRC/media-error fallback (I/O + CRC) |
| | EM-RECOVER-BOTH-CORRUPT | ⊘ | terminal branch of the same device-read + CRC check |
| **reserve_extent** | EM-RESERVE-INVISIBLE-UNTIL-PUBLISH | P | `keyvec.rs` :: new (reserved slot keeps FREE_KEY → !member until publish) |
| | EM-RESERVE-OVERSIZE | P | `format_validate.rs` :: check_element_fits (aligned elem>slab→OutOfSpace) |
| | EM-PUBLISH-VISIBLE | P | `keyvec.rs` :: set_key (key≠FREE_KEY → member, reads back key) |
| | EM-PUBLISH-FREEKEY-DISCARD | P | `keyvec.rs` :: set_key (key==FREE_KEY → !member) |
| | EM-ABORT-RELEASES | P | `keyvec.rs` :: free_key (→!member) + `deferred_free.rs`/`bitmap.rs` (bit clear = reusable) |
| | EM-SHARD-BY-KEY | P | `shard.rs` :: region_for_key (pow2 mask → in-bounds). FI-10 RED |
| | EM-RESERVE-OUTOFSPACE | ⊘ | buddy free-list exhaustion via `Vec::swap_remove`/`position`; no extern spec |
| | EM-RESERVE-SIZECLASS | ⊘ | `SizeClassManager` uses std `HashMap` (no logic specs) + buddy alloc boundary |
| | EM-DROP-AUTO-ABORT | ⊘ | RAII `Drop` dispatch is a language-mechanism obligation; released-state effect ≡ EM-ABORT-RELEASES (P), Drop-calls-abort wiring trusted by inspection |
| **get_extents** | EM-GETEXTENTS-EXACT | P | `keyvec.rs` :: count_members (count) + `slab_geom.rs` :: slot_offset (offset) + membership |
| | EM-GETEXTENTS-EMPTY-FRESH | P | `keyvec.rs` :: new + `gate.rs` :: gate_count (None→0) |
| **for_each_extent** | EM-FOREACH-COUNT | P | `keyvec.rs` :: count_members = num_members (N members → N callbacks) |
| **remove_extent** | EM-REMOVE-HIDES | P | `keyvec.rs` :: free_key (slot→FREE_KEY → !member) |
| | EM-REMOVE-NOTFOUND | P | `shard.rs` :: region_for_offset_index (out-of-range→None) + `slab_geom.rs` :: contains_offset/slot_for_offset |
| | EM-REMOVE-OFFSET-ROUTING | P | `shard.rs` :: region_for_offset_index (in-bounds) + `slab_geom.rs` :: slot_for_offset/contains_offset. `BTreeMap::range` navigation trusted by inspection |
| **checkpoint** | EM-CKPT-SKIP-CLEAN | P | `checkpoint_gate.rs` :: checkpoint_action + double_checkpoint_second_skips (clean→no I/O). FI-8 RED |
| | EM-CKPT-DURABLE | ⊘ | serialize regions + CRC32 blob + inactive-copy device write + superblock active-switch + seq bump; byte I/O |
| | EM-CKPT-FLUSH | ⊘ | feature `volatile_write_cache` device flush ordering; absent from default build (F-5) |
| | EM-CKPT-HOOK | ⊘ | post-checkpoint hook registration is concrete-only (F-3); callback dispatch not modeled |
| | EM-CKPT-COALESCE | ⤴ | concurrency (F-4) — coalescing / at-most-one-in-flight → Loom lane |
| **get_instance_id** | EM-INSTANCEID-RETURNS | P | `gate.rs` :: get_instance_id (gated read of stored id) |
| **set_checkpoint_interval** | EM-CKPTINTERVAL-SET | P | `config.rs` :: IntervalCell new (30 s default) / set_interval (Some/None). F-1 doc divergence noted |
| **used_bytes** | EM-USEDBYTES-BUDDY-GRANULARITY | P | `accounting.rs` :: used_bytes = Σ(usable−free) |
| **capacity_bytes** | EM-CAPACITY-USABLE | P | `accounting.rs` :: capacity_bytes = Σ usable |
| **set_metadata_base_lba** | EM-METABASE-SHIFTS-IO | P | `config.rs` :: shifted_block_lba (`block_lba = base+lba+i`) |
| **set_data_base_lba / data_base_lba** | EM-DATABASE-ROUNDTRIP | P | global #5 (`config.rs`) — these methods' entire contract |

## Residual gaps (what green does NOT prove)
- **Mirror boundary:** the extent-manager crate cannot build under Creusot (`interfaces` crate, `parking_lot`, `HashMap`/`BTreeMap`, SPDK/DMA FFI, async), so each module is a faithful whole-function mirror; residual mirror-vs-source body equality is discharged by inspection against the cited source line ranges.
- **⊘ boundaries (9):** byte serialization + CRC32 (`crc32fast`) and metadata-device reads/writes (WRITES-SUPERBLOCK, CKPT-DURABLE, RECOVER-DUALCOPY, RECOVER-BOTH-CORRUPT, CKPT-FLUSH); std-container semantics with no logic specs (RESERVE-SIZECLASS `HashMap`, RESERVE-OUTOFSPACE buddy `swap_remove`/`position`); RAII `Drop` dispatch (DROP-AUTO-ABORT); closure callback dispatch (CKPT-HOOK).
- **⤴ delegation (1):** EM-CKPT-COALESCE — single-threaded deductive lane cannot express concurrent coalescing (F-4); carried to Loom.
- **Anti-vacuity:** 14 fault injections (FI-1..FI-14), one per module/global, each turned its target VC ✘ then reverted; final tree re-verified `Proved (56 files) ✔`.
