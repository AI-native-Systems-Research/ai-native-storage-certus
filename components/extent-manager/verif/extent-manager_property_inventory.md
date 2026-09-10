# extent-manager — Property Inventory (tool-independent, reconciled)

**Role 1 output** (`build-property-inventory`). Single source of truth: id-keyed verifiable
properties, each traced to spec + code, bundled onto the 14 `IExtentManager` public methods.
**No prover is named here** and **no proof status is recorded** — lanes/status are added downstream
(Role 2 verify, Role 3 aggregate).

- Spec read: `components/extent-manager/specs/001-extent-manager-v2/spec.md` (blind extraction)
- Code read: `src/lib.rs`, `src/region.rs`, `src/slab.rs`, `src/buddy.rs`, `src/bitmap.rs`,
  `src/checkpoint.rs`, `src/recovery.rs`, `src/superblock.rs`, `interfaces/src/iextent_manager.rs`
  (blind extraction)
- Workspace / commit: `em-creusot-rerun` @ `5dcadff1`, `em-kani-rerun` @ `b4ace468`
- Reconciled: **2026-09-09** (both extractions independent/blind; agreement is real signal).

## Denominator
- **N = 14 public methods** (the `define_interface! { IExtentManager { … } }` block; excludes
  `Display::fmt` and `#[cfg(test)]`). Concrete-only methods `set_metadata_ns_id`, `set_dma_alloc`,
  `set_post_checkpoint_hook`, and the `WriteHandle` methods (`publish`/`abort`/`key`/…) are **NOT**
  in the trait and are excluded from N — see reconciliation flag F-3.
- **M = 47 distinct verifiable properties.**
- **Attachments = 63** (Σ of bundle sizes; a property shared by k methods counts k times).

The two counts differ because **6 properties are named global invariants / shared obligations** that
attach to several methods (e.g. `EM-INIT-GATE` serves all 8 data-path ops; `EM-KEYVEC-MEMBERSHIP`
serves reserve/remove/get/for_each). This is expected and correct — one property, many bundles.

Every property carries `global: true|false` (true iff `|methods| > 1`) and `attachments` (= `|methods|`).
These are the **leverage signal** Role 2 consumes: prove globals **once, highest-`attachments` first**,
and the `proved` status propagates to every bundle by `id`.

## Bundle-size distribution (counted, not eyeballed)
- **N = 14** methods; **Σ attachments = 63**
- **median = 3.5**, **mean = 4.5**
- **range = 1 … 13** (min 1, max 13)
- **mode = 1** (four methods have bundle size 1)
- sizes ascending: `1, 1, 1, 1, 2, 3, 3, 4, 5, 6, 7, 7, 9, 13`

## Global-invariant ledger — Role-2 prove-once worklist (sorted by attachments ↓)

Prove each of these **once** as a maintained invariant, in this order; discharging the top of the list
unblocks the most public methods per proof.

| rank | id | attachments | global | proof note |
|---|---|---|---|---|
| 1 | EM-INIT-GATE | 8 | ✓ | **highest leverage.** No data-path op acts before `format`/`initialize` populates `regions`+`shared`. Manifests three ways (see F-6): `Err(NotInitialized)` for reserve/remove/checkpoint/get_instance_id; empty/`0` for get_extents/used_bytes/capacity_bytes; silent no-op for for_each_extent. One obligation, method-specific shape. Cheap; unblocks all 8. |
| 2 | EM-KEYVEC-MEMBERSHIP | 4 | ✓ | **the hard one** — needs a logic-level model of the per-slab dense key vector (`Vec<u64>`, `FREE_KEY = u64::MAX` sentinel). An extent is enumerable **iff** its slot key ≠ FREE_KEY. Publish sets the key; remove/abort/free set FREE_KEY. |
| 3 | EM-SECTOR-ALIGN | 3 | ✓ | every reserved/enumerated extent's `size` and `offset` are `sector_size`-aligned (`aligned = (x + ss − 1)/ss·ss`; `slot_offset = start + idx·element_size`). Arithmetic; align lemma. |
| 4 | EM-DEFERRED-FREE | 3 | ✓ | crash-safety core (FR-025). A removed slot is pushed to `pending_frees`, NOT freed, and MUST NOT be reallocated until a successful `checkpoint()` runs `flush_pending_frees`. Needs the pending-free set + reserve-doesn't-reuse reasoning. |
| 5 | EM-DATABASE-ROUNDTRIP | 2 | ✓ | `data_base_lba()` returns the value last stored by `set_data_base_lba` (default 0); component does **no** data-device I/O (caller-consumed, FR-036). Trivial get/set roundtrip on a `Mutex<u64>`. |
| 6 | EM-USED-LE-CAP | 2 | ✓ | `used_bytes() ≤ capacity_bytes()` always: `used = Σ(usable − free)`, `cap = Σ usable`, `free ≥ 0`. Derived accounting invariant (code-only; spec states neither ≤). |

