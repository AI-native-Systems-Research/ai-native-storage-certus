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

Recorded by the dispatcher apply of 2026-10-21 (approved by Cornel). Interfaces proposals 5
and 6 (doc comments in `src/idispatcher.rs`) were applied by hand and are not tasks.

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
