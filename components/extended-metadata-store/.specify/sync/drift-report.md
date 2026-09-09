---
spec_sync_component: extended-metadata-store
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-09T23:14:09Z
spec_sync_git_commit: 8f6a5b40
spec_sync_inputs_sha256: 6b39fa69d44019b99e12fd297e6866c9ac6cdf81a41d33c73bc8a138754c52eb
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Re-stamp 2026-09-09 (merged-branch interfaces-fold; no drift).** Branch
> `fix-dispatcher-store-backpressure` was merged into `unstable` at `3411518a`; that
> branch adds `DispatcherConfig::store_backpressure_ms` plus two `TierEventStats`
> counters (`store_backpressure_events`, `store_drops_on_full`) to
> `components/interfaces/src/idispatcher.rs`. `scripts/spec-sync-hash.sh` folds
> the whole `components/interfaces/` tree into every component's hash, so this
> component's digest moved even though its own `src/`+`specs/` are byte-for-byte
> unchanged and it references none of those new dispatcher symbols (verified by
> grep across `components/` and `lib/`). The merge's conflict resolution had
> reverted this stamp to unstable's `b220a1c8` value; the digest is recomputed
> here at merged HEAD. Drift status remains `clean`; the report body stands
> verbatim.
>
> **Correction 2026-09-09T23:14Z (clean-tree digest; branch linearized).** The
> 22:27 re-stamp above was computed in a working tree that carried untracked,
> uncommitted files under this component's `specs/` — the speckit scratch docs
> (`data-model.md`, `research.md`, `quickstart.md`, `contracts/`, `checklists/`)
> and an entire local `002-ssd-integration-test/` spec — none of which are
> tracked on this branch. `scripts/spec-sync-hash.sh` walks files on disk, so
> those strays inflated the local digest and CI's clean checkout recomputed a
> different one (same failure class as the 2026-09-03 `iipc.rs` note below). The
> digest is now recomputed in a pristine `git worktree` at the branch tip over
> the **tracked** inputs only (`specs/001-extended-metadata-store/{spec,plan,tasks}.md`
> + `components/interfaces`), giving `6b39fa69…`, which is exactly what CI hashes.
> The branch was also rebased onto `origin/unstable` (merge commit removed), so
> `spec_sync_git_commit` now points at the linear tip `8f6a5b40`. No `src/`,
> `specs/`, or report-body content changed; drift status remains `clean`.

# Spec ↔ Implementation Drift Report — extended-metadata-store

> **Digest refreshed 2026-09-03 (spec-sync gate fix).** The earlier
> `spec_sync_inputs_sha256` was computed in a working tree that contained an
> untracked `components/interfaces/src/iipc.rs` — a local file not part of this
> branch — which `scripts/spec-sync-hash.sh` folds into every component's
> interface hash, so CI's clean checkout recomputed a different digest. The
> stray file was removed and the digest recomputed in a clean tree. No `src/`,
> `specs/`, or report content changed; drift status remains `clean`.

**Generated**: 2026-09-03
**Mode**: Read-only drift analysis. No code or spec behavior changes applied this
sweep; the only edit was recording the confirmation date in the spec's
`Last-Synced` line. One open ALIGN item is documented and parked by maintainer
decision.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (`001-extended-metadata-store`) |
| Requirements Checked | 18 FR + 11 NFR + 7 SC |
| Aligned | 36 |
| Drifted | 0 actionable |
| Not Implemented | 0 |
| Unspecced | 0 |
| Parked (open ALIGN, documented) | 1 |