**Reading of this table:** rank 1 (`EM-INIT-GATE`) is the cheap +8 win; rank 2
(`EM-KEYVEC-MEMBERSHIP`) is the key-vector modelling campaign (analogous to memory-tier's
`CONTAINS-REFLECTS` FMap gap) and the highest-value hard target. Ranks 3–6 are arithmetic /
roundtrip follow-ons.

---

## Bundle size per public method (counted)

| # | method | bundle size | unique to method | shared globals in bundle |
|---|---|---|---|---|
| 1 | reserve_extent | 13 | 9 | EM-INIT-GATE, EM-KEYVEC-MEMBERSHIP, EM-SECTOR-ALIGN, EM-DEFERRED-FREE |
| 2 | format | 9 | 9 | — (initializer; no global gates it) |
| 3 | initialize | 7 | 7 | — (initializer; no global gates it) |
| 4 | checkpoint | 7 | 5 | EM-INIT-GATE, EM-DEFERRED-FREE |
| 5 | remove_extent | 6 | 3 | EM-INIT-GATE, EM-KEYVEC-MEMBERSHIP, EM-DEFERRED-FREE |
| 6 | get_extents | 5 | 2 | EM-INIT-GATE, EM-KEYVEC-MEMBERSHIP, EM-SECTOR-ALIGN |
| 7 | for_each_extent | 4 | 1 | EM-INIT-GATE, EM-KEYVEC-MEMBERSHIP, EM-SECTOR-ALIGN |
| 8 | used_bytes | 3 | 1 | EM-INIT-GATE, EM-USED-LE-CAP |
| 9 | capacity_bytes | 3 | 1 | EM-INIT-GATE, EM-USED-LE-CAP |
| 10 | get_instance_id | 2 | 1 | EM-INIT-GATE |
| 11 | set_checkpoint_interval | 1 | 1 | — |
| 12 | set_metadata_base_lba | 1 | 1 | — |
| 13 | set_data_base_lba | 1 | 0 | EM-DATABASE-ROUNDTRIP |
| 14 | data_base_lba | 1 | 0 | EM-DATABASE-ROUNDTRIP |

Σ bundle sizes = 63 = Σ attachments. ✓

---

## Named global invariants / shared obligations (6)

| id | statement | methods (attachments) | origin |
|---|---|---|---|
| EM-INIT-GATE | No data-path op acts before `format`/`initialize` populates `regions`+`shared`; gated calls return NotInitialized / empty / 0 / no-op without mutation. | reserve_extent, remove_extent, get_extents, for_each_extent, checkpoint, get_instance_id, used_bytes, capacity_bytes (8) | spec+code |
| EM-KEYVEC-MEMBERSHIP | An extent is enumerable iff its slab key-slot ≠ FREE_KEY (`u64::MAX`); the dense per-slab `Vec<u64>` is the sole membership record (no separate index). | reserve_extent, remove_extent, get_extents, for_each_extent (4) | spec+code |
| EM-SECTOR-ALIGN | Every reserved/enumerated extent's `size` and `offset` are `sector_size`-aligned. | reserve_extent, get_extents, for_each_extent (3) | spec+code |
| EM-DEFERRED-FREE | A slot freed by `remove_extent` is not reallocated until the removal is persisted by a successful `checkpoint()` (deferred free via `pending_frees` → `flush_pending_frees`). | remove_extent, checkpoint, reserve_extent (3) | spec+code |
| EM-DATABASE-ROUNDTRIP | `data_base_lba()` returns the value last set by `set_data_base_lba` (default 0); the component performs no data-device I/O with it (caller-consumed). | set_data_base_lba, data_base_lba (2) | spec+code |
| EM-USED-LE-CAP | `used_bytes() ≤ capacity_bytes()` at every reachable state. | used_bytes, capacity_bytes (2) | code-only (derived) |

