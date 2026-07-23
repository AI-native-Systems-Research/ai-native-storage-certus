# Spec-Sync Apply Report — spdk-env

**Mode**: AUTO-BACKFILL
**Applied**: 2026-07-22
**Source**: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T22:33:36Z)
**Backups**: `.specify/sync/backups/20260722T232033Z/` (pre-edit copies of all
Markdown files touched below)

## Scope

Only Markdown under `components/spdk-env/specs/**` and
`components/spdk-env/.specify/sync/**` was edited. No source code was
touched.

## Actions taken

### 1. SUPERSEDE — `specs/001-spdk-vfio-env/spec.md`

Added a `SUPERSEDED` banner and status change pointing to
`002-spdk-env-vfio-init`. The spec was the raw, never-filled-in
`spec-template.md` scaffold; the feature area it targeted (SPDK/DPDK env
init + VFIO device iteration) is fully specified in `002-spdk-env-vfio-init`.

### 2. BACKFILL — `specs/002-spdk-env-vfio-init/spec.md`

- **SC-005**: Corrected from "structured log messages through the
  framework's logging system" to describe actual `eprintln!`-based
  diagnostics, explicitly noting there is no logger receptacle (matches
  FR-007 and `src/env.rs`). Resolves the internal SC-005/FR-007
  contradiction flagged as high severity in the drift report.
- **FR-019** (new): Documents the explicit `fini(&self)` method on
  `ISPDKEnv` (teardown precondition: controllers detached / DmaBuffers
  freed; idempotent; relationship to Drop-based cleanup in FR-012).
- **FR-020** (new): Documents the `DmaBuffer` API (`new()`, `unsafe
  from_raw()`, `Deref`/`DerefMut`, Drop-time SPDK deallocation gated by the
  `interfaces::set_spdk_env_active`/`is_spdk_env_active` coordination flag).
- **FR-021** (new): Documents the five operator shell scripts
  (`bind_vfio.sh`, `add_kernel_options.sh`, `cfg_user_spdk.sh`,
  `show_spdk_devices.sh`, `fix_dnf_cache.sh`) as sanctioned setup tooling.
- **Key Entities**: Added a `DmaBuffer` entry; updated the `ISPDKEnv`
  description to mention `fini()`.
- **Assumptions**: Cross-referenced `scripts/bind_vfio.sh`,
  `add_kernel_options.sh`, and `cfg_user_spdk.sh`/`show_spdk_devices.sh` by
  name in the two assumptions that previously said configuration was
  "performed externally" with no named tooling.

### 3. BACKFILL — stale ILogger references

- **`specs/002-spdk-env-vfio-init/contracts/ispdk-env.md`**: Removed the
  `logger: ILogger` receptacle from the Component Declaration, removed
  `LoggerNotConnected` from `init()`'s preconditions/errors, removed the
  `comp.logger.connect(logger_arc)` step from the Usage Contract, and added
  a `fini()` method section and a `fini()` step in the Usage Contract.
- **`specs/002-spdk-env-vfio-init/data-model.md`**: Removed the
  `LoggerNotConnected` variant from the `SpdkEnvError` table and the
  `SPDKEnvComponent --receptacle--> ILogger` relationship line; added a
  `DmaAllocationFailed` variant, a `DmaBuffer` entity section, and
  `fini()`-related relationships/state-transition edges (replacing the
  `Constructed -> LoggerWired -> Initialized` chain with
  `Constructed -> Initialized -> Finalized` via `init()`/`fini()`/`drop()`).

### 4. ALIGN/DEFECT/NOTE tasks — `.specify/sync/align-tasks.md` (new file)

Four tasks appended (details in that file):

1. **Task 1** (medium, ALIGN) — NVMe-only enumeration (`env.rs`
   `enumerate_devices()`) vs. SC-001/User Story 1/Clarifications' "all VFIO
   device types" claim. Left as a decision point (extend enumeration vs.
   re-scope the spec) rather than resolved in this Markdown-only pass.
2. **Task 2** (low, DEFER) — SC-002's stale "missing logger" misconfiguration
   clause; not explicitly in scope for this pass, left for a follow-up
   trivial backfill.
3. **Task 3** (medium, DEFECT) — `do_init()`'s error path never calls
   `spdk_env_fini()` to unwind a successful `init_spdk_env()`; currently
   latent because `enumerate_devices()` never returns `Err`. Requires a code
   change, out of scope here.
4. **Task 4** (informational, NOTE) — FR-015 already self-flags as future
   work; left as-is per instructions. Noted that User Story 1 Acceptance
   Scenario 4 / its Edge Case still describe the skip-and-warn behavior as
   working today, which remains inconsistent with FR-015's caveat.

## Not changed

- `specs/002-spdk-env-vfio-init/plan.md`, `research.md`, `tasks.md`,
  `checklists/requirements.md` — out of scope for this resolution set; no
  drift findings named them directly.
- No `NEW_SPEC` was created — none of the unspecced features (DmaBuffer,
  `fini()`, operator scripts) warranted a standalone spec; all were folded
  into `002-spdk-env-vfio-init` as new FRs.
- No source code under `components/spdk-env/src/` or `scripts/` was modified.
