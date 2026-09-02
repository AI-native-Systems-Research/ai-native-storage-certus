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