---

## Per-method unique obligations (41)

### format (9)
- **EM-FORMAT-VALIDATE-SECTOR** — error — `sector_size == 0` → CorruptMetadata. `[spec: FR-002]` `[code: lib.rs:384]`
- **EM-FORMAT-VALIDATE-SLABMULT** — error — `slab_size % sector_size != 0` → CorruptMetadata. `[spec: FR-002]` `[code: lib.rs:387]`
- **EM-FORMAT-VALIDATE-MAXEXT** — error — `max_extent_size > slab_size` → CorruptMetadata. `[spec: FR-002]` `[code: lib.rs:392]`
- **EM-FORMAT-VALIDATE-REGPOW2** — error — `region_count == 0` or not a power of two → CorruptMetadata. `[spec: FR-002]` `[code: lib.rs:397]`
- **EM-FORMAT-META-TOOSMALL** — error — metadata device too small for two checkpoint regions (`checkpoint_region_size == 0`) → CorruptMetadata. `[spec: FR-002, edge]` `[code: lib.rs:434]`
- **EM-FORMAT-NO-DATA-SPACE** — error — no usable data space after metadata reservation (`usable_data_size == 0`) → CorruptMetadata. `[code: lib.rs:448]` *(code-only)*
- **EM-FORMAT-META-NOT-CONNECTED** — error — metadata block device receptacle not connected → NotInitialized. `[code: lib.rs:406-409]` *(code-only)*
- **EM-FORMAT-WRITES-SUPERBLOCK** — postcondition — writes a valid superblock (magic `CERTUSV4` `0x4345_5254_5553_5634`, version 6, CRC32 over bytes 0-91) at LBA 0 of the metadata device. `[spec: FR-003]` `[code: lib.rs:483-498, superblock.rs:58-97]`
- **EM-FORMAT-DATA-LAYOUT** — postcondition — `metadata_region_size > 0` ⟹ `data_start_offset = checkpoint_region_offset + 2·checkpoint_region_size` (shared device); `== 0` ⟹ `data_start_offset = 0` (separate device). `[spec: FR-035]` `[code: lib.rs:442-446]`

### initialize (7)
- **EM-INIT-VALIDATE-MAGIC** — error — superblock magic mismatch → CorruptMetadata (message names the magic). `[spec: FR-004, US3-AS3]` `[code: superblock.rs:108-112]`
- **EM-INIT-VALIDATE-CRC** — error — superblock CRC32 mismatch → CorruptMetadata. `[spec: US3-AS4]` `[code: superblock.rs:146-150]`
- **EM-RECOVER-ROUNDTRIP** — postcondition — after a successful checkpoint + reboot, `initialize()` restores exactly the extents present at the last checkpoint with correct key/offset/size, rebuilding the bitmap (slot allocated iff key ≠ FREE_KEY) and buddy state (`mark_allocated`). `[spec: FR-004, FR-018, US2-AS1, SC-002]` `[code: lib.rs:514-582, recovery.rs:76-85]`
- **EM-RECOVER-DUALCOPY** — postcondition — recovery reads the active copy first; on CRC/media error it falls back to the inactive (previous-seq) copy. `[spec: FR-017, US3-AS1/AS2, SC-003]` `[code: recovery.rs:29-66]`
- **EM-RECOVER-BOTH-CORRUPT** — error — both active and inactive checkpoint copies corrupt → CorruptMetadata. `[code: recovery.rs:68-70]` *(code-only)*
- **EM-RECOVER-FRESH-EMPTY** — postcondition — `checkpoint_seq == 0` (formatted, never checkpointed) → recover empty per-region state, no error. `[code: recovery.rs:18-21]` *(code-only)*
- **EM-RECOVER-LAST-CKPT-CONSISTENCY** — postcondition — extents published after the last checkpoint are absent after recovery (last-checkpoint consistency). `[spec: US2-AS2, FR-018]` `[code: recovery.rs (state derived solely from persisted key vectors)]`