This report supersedes the earlier stale artifact (which read "Generated:
pending", "Specs Analyzed: 2", and referenced a non-existent `002` spec). The
2026-08-20 Phase B backfill had already rewritten the spec to match the shipped
implementation; this sweep re-verifies that alignment against the current tree
and finds it holds. The `002` spec never existed — that reference was an
artifact of the stale report and is not re-introduced.

## Spec: 001-extended-metadata-store

### Aligned ✓ (verified this sweep)

| Req | Evidence |
|-----|----------|
| FR-01 put(key, value) stores bytes | `src/lib.rs:158-170` |
| FR-02 get(key) returns stored value | `src/lib.rs` get path; unit tests in `mod tests` |
| FR-03 delete(key) removes entry | `src/lib.rs`; covered by lib unit tests |
| FR-04 in-memory default mode (no device) | default build compiles without `testing`/`spdk`; 9 lib unit tests green |
| FR-05 force_flush() invokes durable flush trigger when attached, else no-op | `src/lib.rs:201-215` (`force_flush`), `:111` (`attach_flush_trigger`), `:68` (`FlushTrigger` alias) |
| FR-06 ValueTooLarge enforced at put() (128 KiB max) | `src/lib.rs:158-170` returns `ValueTooLarge`; only enforced error at put time |
| FR-07..FR-18 persistence/format/CRC/ping-pong behaviors | `src/flush.rs`, `src/on_disk.rs`; exercised by `--features testing` persistence suite (19 tests) |
| NFR-01..NFR-11 | dual-region ping-pong flush, CRC32 on-disk integrity, feature-gating (`testing = ["interfaces/spdk"]`), workspace membership (`Cargo.toml:23`, dep `:105`) |
| SC-1 9 unit tests in `src/lib.rs` | `mod tests` in `src/lib.rs` (default `cargo test` reports 15 lib unit = 9 in lib.rs + 6 always-compiled in `on_disk.rs`) |
| SC-2..SC-7 | MockBlockDevice-backed round-trip, crash-consistency, capacity accounting, CRC detection — all present and passing |

**Verification runs this sweep** (all green):
- default `cargo test -p extended-metadata-store` — 15 lib unit tests
- `cargo test -p extended-metadata-store --features testing` — 19 persistence tests
- `cargo clippy -p extended-metadata-store` — no warnings
- `cargo doc -p extended-metadata-store --no-deps` — warning-free

### Phase B blockers — confirmed resolved

The 2026-08-20 backfill claimed Phase B had cleared the MockBlockDevice and
workspace-membership blockers. Both claims verified true in the current tree:
- `MockBlockDevice impl IBlockDevice` — `src/test_support.rs:171`; stats accessor
  `read_write_stats` — `src/test_support.rs:223`; state helper
  `create_test_component_from_state` — `src/test_support.rs:272`.
- Workspace membership — `Cargo.toml:23` (member) and `Cargo.toml:105`
  (workspace dependency).

### Parked (open ALIGN — documented, no change this sweep)

- **`CapacityExhausted` is defined but never constructed.** The interface variant
  `ExtendedMetadataStoreError::CapacityExhausted`
  (`components/interfaces/src/iextended_metadata_store.rs:12`) is not produced by
  any `src/` path. `put()` enforces only `ValueTooLarge` (`src/lib.rs:158-170`);
  region-capacity overflow is surfaced at flush time as a `String`
  ("exceeds region capacity", `src/flush.rs:34-38`), not as the typed
  `CapacityExhausted` variant.
  - **Consequence:** the hardware integration test `test_capacity_exhaustion`
    (`tests/integration_ssd.rs:513-547`) is **vacuous** — its match arm expecting
    `CapacityExhausted` (`:530`) can never be taken, because `put()` never returns
    it. The corresponding persistence test `capacity_exhaustion_detected`
    (T051, `tests/persistence.rs:627`) is meaningful: it drives the flush-time
    `String` error and passes.
  - **Resolution class:** ALIGN (either construct `CapacityExhausted` on the
    capacity path and fix the test, or remove the dead variant and rewrite the
    test around the flush-time error).
  - **Decision (2026-09-03, maintainer):** keep parked and documented. No code
    change this sweep. Because it is a defined-but-unconstructed variant plus a
    non-exercised hardware-only test — not a behavioral discrepancy between spec
    and shipped behavior — it does not constitute actionable spec/impl drift, and
    the report is stamped `clean`. Revisit when the capacity error path is next
    touched.

### Not Implemented ✗

None.

## Unspecced Features

None. All implemented surface is captured in the spec.

## Recommendations

- When the capacity error path is next revisited, resolve the parked ALIGN item
  above (construct the typed variant + de-vacuum `integration_ssd.rs:513-547`, or
  retire the variant and rewrite the test).
- Commit this stamped `drift-report.md` together with the spec edit so the CI
  Spec-Sync Gate sees a fresh report whose input hash matches the tree.
