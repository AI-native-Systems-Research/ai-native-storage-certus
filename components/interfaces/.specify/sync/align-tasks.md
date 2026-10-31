# Align Tasks — `interfaces`

**Regenerated**: 2026-08-20 (Spec-Sync Phase B)
**Policy**: `.specify/sync/PHASE_B_POLICY.md`

## No ALIGN tasks this pass

The current drift report (`.specify/sync/drift-report.json`) contains 2 drifted
requirements — **FR-014** and **FR-025** — and 0 other drift. Per the Phase B
per-component note for `interfaces`, both were an already-fixed defect and are classified
**RESOLVED** (see `proposals.md` / `apply-report.md`), not ALIGN. There are therefore no
code-side alignment tasks to open.

## Superseded (previously listed, now RESOLVED)

Earlier sync passes recorded two code-side ALIGN tasks against FR-014 (wire
`mod iextended_metadata_store;` + re-exports into `src/lib.rs`; add
`extended-metadata-store` to the root workspace). The first of these is **DONE on the main
development thread** and verified in the current source:

- `src/lib.rs:77` — `mod iextended_metadata_store;` (ungated)
- `src/lib.rs:99` — `pub use iextended_metadata_store::{ExtendedMetadataStoreError, IExtendedMetadataStore};` (ungated)

`IExtendedMetadataStore` and `ExtendedMetadataStoreError` are now part of the compiled,
exported `interfaces` API, so FR-014/FR-025 no longer drift. (Bringing the
`extended-metadata-store` crate into the root workspace is tracked under that component's
own sync, not `interfaces`.)

---

## Open items from the 2026-10-21 run (branch `fix/dispatcher-remove-clear-mt`)

First recorded by the dispatcher apply of 2026-10-21. Confirmed and completed by the
interfaces apply of 2026-10-21T22:58:04Z (approved by Cornel; see `apply-report.md`).

### DONE interfaces-5 / interfaces-6 — `IDispatcher` doc comments (approved ALIGN, applied by hand)

These two approved ALIGN proposals changed only doc comments in
`components/interfaces/src/idispatcher.rs`. They were applied by hand before this apply
(`git diff -- components/interfaces/src/idispatcher.rs`). The apply did not edit `src/`. It
checked that the working-tree text matches the proposed text word for word.

- **interfaces-5** (FR-008 `clear_memory_tier`): the doc now reads "Empty the memory tier …".
  Pinned entries (lookup, client `pin`, or the pending write-through) keep their slot and are
  retried. The count covers entries actually demoted or removed, and the doc gains a
  `# Errors` section for `Timeout`.
- **interfaces-6** (FR-008 `batch_lookup`): "Returns one [`LookupOutcome`] per input entry, in
  the same order: the result together with how the key was served (`served_by`)."

#### Acceptance Criteria
- [x] Doc text matches proposals 5 and 6.
- [ ] `cargo doc -p interfaces --no-deps --features spdk` is warning-free. *Not run by this apply (it only edits specs and sync files). Run it before merging.*

### OPEN interfaces-7 — FR-008 `IDispatcher` `remove` / `clear_memory_tier` / `flush_to_ssd` contract, in component **`dispatcher-p2p`** (major, HIGH confidence)

> Approval note: *"TRACK (Cornel 2026-10-21): port the 3 dispatcher repairs to dispatcher-p2p in a separate PR with its own Loom tests and spec sync."*

The work is in component `dispatcher-p2p`, not in `interfaces`; it is recorded here because
the contract it violates is the `IDispatcher` doc in this crate.