### reserve_extent (9)
- **EM-RESERVE-INVISIBLE-UNTIL-PUBLISH** — postcondition — a reserved (unpublished) extent is not returned by `get_extents()`/`for_each_extent()`. `[spec: FR-005, US1-AS1, US5-AS3]` `[code: lib.rs:584-630, region.rs:41-92 (slot allocated, key stays FREE_KEY)]`
- **EM-RESERVE-OUTOFSPACE** — error — data device full / no fitting slot → OutOfSpace. `[spec: FR-005, edge]` `[code: region.rs:72-75]`
- **EM-RESERVE-OVERSIZE** — error — aligned `element_size > slab_size` → OutOfSpace. `[code: region.rs:68-70]` *(code-only)*
- **EM-RESERVE-SIZECLASS** — postcondition — each distinct sector-aligned size gets its own size class; same-size extents pack into a slab; new slabs are allocated on demand from the buddy allocator. `[spec: FR-020, FR-021, edge]` `[code: region.rs:41-92, slab.rs:29-35]`
- **EM-PUBLISH-VISIBLE** — postcondition — `WriteHandle::publish()` writes the key into the slot's key vector; the extent then appears in `get_extents()` with correct key + offset. `[spec: FR-006, US1-AS2]` `[code: iextent_manager.rs:132-139, lib.rs:597-616, region.rs:114-119]` *(WriteHandle method; bundled on reserve_extent — see F-3)*
- **EM-PUBLISH-FREEKEY-DISCARD** — postcondition — `publish()` with `key == FREE_KEY` (`u64::MAX`) → returns Ok, the slot is immediately freed, and nothing becomes visible. `[spec: FR-006, US1-AS5, edge]` `[code: lib.rs:600-608]`
- **EM-ABORT-RELEASES** — postcondition/frame — `WriteHandle::abort()` releases the allocated slot without writing any key; the slot becomes reusable. `[spec: FR-007, US1-AS3]` `[code: iextent_manager.rs:141-146, lib.rs:618-621, region.rs:94-112]`
- **EM-DROP-AUTO-ABORT** — postcondition — dropping a `WriteHandle` without `publish()`/`abort()` auto-aborts the reservation (RAII). `[spec: FR-033, US1-AS4, edge]` `[code: iextent_manager.rs:149-155]`
- **EM-SHARD-BY-KEY** — invariant — the target region for a key is `key & (region_count − 1)`. `[spec: FR-022]` `[code: lib.rs:211-221]` *(|methods| = 1)*

### get_extents (2)
- **EM-GETEXTENTS-EXACT** — postcondition — returns exactly the set of published-not-removed extents with correct key/offset/size, and the count matches. `[spec: FR-009, US5-AS1, SC-001]` `[code: lib.rs:632-656]`
- **EM-GETEXTENTS-EMPTY-FRESH** — postcondition — freshly formatted, no published extents → empty `Vec`. `[spec: US5-AS2]` `[code: lib.rs:632-656]`

### for_each_extent (1)
- **EM-FOREACH-COUNT** — postcondition — the callback is invoked exactly once per published-not-removed extent (N published → N calls). `[spec: FR-009, US5-AS4]` `[code: lib.rs:658-678]`

### remove_extent (3)
- **EM-REMOVE-HIDES** — postcondition — `remove_extent(O)` on a published extent succeeds and sets the slot key to FREE_KEY in memory; the extent immediately ceases to appear in enumeration. `[spec: FR-008, US6-AS1]` `[code: lib.rs:680-684, region.rs:121-150]`
- **EM-REMOVE-NOTFOUND** — error — no allocated extent at `offset` (out of range, misaligned, unset bitmap, or already FREE_KEY) → OffsetNotFound. `[spec: FR-008, US6-AS2]` `[code: lib.rs:223-255, region.rs:121-141]`
- **EM-REMOVE-OFFSET-ROUTING** — invariant — the owning region is found by `(offset − data_start)/region_bytes` and the owning slab by `BTreeMap::range(..=offset).next_back()`; the resolved slab must contain `offset`. `[spec: FR-012]` `[code: lib.rs:223-255, region.rs:121-127]` *(|methods| = 1; O(log n) complexity claim is not verifiable)*

