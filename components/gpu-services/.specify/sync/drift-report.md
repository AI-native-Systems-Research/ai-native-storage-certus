---
spec_sync_component: gpu-services
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-11-08T00:39:49Z
spec_sync_git_commit: b153ea11e
spec_sync_inputs_sha256: 2f3894e6af01459e77889ddde1e3295937364762b7cfe62eae259147ff4c18ea
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Apply 2026-11-08 (branch fix/dispatcher-j33-repairs, approved by Cornel).** Applied **P-6** (001 FR-016) as BACKFILL: if `spdk_mem_unregister` fails, `unregister_host_memory` still calls `cudaHostUnregister` and then returns the SPDK error; a CUDA failure returns the CUDA error (commit `d45ab524d`, review of PR #591, `src/lib.rs:1218-1242`). The old text occurred exactly once and was replaced with the exact text from `proposals.md`; a provenance note and a 2026-11-08 `Last-Synced` line were added. Backup: `backups/20261109T070901Z/`. **Tasks:** none (no ALIGN proposals). **Open:** **P-5** (`IGpuServices` doc comments in `components/interfaces/src/igpu_services.rs`) is not approved ("Cornel 2026-11-08: not in this PR (edits components/interfaces, re-stamps every component); bundle with the next interfaces change") and is recorded as open in `align-tasks.md` with the refreshed wording. The four observations (vacuous no-GPU test pass, no test for the SPDK-failure path, unpinned region after that `Err`, unguarded `allocate_pinned_dma_buffer` → `src/dma.rs:259`) remain for a human. No `src/` or interfaces file changed; no tests run. Frontmatter not touched. **Status to stamp: `clean`.** Details in `apply-report.md`.

> **Sync 2026-11-08 (branch fix/dispatcher-j33-repairs, HEAD `d45ab524d`) — delta, 1 spec requirement drifted (001 FR-016), BACKFILL proposed as P-6. P-5 (interfaces doc) is still open and is carried forward. Nothing approved, nothing applied. Frontmatter left as stamped; the re-stamp script owns it.** Base: the stamp commit `0eb3d3955` (that stamp covered `9a79224a1`, applied as P-1…P-4 on 2026-11-07). `git log 0eb3d3955..HEAD -- components/gpu-services components/interfaces` shows one commit, `d45ab524d` (review of PR #591). It changes `src/lib.rs` (+12/−6) and `tests/host_memory_without_spdk.rs` (+5/−1). **`components/interfaces` is unchanged since the stamp** (empty `git diff --stat`).
>
> - **What the code does now.**
>   - `unregister_host_memory` (`src/lib.rs:1198-1244`): when `interfaces::is_spdk_env_active()` is true it calls `spdk_mem_unregister`, and a non-zero rc is **kept** as `spdk_err` rather than returned (`:1223-1228`). `cudaHostUnregister` now runs **always** (`:1231`). A CUDA failure returns the CUDA error (`:1232-1237`), so if both steps fail the SPDK error is dropped. Only after a successful CUDA step is the SPDK error returned (`:1239-1242`). The comment at `:1218-1222` gives the reason: if SPDK became active after the region was registered, the SPDK step fails on a range SPDK never saw, and an early return would leak the CUDA pin.
>   - `register_host_memory` (`src/lib.rs:1137-1195`) is **unchanged** by `d45ab524d`. In order: init guard `:1146-1150`, PORTABLE `cudaHostRegister` `:1155-1157`, `we_registered_cuda` `:1158`, `if !is_spdk_env_active() { return Ok(()) }` `:1179-1181`, `spdk_mem_register` `:1183`, EBUSY and rollback `:1184-1191`.
>   - Test `tests/host_memory_without_spdk.rs`: the child process now returns early with `SKIP:` on stderr when `gpu.initialize()` fails (`:35-39`), so on a machine without a GPU the parent sees a pass (`:64-88`). The test still runs only with `--features p2p` (`Cargo.toml:50-52`).
> - **Drifted → P-6 (BACKFILL), moderate:** spec 001 **FR-016** (`specs/001-gpu-cuda-services/spec.md:336-348`) says: "If `spdk_mem_unregister` fails, the method returns the error without calling `cudaHostUnregister`." That sentence was written on 2026-11-07 (P-2) from the old early return. It is now the opposite of the code, which calls `cudaHostUnregister` and then returns the SPDK error. FR-016's provenance citation `src/lib.rs:1220-1225` also moved (the SPDK step is now at `:1223-1228`). Direction BACKFILL: the change came from a code review (Nara, PR #591). It removes a resource leak (a CUDA pin left in place after an `Err`) and keeps the error visible to the caller. The edge case that the 2026-11-07 report recorded as an observation (register with SPDK inactive, unregister with SPDK active) is now handled in code. The FR-016 SHOULD (register and unregister under the same SPDK state) stays valid, because the caller still gets an `Err` in that order.
> - **Still aligned (re-checked against HEAD):**
>   - 001 **FR-015** (`spec.md:310-335`): the code is unchanged and the cited `src/lib.rs:1179-1181` is still exact.
>   - 001 **FR-014**: the no-`gpu` paths return "GPU support not compiled" (`src/lib.rs:1138-1142`, `:1203-1207`).
>   - 001 **FR-007**, last sentence (`spec.md:248-251`): host memory is un/registered only by these two methods.
>   - 002 **FR-019** (`specs/002-gpu-ssd-dma-prepare/spec.md:96`): same code as FR-015.
>   - 002 **FR-020** (`:97`): "calls `spdk_mem_unregister` (only when an SPDK environment is active at call time) then `cudaHostUnregister`". This is still literally true, and it says nothing about the error path, so it does not contradict the new code. More detail lives in 001 FR-016, which P-6 updates. No proposal for 002.
>   - 001 **SC-006** ("All unit tests pass with `cargo test -p gpu-services --features gpu`", `spec.md:495-496`): the new test needs `p2p`, so SC-006 does not cover it. Nothing to change.
>   - `contracts/igpu_services.md:164` only names the methods → FR-015/016.
> - **Interface doc (`components/interfaces/src/igpu_services.rs`), checked against the new code:**
>   - `register_host_memory` `# Errors` (`:802`, "Returns an error if CUDA host registration or SPDK registration fails"): **aligned**. SPDK registration can only fail when it is attempted.
>   - `unregister_host_memory` (`:808-809`, "Calls `spdk_mem_unregister` then `cudaHostUnregister`"): the **order** is still right. After `d45ab524d`, both calls happen whenever SPDK is active, even when the first one fails, so this commit adds no new interface drift and needs **no new interface BACKFILL**.
>   - `:813` ("Returns an error if unregistration fails"): aligned.
>   - What remains is the drift from `9a79224a1`: the SPDK step is conditional, but the doc reads as unconditional (`:786`, `:788-790`, `:808`). That is **P-5**, still open and deferred. P-5 **edits `components/interfaces`, which re-stamps every component.** Its unregister wording is refreshed in `proposals.md` so it also mentions that the CUDA step runs regardless of the SPDK result. The interfaces spec (`components/interfaces/specs/001-interfaces/spec.md:274-275`) is generic and still aligned.
> - **Observations (no proposal):**
>   - (1) **The test passes vacuously without a GPU.** On a GPU-less runner, `host_memory_without_spdk` passes without checking anything (`tests/host_memory_without_spdk.rs:35-39`). Only the `SKIP:` line on stderr shows this, and the parent captures stderr (`.output()`, `:70-79`) and prints it only on failure (`:80-86`). So a green result is evidence for A-14 only on a GPU host such as node7. FR-015 cites this test as provenance (`spec.md:323-324`), which is still accurate.
>   - (2) **No test covers the `d45ab524d` error path** (SPDK unregister fails → CUDA unregister still runs → SPDK `Err`). The new test exercises only the no-SPDK path. Covering it would need a process with SPDK active and an unregistered range, which means real SPDK and a real GPU.
>   - (3) **The error leaves the region unpinned.** After the SPDK-failure `Err`, the region is no longer CUDA-pinned. A caller that retries `unregister_host_memory` would get a `cudaHostUnregister` error. The in-tree callers ignore the result (`components/dispatcher/src/lib.rs:2424`, `components/dispatcher-p2p/src/lib.rs:1735`, both `let _ =`), so this does no harm today. P-6's text states which steps ran.
>   - (4) Carried over unchanged: `allocate_pinned_dma_buffer` → `src/dma.rs:259` still calls `spdk_mem_register` without the guard. That matches 001 FR-020, but it is the same crash class without SPDK, so a dispatcher-side check is still recommended.
> - **Checks:** static only. `cargo test -p gpu-services` was not approved in this session, so it was not run. The GPU test (`--features p2p`, real GPU) was not run either. The commit `d45ab524d` reports dispatcher lib tests only, not a gpu-services GPU run. Not re-verified (no input changed since the 2026-10-31 / 2026-11-07 syncs): every other FR/SC of specs 001/002/003.
>
> **Status I would stamp: `drifted` until P-6 is applied; `clean` after (P-5 stays optional, interfaces).**

> **Apply 2026-11-07 (branch fix/dispatcher-j33-repairs, approved by Cornel).** Applied **P-1** (001 FR-015), **P-2** (001 FR-016), **P-3** (002 FR-019) and **P-4** (002 FR-020) as BACKFILL. Each now says the `spdk_mem_register` / `spdk_mem_unregister` step runs only when an SPDK environment is active (`interfaces::is_spdk_env_active()` at call time). P-2 also records the early `Err` on SPDK failure and the SHOULD to register and unregister under the same SPDK state. Each old text occurred exactly once and was replaced with the exact text from `proposals.md`. `Last-Synced` lines were added to both specs. Backups are in `backups/20261108T001701Z/`. **Tasks:** none (no ALIGN proposals). **Open:** **P-5** (`IGpuServices` doc comments in `components/interfaces/src/igpu_services.rs`) is not approved ("not in this PR (edits components/interfaces, re-stamps every component); bundle with the next interfaces change"). It is recorded as an open item in `align-tasks.md`. The two observations (call-time flag; unguarded `allocate_pinned_dma_buffer` → `src/dma.rs:259`) remain for a human. No `src/` or interfaces file changed. Frontmatter is not touched. **Status to stamp: `clean`.** Details are in `apply-report.md`.

> **Sync 2026-11-07 (branch `fix/dispatcher-j33-repairs`, HEAD `f4579a41f`) — delta, 4 spec requirements + 1 interface doc drifted, BACKFILL proposed (P-1…P-5, none approved, nothing applied). Frontmatter left as stamped; the re-stamp script owns it.** Base: the stamp commit `32ffa627f`. `git diff 32ffa627f HEAD` over this component's inputs: `src/lib.rs` (+17/−3), `Cargo.toml`, new `tests/host_memory_without_spdk.rs`, all from one commit `9a79224a1` ("no SPDK registration without an SPDK environment - A-14"). `components/interfaces` also moved (`ispdk_env.rs` +8, `init_with_allowed`, commit `fc2a2b80d`); this component does not implement or call `ISPDKEnv`, so that part is transitive only.
>
> - **What the code does now.** `register_host_memory` (`src/lib.rs:1137-1195`): init guard (`:1146-1150`), `cudaHostRegister(…, PORTABLE)` (`:1155-1157`), `we_registered_cuda` (`:1158`), then **`if !interfaces::is_spdk_env_active() { return Ok(()) }`** (`:1179-1181`) *before* `spdk_mem_register` (`:1183`); the EBUSY-as-success and `we_registered_cuda` rollback (`:1184-1191`) are unchanged and only reachable with SPDK active. `unregister_host_memory` (`src/lib.rs:1198-1238`): `spdk_mem_unregister` only **`if interfaces::is_spdk_env_active()`** (`:1220-1225`), then `cudaHostUnregister` always (`:1228-1234`). The flag is set by `spdk-env` after a successful `spdk_env_init` (`components/spdk-env/src/env.rs:146`) and cleared at the start of fini (`:260`); read via `interfaces::is_spdk_env_active` (`components/interfaces/src/spdk_types.rs:213`). Same guard pattern already used by `memory-tier` (`components/memory-tier/src/lib.rs:152`, `:314`) and `DmaBuffer::drop` (`spdk_types.rs:381`).
> - **Drifted (spec says SPDK registration is unconditional):** spec 001 **FR-015** (`specs/001-gpu-cuda-services/spec.md:309-326`: "page-locks … via `cudaHostRegister` … and registers it with SPDK via `spdk_mem_register`"), spec 001 **FR-016** (`spec.md:327-331`: "unregisters memory from SPDK via `spdk_mem_unregister` then removes page-locking via `cudaHostUnregister`"), spec 002 **FR-019** (`specs/002-gpu-ssd-dma-prepare/spec.md:95`) and **FR-020** (`:96`, "calls `spdk_mem_unregister` then `cudaHostUnregister`"). None mentions the no-SPDK case. Severity moderate: a caller reading the spec would believe a successful `register_host_memory` means the region is NVMe-DMA-capable; without SPDK it is only CUDA-pinned. Direction BACKFILL: the change fixes a SIGSEGV (found on node7, A30, J40), has a regression test, and matches the established guard pattern; there is no SPDK memory map to register into, so the spec's unconditional wording cannot be met by any code.
> - **Interface doc drifted too** (`components/interfaces/src/igpu_services.rs:786-815`): summary "Register existing host memory as CUDA-pinned and SPDK DMA-capable", body "Calls `cudaHostRegister` … then `spdk_mem_register`" (`:788-790`), errors "if CUDA host registration or SPDK registration fails" (`:802` — still true, SPDK registration simply isn't attempted without an environment); unregister "Calls `spdk_mem_unregister` then `cudaHostUnregister`" (`:808-809`). P-5, **touches `components/interfaces` (re-stamps every component)**; recommended to defer and bundle with the next interfaces change. The interfaces spec (`components/interfaces/specs/001-interfaces/spec.md:274-275`, "Register existing memory for DMA" / "Unregister host memory") is generic and still aligned.
> - **Still aligned:** 001 FR-014 (no-`gpu` paths return "GPU support not compiled", `src/lib.rs:1138-1142`, `:1203-1207`, unchanged); 001 FR-007's last sentence (host un/registration only through these two methods, `spec.md:248-250`); FR-015's PORTABLE flag, CUDA idempotence, EBUSY, `we_registered_cuda` rollback (unchanged lines). The contract file's FR map (`contracts/igpu_services.md:164`) only names the methods, so it needs nothing.
> - **Edge case for the backfill wording:** the flag is read at *call time* in both methods, independently. A region registered while SPDK was inactive and unregistered after SPDK became active would get an `spdk_mem_unregister` on a region SPDK never saw; a non-zero rc returns `Err` at `:1222-1224` *before* `cudaHostUnregister`, leaving the CUDA pin. The reverse order (registered with SPDK, SPDK torn down first) skips the SPDK call, which is correct. No caller in the tree does the first order (the dispatcher registers its memory-tier pool in `initialize` and unregisters it in `shutdown`, `components/dispatcher/src/lib.rs:2256`, `:2419`; dispatcher-p2p likewise, `components/dispatcher-p2p/src/lib.rs:1451`, `:1735`), so I do not propose a code change; the proposed spec text says "at call time" so the contract is accurate.
> - **Observation, not drift (no proposal):** `allocate_pinned_dma_buffer` (`src/lib.rs:1097-1130` → `dma::create_spdk_dma_buffer_from_cuda_host_alloc`, `src/dma.rs:259`) still calls `spdk_mem_register` without the guard. This matches 001 FR-020 (`spec.md:350-356`, registration is the point of the method), so it is not drift, but it has the same crash class as A-14 if called without an SPDK environment. The dispatcher's `StagingPool::new` calls it (`components/dispatcher/src/pipeline.rs:219`). Whether that path is reachable in memory-tier-only mode is a dispatcher question, outside this sync; flagged for a human.
> - **Checks:** read-only. I could not run `cargo test`/`cargo check` in this session (the commands were not approved), and the new test needs a real GPU (`--features p2p`, `Cargo.toml` `[[test]] host_memory_without_spdk`). The commit message reports it passing on node7; I have not reproduced that. Not re-verified (unchanged since the 2026-10-31 sync): every other FR/SC of specs 001/002/003.
>
> **Status I would stamp: `drifted` until P-1…P-4 are applied (P-5 optional, interfaces); `clean` after.**

> **Re-stamp 2026-11-05 (branch `certus-host-setup`, on unstable `bb0c9b9c`).** Transitive only: this component's own `src/` and `specs/` are as unstable has them (its previous stamp was re-verified against `git archive origin/unstable`); its digest moves because `components/interfaces` changed (`ISPDKEnv::init_with_allowed`, added for the two-process drive race; see the spdk-env and interfaces syncs of the same date). This component does not implement `ISPDKEnv`. No analysis below changes.

> **Sync 2026-10-31 (branch `specsync-b`: optimizer session `s20261029-094947` merged onto unstable, `7b2b8b04`, `55a4ddd4` clippy) — delta, drift resolved by backfill. Not stamped here: the final pass stamps every component.** Base `a0292be6`, HEAD `0a71990e`. Changed inputs: `src/lib.rs`, `src/cuda_ffi.rs` (and `build.rs`, not hashed), plus `components/interfaces`.
>
> - **New method, unspecced → BACKFILL B-1 (FR-026, applied):** `memcpy_batch_async` (`src/lib.rs:973-1038`, commit `4941320a`): `spdk`-gated; without `gpu` returns "GPU support not compiled" (`:981-985`); with `gpu`: init guard (`:989-991`), slice-length check (`:992-999`), empty batch → `Ok(())` (`:1000-1002`), one `cudaMemcpyAttributes` with `src_access_order = CUDA_MEMCPY_SRC_ACCESS_ORDER_STREAM` (`:1004-1007`) applied to all entries (`attrs_idxs = [0]`, `num_attrs = 1`), then `cudaMemcpyBatchAsync` (`:1016-1027`); a CUDA error → "cudaMemcpyBatchAsync failed: …" (`:1029-1034`). FFI: `CUDA_MEMCPY_SRC_ACCESS_ORDER_STREAM = 1`, `#[repr(C)] cudaMemLocation` / `cudaMemcpyAttributes` (`src/cuda_ffi.rs:87-106`), `cudaMemcpyBatchAsync` declared with the CUDA 13.x signature (`:157-167`). Checked against `/usr/local/cuda` = 13.2 headers (`cuda_runtime_api.h:6540`, `driver_types.h:2397-2435`): signature, enum value and struct layout match. The CUDA 12.8 signature has an extra `size_t *failIdx` (`generated_cudart_removed_meta.h:316-326`), and the pip `libcudart.so.12` that `build.rs` also adds to the link path exports the same symbol name, so this binding is correct only against a 13.x runtime; FR-026 now states the requirement. Interface contract `components/interfaces/src/igpu_services.rs:745-768` matches. `spec.md` Assumptions (feature-flag line) and `contracts/igpu_services.md` (new section, B-2) updated. After this, every `IGpuServices` method the component implements is named by an FR (the contract file's trait listing is the original subset; the new section maps the rest to their FRs).
> - **Follow-up, same day (commit `8b800832`):** a runtime-version check now enables the batch call only on CUDA 13.0+ and falls back to one `cudaMemcpyAsync` per entry below that (`src/lib.rs:124-147`, `:1035-1058`); FR-026 updated to say so instead of requiring a 13.x runtime. Byte-exact hardware round trip passes with batching on (CUDA 13.2) and forced off (`CERTUS_NO_BATCH_COPY`).
> - **Behaviour-identical, no spec impact:** `7b2b8b04` turns six `#[allow(clippy::not_unsafe_ptr_arg_deref)]` into `#[cfg_attr(feature = "gpu", expect(..., reason))]` (`src/lib.rs:300-307`, `:517-524`, `:750-757`, `:811-818`, `:862-869`, `:923-930`); `build.rs` `unwrap()` → `#[expect(clippy::expect_used)] .expect(...)` and an inlined format arg (same link-search output). Interfaces (`55a4ddd4`): doc backticks, `#[must_use]`, `SAFETY` comments on `GpuIpcHandle` / `GpuDmaBuffer` / `GpuStream` `Sync`, and `GpuDmaBuffer`'s `Debug` now ends with `..` (`finish_non_exhaustive()`, `igpu_services.rs:222`). No spec here quotes `Debug` output (searched all `specs/`), and no type, field or signature the specs name changed. Spec citations into `src/lib.rs` (FR-015 `:960-1010`, FR-020 `:937-939`, FR-023 `:656-678`, FR-024 `:799-851`, FR-025 `:853-895`) and `igpu_services.rs` are dated "Backfilled …" provenance notes, not current locations; left as written (historical).
> - Checks: `cargo test -p gpu-services` 5 + 1 passed; `cargo check -p gpu-services --features gpu,spdk` clean. Not re-verified this run (unchanged): FR-001…FR-025, SC-001…SC-008, specs 002 and 003; the 2026-09-03 verdicts stand.
>
> **Status I would stamp: `clean`.**

> **Re-stamp 2026-10-29 (branch `fix/dispatcher-remove-ssd-extent`, merged with unstable `e35618b2`).** Transitive only: this component's own `src/` and `specs/` are as unstable has them; its digest moves because `components/interfaces` changed (the `IDispatcher::check` doc gains an `# Errors` section, B-17 of the dispatcher sync). No analysis below changes.
> **Re-stamp 2026-10-25 (branch `001-rdma-cache-transfer`, rebased onto unstable `a905f89b`).** Transitive only: this component's own `src/` and `specs/` are as unstable has them; its digest moves because `components/interfaces` changed (`IDispatcher::clear_all`) and the rebase brought unstable's changes. No analysis below changes.

> **Re-stamp 2026-10-23 (branch `unstable`, commit `d7a24d58` "Fix workspace build warnings" plus the spec-sync follow-up).** That commit only fixed warnings under the workspace lint posture: it added `Debug` impls, narrowed unreachable `pub` items to `pub(crate)`, wrapped `unsafe fn` bodies in explicit `unsafe {}` blocks, and the follow-up ran `rustfmt`. It also touched `components/interfaces` (a `Debug` impl for `Command`), which changes every component's digest. This component's own `src/` was in that commit. Every changed item was checked: none is part of a provided interface, none is named as public by the spec, and no behaviour changed. No spec citation into this component's source moved. All citations were re-verified by diff-mapping each base line to its new position. Status stays `clean`.

> **Re-stamp 2026-10-21 (transitive, `components/interfaces` changed on branch `fix/dispatcher-remove-clear-mt`).** Only `components/interfaces` changed for this component: `IDispatcher` doc comments (`flush_to_ssd` errors, `clear_memory_tier`, `batch_lookup`) and the interfaces spec (FR-008, FR-009, FR-017, FR-018, FR-018a, FR-021, FR-023, Overview, NFR-002, Implementation Notes; text only, no type or signature changed). This component uses some of the named items (1 files), but no type, signature or behaviour of them changed. This component's last full sync below stands; no re-analysis was needed.

> **Re-stamp 2026-10-14 (transitive: `components/interfaces` changed, branch `fix/cold-staging-buf-16mib`).** The spec-sync digest folds `components/interfaces/{src,specs}` into every component's hash, so the single interface change on this branch — `DispatcherConfig::cold_staging_buf_bytes` default 4 MiB → 16 MiB (commit `8e12e49f`) and the matching correction to interfaces FR-027 — invalidates every component's stamp at once; this is the design, not a defect. **No code re-analysis was performed for this component, and none was needed:** its own `src/**` and `specs/**` are unchanged on this branch, and it does not reference `cold_staging_buf_bytes` (consumed only by the dispatcher's host-DRAM cold-staging pool). Digest refreshed to the recomputed value.

> **Re-stamp 2026-10-09 (transitive: `components/interfaces` changed, branch `fix/evict-blocked-by-pin`).** The only change to this component's hashed inputs is a doc-comment correction on `IDispatchMap::try_evict_to_block` in `components/interfaces/src/idispatch_map.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and its previous stamp was re-verified to match `origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

> **Re-stamp 2026-10-05 (transitive: `components/interfaces` changed, branch `fix/poller-cpu-placement-logging`).** The only change to this component's hashed inputs is a doc-comment correction on `DispatcherConfig::poller_base_cpu` in `components/interfaces/src/idispatcher.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and it does not reference `poller_base_cpu` (verified by grep over its src/ and specs/). Its previous stamp was re-verified to match `git archive origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

> **Re-stamp 2026-10-01 (transitive: `components/interfaces` changed).** The spec-sync digest
> folds `components/interfaces/{src,specs}` into **every** component's hash, so an interface
> change invalidates all of them at once -- which is the design, not a defect. The interface
> change is the `served_by` attribution work: new `ServedBy` and `LookupOutcome` types, and
> `IDispatcher::batch_lookup` widened to return `Vec<LookupOutcome>`.
>
> **No re-analysis was performed for this component, and the digest bump asserts only what was
> actually checked**: the interface change is additive except for `batch_lookup`'s return type,
> which the compiler enforces across all implementors, and this component's own `src/**` and
> `specs/**` are unchanged. Components that implement `IDispatcher` (`dispatcher`,
> `dispatcher-p2p`) had their specs updated substantively; this one did not need it. If a later
> sweep finds drift here, this stamp is not evidence against it.

> **Re-stamp 2026-09-15 (workspace `cargo fmt` sweep; no drift).** Commit `f9bcd965` ("Add shmq RESERVE batch shared-deadline regression test") ran `cargo fmt` across the whole workspace, reflowing this component's `src/*.rs` (multi-line ↔ single-line argument lists and struct literals, import reordering). `git diff -w` confirms no token-level logic, signature, or contract change — the only substantive addition in that commit is a regression test in `lib/shmq-dispatcher/src/translate.rs`, which is outside this component and outside the spec-sync gate's `components/` scope. The formatting moved this component's `spec_sync_inputs_sha256`, but its spec↔implementation alignment is unchanged. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

> **Re-stamp 2026-09-15 (interfaces-fold; no drift).** Branch `fix-reserve-batch-deadline` (`bec6c6ec`) added a `deadline: Option<std::time::Instant>` parameter to `IDispatcher::reserve_memory` in `components/interfaces/src/idispatcher.rs` (shared batch backpressure deadline for the shm-queue OP_RESERVE handler). `scripts/spec-sync-hash.sh` folds the whole `components/interfaces` tree into every component's hash, so this component's digest moved even though its own spec/implementation did not change. This component does not describe or call `reserve_memory`; the interface delta cannot affect its spec↔implementation alignment. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

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

# Drift Report: gpu-services

**Generated**: 2026-09-03 (Spec-Sync re-sweep + independent verification)
**Project**: gpu-services
**Specs analyzed**: `001-gpu-cuda-services`, `002-gpu-ssd-dma-prepare`,
`003-gpu-p2p-server`
**Mode**: Read-only drift analysis; **ALIGN** (code fix) for spec 001 FR-017 +
**BACKFILL** (spec doc) for spec 001 FR-008 this sweep; spec 003 FR-012 BACKFILL
carried from the prior sweep; then freshness stamp.

This sweep re-verified **every FR/SC across all three specs against the actual
implementation** (not a re-stamp of the prior artifact). The prior sweep reported
77/78 aligned with a single spec-003 FR-012 drift; independent verification this
sweep found **two additional issues the prior sweep missed** in spec 001 (FR-017
init-guard gap; `pub mod cuda_ffi` unspecced public surface). Both are resolved
this sweep — the component is now genuinely aligned.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 3 |
| Requirements Checked | 78 (FR-001..025 + SC-001..008 [001]; FR-001..024 + SC-001..005 [002]; FR-001..012 + SC-001..004 [003]) |
| Aligned (after this sweep) | 78 |
| Drifted → resolved this sweep | 2 (001 FR-017 ALIGN, 001 FR-008 BACKFILL) |
| Drifted → resolved prior sweep | 1 (003 FR-012 BACKFILL) |
| Not Implemented | 0 |
| Unspecced (now documented) | 0 |

**Verification runs this sweep** (all green):
- `cargo build -p gpu-services` — clean
- `cargo clippy -p gpu-services --all-targets -- -D warnings` — clean
- `cargo test -p gpu-services -- --test-threads 1` — 5 + 1 passed; 0 failed
- `cargo check -p gpu-services --features gpu` — clean (compiles the edited
  `#[cfg(feature = "gpu")]` stream code)
- `cargo clippy -p gpu-services --features gpu -- -D warnings` — clean

## Resolved this sweep

### 001-gpu-cuda-services / FR-017 — ALIGN (code fix)

- **Severity**: minor (reachable robustness/contract gap).
- **Spec**: FR-017 states `create_stream`, `destroy_stream`, and
  `stream_synchronize` "**All three require prior initialization** and return
  errors when GPU support is not compiled."
- **Actual (before this sweep)**: only `create_stream` enforced the init guard
  (`src/lib.rs:570-572`). `destroy_stream` (`src/lib.rs:657-676`) and
  `stream_synchronize` (`src/lib.rs:702-721`) went straight to the CUDA call
  under `#[cfg(feature = "gpu")]` with **no `!state.initialized` check** — unlike
  the sibling `set_device`/`device_of_ptr` (FR-021/022) which do enforce it
  (`src/lib.rs:598-600,626-628`). The gap is **reachable**, not theoretical:
  `interfaces::GpuStream` is `pub struct GpuStream(pub *mut c_void)`
  (`components/interfaces/src/igpu_services.rs:225`), so a caller can fabricate a
  `GpuStream` and invoke `destroy_stream`/`stream_synchronize` without ever
  calling `create_stream`/`initialize()`.
- **Direction — code authoritative (ALIGN)**: spec 001 is an authored spec (not
  a code-backfill), and three sibling methods already implement exactly this
  guard with identical code; the two omissions are an oversight, so the spec
  states the intended contract and the code had the defect. Backfilling would
  weaken a legitimate contract to match a bug (forbidden by the HARD RULE).
- **Fix applied**: `destroy_stream` and `stream_synchronize` now lock state,
  return `Err("Not initialized: call initialize() first")` when not initialized,
  then drop the lock before the CUDA call — byte-for-byte the pattern used by
  `create_stream`/`set_device`/`device_of_ptr`. `stream_query` (FR-023) is left
  unchanged: FR-023 deliberately requires only the gpu feature + a valid handle,
  and the code matches (`src/lib.rs:678-700`).
- **Location (fixed)**: `components/gpu-services/src/lib.rs`
  (`destroy_stream`, `stream_synchronize`).

### 001-gpu-cuda-services / FR-008 — BACKFILL (spec doc)

- **Severity**: minor (undocumented but intended public surface).
- **Finding**: `pub mod cuda_ffi` (`src/lib.rs:26`, gated behind `--features
  gpu`) exposes raw CUDA FFI publicly but was **not** covered by FR-008's
  "exclusively through the interface" scope or its existing `dma`-module
  exception. Verified it is genuinely-intended public surface: consumed as
  `gpu_services::cuda_ffi::*` by the `gpu-p2p-server` binary
  (`src/bin/p2p_server.rs:20`) and four sibling apps (`certus-server`,
  `certus-server-yaml`, `gpu-bb-vs-p2p`, `nvme-bar1-bench`) for low-level CUDA
  calls (e.g. `cudaHostRegister`) not surfaced through `IGpuServices`. Reducing
  visibility would break those consumers → **BACKFILL** (document the exception),
  not ALIGN.
- **Also documented**: the `dma`-module cleanup free-functions paired with the
  already-excepted builders (`spdk_unregister_and_ipc_close`,
  `spdk_unregister_unpin_and_ipc_close`, `spdk_unregister_and_cuda_free_host`,
  `spdk_unregister_gdr_unmap_and_close`, `vfio_unmap_extmem_munmap`,
  `vfio_unmap_extmem_only`) — `pub` counterparts used as `DmaBuffer` free-fn
  pointers (no external callers found), now named in the FR-008 exception.
- **Change applied**: extended the FR-008 exception note in
  `specs/001-gpu-cuda-services/spec.md`; added a `Last-Synced: 2026-09-03` line.

## Resolved prior sweep (verified again this sweep)

### 003-gpu-p2p-server / FR-012 — BACKFILL (spec doc)

- The original FR-012 asserted a *runtime* MDTS ceiling the code never
  implemented. Verified this sweep: `do_chunked_read` issues one `ReadAsync` per
  `--chunk-size` chunk and awaits all completions
  (`src/bin/p2p_server.rs:273-323`); an oversized chunk surfaces as an NVMe read
  error — `do_chunked_read` returns `Err` (`:308`), the handler propagates it
  (`:399,461,524`), and the accept loop writes `ERROR: <message>` to the client
  (`:658-659`) — i.e. a **safe failure**, not silent corruption. The MDTS bound
  is an operator responsibility documented via CLI help (`:54`) + the 131072-byte
  default. Correct, intentional code + overclaiming (backfilled-from-code) spec →
  BACKFILL. Applied to spec 003 FR-012 (reworded), US1 Scenario 4 (added),
  Assumptions (added). The HARD RULE holds — no bug is masked.

## Aligned ✓ (verified this sweep)

### 001-gpu-cuda-services (FR-001..025, SC-001..008)
Init/shutdown/discovery (`lib.rs:98,129,150`; compute-cap ≥7 filter
`device.rs:39`); memory attr check (`memory.rs:26`); IPC deserialize/verify/pin/
unpin (`lib.rs:166,192,215,249`; `ipc.rs`); `pin_memory` idempotent (`lib.rs:227`)
and `unpin_memory` tracking-only, never `cudaHostUnregister` (`lib.rs:249-267`,
per the 2026-08-07 backfill); DMA copy sync/async (`lib.rs:301,511,725/779/823/
877`); stream ops incl. the two ALIGN fixes above; `set_device`/`device_of_ptr`
init-guarded (`lib.rs:588-654`); host register/unregister with correct rollback +
ordering (`lib.rs:960,1010`); FR-008 interface exposure (+ documented `dma`/
`cuda_ffi` exceptions); FR-009/010 gpu-feature gating + Criterion benches; SC-008
demo apps present.

### 002-gpu-ssd-dma-prepare (FR-001..024, SC-001..005)
`prepare_memory_for_spdk` pipeline (`lib.rs:351`) with lazy peer access
(`ipc.rs:55`), internal-`HashSet` pin check (`lib.rs:427-438`), free-fn selection
by prior-pin state (`dma.rs:69-102,136-140`), device-context set/restore on all
paths (`lib.rs:372-448`), creation-failure rollback (`lib.rs:478-486`),
`spdk_mem_register`/rollback, host register/unregister, and the GDRCopy builders
`_from_gpu_bar`/`_from_phys`/`_from_bar_direct` (`dma.rs:353,547,635`) + GDRCopy
FFI (`gdrcopy_ffi.rs`, `GPU_PAGE_SIZE = 1<<16`).

### 003-gpu-p2p-server (FR-001..012, SC-001..004)
Feature gating, CLI + defaults, kernel-module check, SPDK/GPU/NVMe init + atexit,
p2p staging pool, UDS accept loop + payload parse + mode dispatch (`handle_bounce`
`:374`, `handle_p2p` `:436`, `handle_p2p_cold` `:493`), `OK …`/`ERROR: …` response
contract, `--once`, SIGINT/SIGTERM cleanup, and chunked reads (FR-012, above).

## Not Implemented ✗
None.

## Unspecced Features
None remaining — `pub mod cuda_ffi` and the `dma` cleanup free-functions are now
documented under the spec 001 FR-008 exception; auxiliary `dma.rs`/`gdrcopy_ffi.rs`
items were backfilled into spec 002 in prior rounds.

## Recommendations
- Spec 003 remains marked "backfilled — needs human review"; a maintainer pass
  would let it graduate from backfilled-draft status (doc-only; outside the
  gate's `src/` + `specs/` hash scope).