**Spec Requirement**: `src/idispatcher.rs` (working tree): the `remove` doc ending at `:647` (`remove` does not wait for the write-through), `:807-834` (`clear_memory_tier` must not free a pinned entry's slot; `Timeout`), `:836-850` (`flush_to_ssd` returns `IoError` on a failed write-through).
**Current Code** (`components/dispatcher-p2p/src/lib.rs`): `remove` maps the write-through pin's `ActiveReferences` to `KeyNotFound` (`:2380-2381`, pin from `:2609`); `clear_memory_tier` frees the slots of `ActiveReferences` entries and counts every candidate (`:2833-2864`); `flush_to_ssd` returns the pre-flush `in_flight()` and never `IoError` (`:2889-2902`).
**Required Change**: a separate `dispatcher-p2p` repair porting the dispatcher's `WriteThroughPins` ledger (`components/dispatcher/src/background.rs:250-389`), the pin-preserving `clear_memory_tier` with a bounded `Timeout` (`components/dispatcher/src/lib.rs:3940-4004`) and the failed-write-aware `flush_to_ssd` (`components/dispatcher/src/lib.rs:4006-4035`).
**Files to Modify**: `components/dispatcher-p2p/src/lib.rs` (+ background module, new tests), `components/dispatcher-p2p/specs/**` in its own sync
**Estimated Effort**: large

#### Acceptance Criteria
- [ ] The three dispatcher `repair_test_*.rs` Loom tests, ported as failing twins, fail on the current `dispatcher-p2p` and pass after the port.
- [ ] `dispatcher-p2p` spec sync run in the same PR.

### OPEN interfaces-14 — NFR-004 doc comments with runnable examples (moderate, MEDIUM confidence)

> Approval note: *"TRACK (Cornel 2026-10-21): runnable doc examples, later doc task."*

Not approved in this run. Recorded so it can be picked up as a later documentation task. The
BACKFILL alternative in `proposals.md` (soften NFR-004) was **not** chosen, so NFR-004 stays
as written in the spec.

**Spec Requirement**: NFR-004 (`specs/001-interfaces/spec.md`, "All public types and methods have doc comments with runnable examples."), and the same rule in the root `CLAUDE.md`.
**Current Code**: `src/idispatch_map.rs`, `src/imemory_tier.rs`, `src/ieviction_policy.rs`, `src/iextent_manager.rs` and `src/ipartition_table.rs` have no doc examples. These public items have no docs: `MemoryTierTelemetrySnapshot` fields (`src/imemory_tier.rs:13-15`), `ExtentManagerError` variants (`src/iextent_manager.rs:20-24`), `Extent` fields (`:12-14`), `FormatParams::new` (`:70`), and `WriteHandle` with `key`/`extent_offset`/`extent_size`/`publish`/`abort` (`:95-146`). `missing_docs` is not enforced in `src/lib.rs`.
**Required Change**: Add doc comments to the listed items and one example per trait in the five modules. Type-level examples can be runnable; trait-method examples should be `no_run`, as in `idispatcher.rs`. Optionally add `#![warn(missing_docs)]` to `src/lib.rs`.
**Files to Modify**: the five `src/*.rs` modules above; optionally `src/lib.rs`
**Estimated Effort**: medium

#### Acceptance Criteria
- [ ] Every public item in the five modules has a doc comment.
- [ ] Each trait in the five modules has at least one example. `cargo test --doc -p interfaces` passes, with and without `--features spdk`.
- [ ] `cargo doc -p interfaces --no-deps --features spdk` is warning-free.

---

## Open item from the 2026-10-30 run (branch `specsync-a`)

### OPEN interfaces-P4 — SC-4 rustdoc warnings (minor, pre-existing, not approved)

**Spec Requirement**: SC-4 / NFR-004, `cargo doc --no-deps` is warning-free.
**Current Code**: `cargo doc -p interfaces --no-deps` (default features) reports 3 unresolved intra-doc links: [`IDispatcher::tier_event_stats`] at `src/idispatcher.rs:339` (the `IDispatcher` trait is `spdk`-gated, so the link does not resolve in a default build), [`create_stream`] at `src/igpu_services.rs:587` and [`stream_synchronize`] at `:607` (inside the trait; need `Self::`). All three exist at `a0292be6`; this branch did not introduce them.
**Required Change**: qualify the two `IGpuServices` links as `Self::…`; for the `IDispatcher` link, use plain code text or a `cfg_attr`-conditional link.
**Files to Modify**: `components/interfaces/src/idispatcher.rs`, `components/interfaces/src/igpu_services.rs`
**Estimated Effort**: small

#### Acceptance Criteria
- [ ] `cargo doc -p interfaces --no-deps` and `--features spdk` are warning-free.