### checkpoint (5)
- **EM-CKPT-DURABLE** — postcondition — serializes each region's slab descriptors + complete key vectors into a CRC32-protected blob, writes it to the **inactive** checkpoint copy, then updates the superblock to switch the active pointer and bump `checkpoint_seq`. `[spec: FR-013, US2-AS1, SC-002]` `[code: checkpoint.rs:35-115]`
- **EM-CKPT-SKIP-CLEAN** — postcondition — if no region is dirty since the last checkpoint, `checkpoint()` completes successfully without any device write. `[spec: FR-014, US2-AS3]` `[code: lib.rs:276-286]`
- **EM-CKPT-COALESCE** — invariant (concurrency) — concurrent `checkpoint()` calls are coalesced; at most one checkpoint I/O is in flight; a burst is satisfied by ≤ 2 sequential I/O rounds. `[spec: FR-015, US2-AS4, SC-006]` `[code: lib.rs:729-761]` *(concurrency lane — see F-4)*
- **EM-CKPT-FLUSH** — postcondition (feature-gated) — with `volatile_write_cache` enabled, an explicit `flush()` is issued to the metadata device after checkpoint + superblock writes. `[spec: FR-030]` `[code: checkpoint.rs:102-103, lib.rs:309-310]` *(feature `volatile_write_cache`)*
- **EM-CKPT-HOOK** — postcondition — a registered post-checkpoint hook fires exactly once, synchronously, after a successful checkpoint (after the superblock update, before return). `[spec: FR-037]` `[code: lib.rs:365-367]` *(registration method `set_post_checkpoint_hook` is concrete-only — F-3)*

### get_instance_id (1)
- **EM-INSTANCEID-RETURNS** — postcondition — returns the `instance_id` stored in the superblock. `[spec: FR-026]` `[code: lib.rs:686-692]`

### set_checkpoint_interval (1)
- **EM-CKPTINTERVAL-SET** — postcondition — `Some(dur)` sets the background checkpoint interval; `None` disables the background timer entirely (default 30 s). `[spec: FR-016, FR-027]` `[code: lib.rs:694-696, 69-79]`

### used_bytes (1)
- **EM-USEDBYTES-BUDDY-GRANULARITY** — postcondition — returns `Σ_regions (buddy.total_usable_size − buddy.total_free)`, i.e. slab-level buddy allocation granularity (not per-extent byte sums). `[spec: FR-031]` `[code: lib.rs:698-708]`

### capacity_bytes (1)
- **EM-CAPACITY-USABLE** — postcondition — returns usable data capacity `= Σ_regions buddy.total_usable_size` = data device size minus any in-device metadata region reserved at format; equals full data capacity in separate-device mode. `[spec: FR-032]` `[code: lib.rs:710-715]`

### set_metadata_base_lba (1)
- **EM-METABASE-SHIFTS-IO** — postcondition — the stored `metadata_base_lba` shifts all subsequent metadata-device I/O (superblock + checkpoint regions) by that LBA offset; default 0. `[spec: FR-036]` `[code: lib.rs:717-719, 200-208]`

*(set_data_base_lba and data_base_lba have 0 unique obligations — their entire contract is the
global `EM-DATABASE-ROUNDTRIP`.)*

---

## Reconciliation flags (surfaced, not smoothed)

- **F-1 — `checkpoint_interval` doc divergence (self-flagged by spec FR-016).** The `IExtentManager`
  interface doc-comment (`iextent_manager.rs:200-206`) and the component `README.md` say "five
  minutes"; the actual default set in `ExtentManager::new_inner` is **30 seconds** (`lib.rs:112`).
  The *property* (`EM-CKPTINTERVAL-SET`) is stated at the true 30 s; the stale doc strings are a
  documentation defect to fix, not a behavior gap.
