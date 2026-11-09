# Drift Resolution Proposals — gpu-services

Generated: 2026-11-08T12:00:00Z · Branch: `fix/dispatcher-j33-repairs` (HEAD `d45ab524d`) · Based on: `drift-report.json` of the same run (delta `0eb3d3955..HEAD`, one commit `d45ab524d`, review of PR #591).

Mode: batch. **No proposal is approved** (`approved: false` on all). Apply was not run. No spec, source or interface file was edited.

## Summary

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 2: P-6 (component spec); P-5 (interface doc, carried over from 2026-11-07, **touches `components/interfaces`**) |
| Align (Spec → Code) | 0 |
| Human Decision | 0 (observations below, no proposal) |
| New Specs | 0 |
| Remove from Spec | 0 |

| Id | Requirement | Type | Approved | Edits |
|---|---|---|---|---|
| P-6 | 001 FR-016 | BACKFILL | false | `components/gpu-services/specs/001-gpu-cuda-services/spec.md` only |
| P-5 | `IGpuServices::register_host_memory` / `unregister_host_memory` doc comments | BACKFILL (**interfaces**) | false | `components/interfaces/src/igpu_services.rs`. **Re-stamps every component.** Optional; recommend deferring (Cornel, 2026-11-07: "bundle with the next interfaces change") |

**Is a BACKFILL needed? Yes, one: P-6.**
- With P-6 applied, the component is `clean`, and P-6 edits only `components/gpu-services/specs/`.
- `d45ab524d` adds **no new interface drift**:
  - `register_host_memory` `# Errors` ("Returns an error if CUDA host registration or SPDK registration fails", `igpu_services.rs:802`) is still true.
  - `unregister_host_memory` ("Calls `spdk_mem_unregister` then `cudaHostUnregister`", `:808-809`) has the right order. It now holds even on SPDK failure, which it did not before `d45ab524d`.
- The only interface gap left is the conditional SPDK step from `9a79224a1`, which P-5 already covers.
- 002 FR-019 / FR-020 and 001 FR-015 need nothing (see the drift report).

Previous proposals (2026-11-07, P-1…P-5) are in git: `git show 0eb3d3955:components/gpu-services/.specify/sync/proposals.md`. P-1…P-4 were applied on 2026-11-07. P-5 is still open (see `align-tasks.md:202`).

---

### P-6: 001-gpu-cuda-services / FR-016

**Type**: BACKFILL · **Confidence**: HIGH · **approved: false** · touches interfaces: no

**Requirement**: FR-016, `unregister_host_memory`.

**Spec says** (`specs/001-gpu-cuda-services/spec.md:342-343`, inside FR-016 at `:336-348`):
> "If `spdk_mem_unregister` fails, the method returns the error without calling `cudaHostUnregister`."

The provenance note at `:347-348` also cites `src/lib.rs:1220-1225`, which has moved.

**Code does** (`src/lib.rs:1198-1244`, after `d45ab524d`):
1. When `interfaces::is_spdk_env_active()` is true, it calls `spdk_mem_unregister(ptr, size)`. A non-zero rc becomes `spdk_err = Some("spdk_mem_unregister failed (rc=…)")` and is **not** returned yet (`:1223-1228`).
2. It **always** calls `cudaHostUnregister(ptr)` (`:1231`).
3. If the CUDA call fails, it returns `Err("cudaHostUnregister failed: …")`, and the SPDK error, if any, is dropped (`:1232-1237`).
4. Otherwise it returns the SPDK error if there was one, else `Ok(())` (`:1239-1242`).

The reason is in the comment at `:1218-1222`: if SPDK became active only after the region was registered, the SPDK step fails, and an early return would leak the CUDA pin.

The spec sentence now says the opposite of the code.

**Old text** (exact, `spec.md:336-348`):
```
- **FR-016**: Component MUST provide an `unregister_host_memory(ptr,
  size)` method (gated behind `spdk` feature) that, when an SPDK
  environment is active, unregisters memory from SPDK via
  `spdk_mem_unregister`, then removes page-locking via
  `cudaHostUnregister`. When no SPDK environment is active
  (`interfaces::is_spdk_env_active()` is false at call time) the SPDK step
  is skipped and only `cudaHostUnregister` runs. If `spdk_mem_unregister`
  fails, the method returns the error without calling `cudaHostUnregister`.
  MUST be called before freeing the underlying allocation. Callers SHOULD
  register and unregister under the same SPDK-environment state (e.g. in
  `initialize` / `shutdown`); the flag is read independently by each call.
  *(Backfilled 2026-11-07 from commit `9a79224a1`, A-14;
  `src/lib.rs:1220-1225`.)*
```
**New text**:
```
- **FR-016**: Component MUST provide an `unregister_host_memory(ptr,
  size)` method (gated behind `spdk` feature) that, when an SPDK
  environment is active, unregisters memory from SPDK via
  `spdk_mem_unregister`, then removes page-locking via
  `cudaHostUnregister`. When no SPDK environment is active
  (`interfaces::is_spdk_env_active()` is false at call time) the SPDK step
  is skipped and only `cudaHostUnregister` runs. If `spdk_mem_unregister`
  fails, the method MUST still call `cudaHostUnregister` (so the CUDA pin
  is not leaked, e.g. when SPDK became active only after the region was
  registered) and then return the SPDK error; if `cudaHostUnregister`
  fails, the method returns the CUDA error. After either error the region
  is no longer CUDA-pinned whenever `cudaHostUnregister` succeeded.
  MUST be called before freeing the underlying allocation. Callers SHOULD
  register and unregister under the same SPDK-environment state (e.g. in
  `initialize` / `shutdown`); the flag is read independently by each call.
  *(Backfilled 2026-11-07 from commit `9a79224a1`, A-14;
  `src/lib.rs:1220-1225`.)* *(Backfilled 2026-11-08 from commit
  `d45ab524d`, review of PR #591: the CUDA step runs even when the SPDK
  step fails; `src/lib.rs:1218-1242`.)*
```
Also add a header line under the existing ones at `spec.md:6-8`:
```
**Last-Synced**: 2026-11-08 (spec-sync, branch `fix/dispatcher-j33-repairs`: BACKFILL FR-016 — `unregister_host_memory` calls `cudaHostUnregister` even when `spdk_mem_unregister` fails, then returns the SPDK error, commit `d45ab524d`. No source changes.)
```

**Rationale (why BACKFILL, not ALIGN)**:
- The new behaviour comes from a code review (Nara, PR #591) and fixes a real leak. With the old early return, the order "register while SPDK was inactive, unregister after SPDK became active" returned `Err` and left the region page-locked for good, because no other API unpins it (FR-007, `spec.md:244-251`). The 2026-11-07 drift report recorded that order as an observation.
- The caller still sees the SPDK failure as an `Err`, so nothing is hidden.
- The spec sentence was written only a day earlier (P-2, 2026-11-07) by copying the old code. It was not an independent design decision, so it is not authoritative over the review fix.
- Changing the code back would bring the leak back.
- The FR-016 SHOULD stays: the mismatched-state order still yields an `Err`.

**Not covered by a test** (see observations): the SPDK-fails path needs real SPDK and a real GPU. The new text describes the code as read; it has not been executed.

---

### P-5 (carried over, refreshed): `IGpuServices` doc comments — **EDITS `components/interfaces` (re-stamps every component)**

**Type**: BACKFILL (interface doc) · **Confidence**: HIGH on content, LOW priority · **approved: false** · touches interfaces: **yes**

Status: the 2026-11-07 P-5 was not approved ("Cornel 2026-11-07: not in this PR (edits components/interfaces, re-stamps every component); bundle with the next interfaces change."). It is recorded as OPEN in `align-tasks.md:202-208`. It is repeated here only so the bundled interfaces change uses wording that matches `d45ab524d`. **This component is `clean` without P-5.**

**Requirement**: interface contract of `register_host_memory` / `unregister_host_memory`.

**Doc says** (`components/interfaces/src/igpu_services.rs`):
- `:786`: "Register existing host memory as CUDA-pinned and SPDK DMA-capable."
- `:788-790`: "Calls `cudaHostRegister` to page-lock the memory (enabling async GPU DMA) then `spdk_mem_register` so NVMe controllers can DMA directly to/from it."
- `:802`: "Returns an error if CUDA host registration or SPDK registration fails." This is **aligned**: SPDK registration can only fail when it is attempted. No change.
- `:808-809`: "Calls `spdk_mem_unregister` then `cudaHostUnregister`. Must be called before freeing the underlying allocation."
- `:813`: "Returns an error if unregistration fails." This is **aligned**. No change.

**Code does**:
- In both methods the SPDK step runs only if `is_spdk_env_active()` (`components/gpu-services/src/lib.rs:1179-1181`, `:1223`).
- Unregister calls `cudaHostUnregister` regardless of the SPDK result (`:1231`, `:1239-1242`).

**Old text** (exact, `igpu_services.rs:788-790`):
```
        /// Calls `cudaHostRegister` to page-lock the memory (enabling async
        /// GPU DMA) then `spdk_mem_register` so NVMe controllers can DMA
        /// directly to/from it.
```
**New text**:
```
        /// Calls `cudaHostRegister` to page-lock the memory (enabling async
        /// GPU DMA) then, if an SPDK environment is active
        /// ([`crate::is_spdk_env_active`]), `spdk_mem_register` so NVMe
        /// controllers can DMA directly to/from it. Without an SPDK
        /// environment the memory is CUDA-pinned only.
```
**Old text** (exact, `igpu_services.rs:808-809`):
```
        /// Calls `spdk_mem_unregister` then `cudaHostUnregister`. Must be
        /// called before freeing the underlying allocation.
```
**New text** (refreshed for `d45ab524d`):
```
        /// Calls `spdk_mem_unregister` (only if an SPDK environment is
        /// active) then `cudaHostUnregister`; the CUDA step runs even if the
        /// SPDK step fails, and the SPDK error is then returned. Must be
        /// called before freeing the underlying allocation.
```
Optionally change `:786` to "Register existing host memory as CUDA-pinned and, with an SPDK environment, SPDK DMA-capable." Leave `:802` and `:813` unchanged.

Caveats:
- `is_spdk_env_active` is re-exported only under `cfg(feature = "spdk")` (`components/interfaces/src/lib.rs:113`). Both methods are `spdk`-gated too. Check with `cargo doc -p interfaces --no-deps --features spdk`, or use plain backticks instead of the intra-doc link.
- **Editing `components/interfaces` changes every component's spec-sync digest.** Defer and bundle with the next interfaces change. The interfaces spec (`components/interfaces/specs/001-interfaces/spec.md:274-275`) is generic and needs nothing.

---

## Observations (no proposal; for a human)

1. **The GPU test passes vacuously without a GPU.** `tests/host_memory_without_spdk.rs:35-39` returns early with a `SKIP:` line when `initialize()` fails. The parent captures stderr (`:70-79`) and shows it only on failure (`:80-86`), so on CI or a GPU-less host the test is green without checking A-14. That is what `d45ab524d` intends. Only a run on a GPU host (node7) counts as evidence. The FR-015 citation of this test (`spec.md:323-324`) is still fair.
2. **No test covers the new error path** (SPDK unregister fails → CUDA unregister → SPDK `Err`). Covering it needs SPDK active plus a region SPDK never registered, on a GPU host. Worth adding next to `tests/gpu_nvme_p2p.rs` (also `p2p`).
3. **Retry after the SPDK `Err`.** Once the SPDK-failure `Err` is returned, the region is already CUDA-unpinned, so a retry fails in `cudaHostUnregister`. In-tree callers discard the result (`components/dispatcher/src/lib.rs:2424`, `components/dispatcher-p2p/src/lib.rs:1735`). P-6's text states the post-condition.
4. **Carried over:** `allocate_pinned_dma_buffer` → `src/dma.rs:259` calls `spdk_mem_register` without the guard. That matches 001 FR-020, but it has the A-14 crash class without SPDK. A dispatcher-side check is recommended.

## Verification status

Static only: every citation was read at HEAD `d45ab524d`. `cargo test -p gpu-services` was not approved in this session, so it was not run. The GPU test (`--features p2p`, real GPU) was not run either. `d45ab524d` reports dispatcher lib tests only.