- **F-2 — Six code-only obligations the spec omits.** `EM-FORMAT-NO-DATA-SPACE`,
  `EM-FORMAT-META-NOT-CONNECTED`, `EM-RECOVER-BOTH-CORRUPT`, `EM-RECOVER-FRESH-EMPTY`,
  `EM-RESERVE-OVERSIZE`, and the derived `EM-USED-LE-CAP`. Behavior is reasonable; recommend the
  spec confirm/record them.
- **F-3 — Concrete-only surface (excluded from N).** `set_metadata_ns_id` (FR-028), `set_dma_alloc`
  (FR-029), and `set_post_checkpoint_hook` (FR-037) are on the concrete `ExtentManager` struct but
  **not** on the `IExtentManager` trait. `WriteHandle::publish`/`abort`/`key`/`extent_offset`/
  `extent_size` are `WriteHandle` methods, not trait methods. The two-phase-commit obligations
  (`EM-PUBLISH-*`, `EM-ABORT-RELEASES`, `EM-DROP-AUTO-ABORT`) are the observable contract of
  `reserve_extent` (US1 is framed entirely around it) and are bundled there; the hook behavior is
  captured as `EM-CKPT-HOOK` on `checkpoint`.
- **F-4 — Concurrency-lane properties.** `EM-CKPT-COALESCE` (FR-015/SC-006) and User Story 4
  (8-thread/8-region 800-extent count, concurrent reserve/abort, concurrent removes; SC-004) are
  concurrency obligations not dischargeable in a single-threaded deductive lane. Flagged; carry to a
  concurrency/model-checking lane, not padded into the sequential bundles.
- **F-5 — Feature-gated property.** `EM-CKPT-FLUSH` exists only under `--features
  volatile_write_cache`.
- **F-6 — `EM-INIT-GATE` manifests three ways** (refinement, not divergence): `Err(NotInitialized)`
  for reserve/remove/checkpoint/get_instance_id; empty `Vec`/`0` for get_extents/used_bytes/
  capacity_bytes; silent no-op for for_each_extent. One obligation, method-specific shape.
- **F-7 — Two independent routing schemes must agree (refinement gap to confirm).**
  `reserve_extent` routes by `key & (region_count − 1)` (`EM-SHARD-BY-KEY`); `remove_extent` routes
  by `(offset − data_start)/region_bytes` (`EM-REMOVE-OFFSET-ROUTING`). Correctness of remove-after-
  reserve requires that a slot placed in region *r* by key sharding falls within region *r*'s
  contiguous byte range — a cross-method consistency obligation implicit in the design. Flag for the
  verify step to confirm (not seeded as an id).
- **F-8 — US6-AS5 "data intact" is partly out of scope.** The extent manager guarantees only that
  the disk slot is not reallocated before checkpoint (`EM-DEFERRED-FREE`); it performs **no**
  data-device I/O (FR-036), so on-disk *data-content* integrity after crash is the caller/dispatcher's
  responsibility, not an em obligation.

## Not verifiable (out of every proof lane — assumptions/environment)
Send + Sync composition and concurrent-use safety (FR-024, not compile-enforced); RwLock
interleavings / lock-contention; background-thread timing and firing (FR-016); graceful `Drop` /
thread shutdown (edge case); raw-pointer / DMA / SPDK hugepage (`DmaBuffer`) allocation; `/dev/urandom`
instance-id generation; O(1)/O(log n) complexity claims (FR-012, FR-021, SC-005); `~100M`-extent
scale target (SC-005); fault-injection test infrastructure (FR-034); block-device sector-atomic-write
assumption; crash-during-checkpoint corrupts-only-inactive-copy assumption. (These are the
"assumed / trusted" boundary, not bundle members.)

---

## Coverage ledgers (nothing unaccounted)

### Method ledger (all 14 have a non-empty bundle)
reserve_extent (13), format (9), initialize (7), checkpoint (7), remove_extent (6), get_extents (5),
for_each_extent (4), used_bytes (3), capacity_bytes (3), get_instance_id (2), set_checkpoint_interval
(1), set_metadata_base_lba (1), set_data_base_lba (1), data_base_lba (1). No empty bundles.

### Spec ledger (every FR / US / edge → ≥1 id or a reason)
- FR-001 → *Not verifiable* (component wiring/receptacles).
- FR-002 → EM-FORMAT-VALIDATE-SECTOR / -SLABMULT / -MAXEXT / -REGPOW2 / -META-TOOSMALL.
- FR-003 → EM-FORMAT-WRITES-SUPERBLOCK. FR-035 → EM-FORMAT-DATA-LAYOUT.
- FR-004 → EM-RECOVER-ROUNDTRIP + EM-INIT-VALIDATE-MAGIC + EM-INIT-VALIDATE-CRC.
- FR-005 → EM-RESERVE-INVISIBLE-UNTIL-PUBLISH + EM-SECTOR-ALIGN + EM-RESERVE-OUTOFSPACE.
- FR-006 → EM-PUBLISH-VISIBLE + EM-PUBLISH-FREEKEY-DISCARD. FR-007 → EM-ABORT-RELEASES.
- FR-008 → EM-REMOVE-HIDES + EM-REMOVE-NOTFOUND + EM-DEFERRED-FREE.
- FR-009 → EM-KEYVEC-MEMBERSHIP + EM-GETEXTENTS-EXACT + EM-FOREACH-COUNT.
- FR-010, FR-011 → collapsed into EM-KEYVEC-MEMBERSHIP. FR-012 → EM-REMOVE-OFFSET-ROUTING (complexity part *Not verifiable*).
- FR-013 → EM-CKPT-DURABLE. FR-014 → EM-CKPT-SKIP-CLEAN. FR-015 → EM-CKPT-COALESCE (concurrency).
- FR-016 → EM-CKPTINTERVAL-SET (timing *Not verifiable*; doc divergence F-1). FR-017 → EM-RECOVER-DUALCOPY.
- FR-018 → EM-RECOVER-ROUNDTRIP + EM-RECOVER-LAST-CKPT-CONSISTENCY. FR-037 → EM-CKPT-HOOK.
- FR-019, FR-020, FR-021 → collapsed into EM-RESERVE-SIZECLASS (complexity *Not verifiable*). FR-022 → EM-SHARD-BY-KEY.
- FR-023, FR-024 → *Not verifiable* (concurrency / Send+Sync). FR-025 → EM-DEFERRED-FREE.
- FR-026 → EM-INSTANCEID-RETURNS. FR-027 → EM-CKPTINTERVAL-SET. FR-028, FR-029 → concrete-only (F-3).
- FR-030 → EM-CKPT-FLUSH (feature). FR-031 → EM-USEDBYTES-BUDDY-GRANULARITY. FR-032 → EM-CAPACITY-USABLE.
- FR-033 → EM-PUBLISH-VISIBLE / EM-ABORT-RELEASES / EM-DROP-AUTO-ABORT. FR-034 → *Not verifiable* (test infra).
- FR-036 → EM-METABASE-SHIFTS-IO + EM-DATABASE-ROUNDTRIP.
- US1 → reserve/publish/abort/drop/freekey ids. US2 → EM-CKPT-DURABLE / EM-RECOVER-ROUNDTRIP / EM-RECOVER-LAST-CKPT-CONSISTENCY / EM-CKPT-SKIP-CLEAN / EM-CKPT-COALESCE.
- US3 → EM-RECOVER-DUALCOPY / EM-INIT-VALIDATE-MAGIC / EM-INIT-VALIDATE-CRC. US4 → concurrency (F-4).
- US5 → EM-GETEXTENTS-EXACT / EM-GETEXTENTS-EMPTY-FRESH / EM-RESERVE-INVISIBLE-UNTIL-PUBLISH / EM-FOREACH-COUNT.
- US6 → EM-REMOVE-HIDES / EM-REMOVE-NOTFOUND / EM-DEFERRED-FREE (AS5 data-content → F-8).
- SC-001..003 → lifecycle + recovery ids. SC-004 (concurrency), SC-005 (scale) → *Not verifiable*. SC-006 → EM-CKPT-COALESCE.
- Edge cases: full→EM-RESERVE-OUTOFSPACE; key 0→valid (covered by KEYVEC-MEMBERSHIP, no special-case id); key u64::MAX→EM-PUBLISH-FREEKEY-DISCARD; invalid format params→EM-FORMAT-VALIDATE-*; meta too small→EM-FORMAT-META-TOOSMALL; ops before init→EM-INIT-GATE; drop w/ outstanding handles→*Not verifiable* (no-panic/thread shutdown); multiple size classes→EM-RESERVE-SIZECLASS; remove+realloc+crash→EM-DEFERRED-FREE + EM-RECOVER-ROUNDTRIP.

### Code ledger (every public fn / error branch / transition → ≥1 id or reason)
All 14 trait fns covered above. Error returns: CorruptMetadata (format validation + superblock/
checkpoint CRC/magic/truncation), NotInitialized (init-gate + not-connected), OffsetNotFound
(remove routing), OutOfSpace (reserve) — all mapped. Internal fns (`region.rs`, `slab.rs`, `buddy.rs`,
`bitmap.rs`, `checkpoint.rs`, `recovery.rs`, `superblock.rs`) collapse to observable ids via the
implementing-obligation map below.

---

## Implementing-obligation map (for the verify step — NOT extra bundle members)

| observable property | implementing obligations (internals) |
|---|---|
| EM-KEYVEC-MEMBERSHIP | slab dense `Vec<u64>` parallel to bitmap (FR-010), FREE_KEY sentinel (FR-011), `get_key`/`set_key`/`free_slot` (slab.rs:37-48) |
| EM-SECTOR-ALIGN | `align_to_sector_size` (region.rs:37-39), `slot_offset = start + idx·element_size` (slab.rs:58-60), reserve `aligned_size` rounding (lib.rs:591) |
| EM-RECOVER-ROUNDTRIP | `slab_from_descriptor` bitmap-derived-from-keys / BITMAP-REFLECTS-KEYS (recovery.rs:76-85), `buddy.mark_allocated` (buddy.rs:117-157), `deserialize_slabs` (checkpoint.rs:183-241), `size_classes.add_slab` |
| EM-DEFERRED-FREE | `pending_frees` push in `remove_extent_by_offset` (region.rs:147), `flush_pending_frees` on checkpoint (region.rs:152-160, lib.rs:318), `free_slot` empties→buddy (region.rs:94-112) |
| EM-CKPT-DURABLE | `write_checkpoint` inactive-copy write + CRC32 + superblock `active_copy` switch + `seq+1` (checkpoint.rs:35-115), `superblock.serialize` (superblock.rs:58-97) |
| EM-USED-LE-CAP / EM-USEDBYTES / EM-CAPACITY | `buddy.total_free`, `buddy.total_usable_size` (buddy.rs:53-55,159-166), buddy coalesce / no-adjacent-free (buddy.rs:84-115) |
| EM-RESERVE-SIZECLASS | `SizeClassManager` non-full-list invariant (slab.rs:91-121, region.rs:47-64), `buddy.alloc` slab-sized chunk (buddy.rs:57-82), slab bitmap rover (slab.rs:29-35, bitmap.rs:42-51) |
| EM-RECOVER-DUALCOPY | `recovery.recover` active-first + inactive fallback (recovery.rs:29-66), `read_checkpoint_region` seq + CRC32 checks (checkpoint.rs:117-173) |
| EM-REMOVE-OFFSET-ROUTING | `region_for_offset` byte-range division (lib.rs:223-255), `BTreeMap::range(..=offset).next_back()` + `contains_offset` + `slot_for_offset` (region.rs:121-141, slab.rs:62-88) |

Internal-only invariants collapsed above (not separate ids): BITMAP-REFLECTS-KEYS (recovery),
`AllocationBitmap` no-double-set/no-double-clear (bitmap.rs:19,28 debug_asserts), SizeClass-only-non-full,
buddy-coalesce, slab-empty-returns-to-buddy, CRC32-integrity (superblock + checkpoint).
</content>
</invoke>
