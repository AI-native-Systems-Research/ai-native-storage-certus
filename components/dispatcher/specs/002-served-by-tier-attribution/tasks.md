# Tasks: Serving-Tier Attribution — Phase 1 (counters)

**Input**: Design documents from `specs/002-served-by-tier-attribution/`
**Prerequisites**: spec.md (reconciled), plan.md (Phase 1 scope), research.md (R1–R5)

**Scope**: Phases 1–4. Phase 1 (counters) is complete; Phases 2–4 were tasked on
2026-09-29 when the decision to implement remote attribution was taken.

**Tests**: Included. Two of the three defects below are *silent* — they drop information
rather than fail — so a test that merely exercises the path proves nothing. Every test task
below states what it must be shown to fail against.

## Format: `[ID] [P?] Description`

- **[P]**: can run in parallel (different files, no dependency)
- Ordering is deliberate: **T001 measures before anything is fixed.** See T001's note.

## Phase 1a: Establish the baseline before changing anything

- [x] **T001** Measure the zero reading against a stated prediction, on the existing build.
  Two runs of one workload that misses, `--until 10 --rate inf`, reading
  `curl localhost:9400/metrics` after each:
  - **solo** (one server, `SOLO=1`, no peers) — predict `certus_lookup_misses_total` > 0
  - **shared** (`stress_a`, peers reachable) — predict `certus_lookup_misses_total` == 0

  Record both readings plus `certus_lookup_hits_total` in `research.md` R2.

  **Why first, and why it is not optional**: R2's cause is confirmed by reading the code, so
  this is a test of that understanding, not a search. If solo *also* reads zero, a second
  defect is hiding and T004's fix would mask it — the number would move and we would wrongly
  conclude we had understood it. Harness: `/tmp/stress-servers.sh`, `/tmp/stress-agents.sh`,
  `/tmp/rate-probe.sh` (cold-format per probe; see its header).

- [x] **T002** [P] Capture the current `hits + misses` versus entries-requested gap on the same
  two runs, from the generator's own per-run counts against the server's counters. This is the
  FR-024 baseline: without it, "accounting is now complete" is unfalsifiable.

  **Result**: before the fix the gap was the whole miss count — hits alone matched the
  generator (354 023 and 301 240 exactly) while misses read 0. After T004 both sides agree
  exactly and hits + misses = 959 167 = total references, in both arms, with zero errors.
  **Consequence: the two accounting holes never fired in this workload** (nothing was held
  back, no non-`KeyNotFound` error occurred), so T012..T016 are hardening and this result is
  not evidence about them.

## Phase 1b: The classification fix — the confirmed cause

- [x] **T003** [P] Unit test in `components/dispatcher` proving a remote **miss** is reported
  as `KeyNotFound`, not `IoError`, and that a remote **transport failure** is still `IoError`.
  Mock `IRemoteLookup` returning `RemoteLookupError::NotFound` for one key and
  `TransportError` for another in one batch.

  **Must be shown to fail** against today's code, which maps both to `IoError`. A test that
  passes before the fix is testing nothing.

- [x] **T004** Preserve the distinction at `components/dispatcher/src/lib.rs:2624-2627`: map
  `RemoteLookupError::NotFound` → `DispatcherError::KeyNotFound(key)` and leave
  `TransportError` → `IoError`. The interface already carries the distinction
  (`iremote_lookup.rs:102-106`); the dispatcher collapses it.

  **This is a behaviour change to a Certus component, not only an accounting fix**: a key no
  node holds stops being reported as an I/O error. Any consumer reasoning about error rates
  currently sees transport failures that never happened.

- [x] **T005** Re-run T001's two readings. **Predicted outcome**: both now report non-zero
  misses, and the shared-versus-solo miss counts differ by roughly the number of keys a peer
  did serve. Record against the prediction, and say so plainly if it does not hold.

## Phase 1c: The remote-hit counter

- [~] **T006 NOT NEEDED — superseded.** No new `TranslatorObserver` method was added.

  The observer is a per-op hook on the *translator*, but this counter does not travel that
  way: `TierEventStats` is an existing struct on `IDispatcher::tier_event_stats()` that both
  servers **already poll**, and it already carries exactly this class of counter
  (`promotions_to_gpu`, `evictions_from_memory`, the store-retry counts). Two fields there
  reach the servers through a path that already runs end to end.

  The plan assumed the observer because it assumed the server had to be *told*; it only had
  to *ask*. Recorded rather than ticked, so the plan's wrong assumption stays visible.

- [x] **T007** Count per batch at `components/dispatcher/src/lib.rs:2575`, where the dispatcher
  zips `remote_results` back and already knows each key's outcome. Requester-side: what this
  node *obtained from* peers.

  Do **not** count in `remote-lookup` — that measures what peers asked *of* this node, which
  is a different quantity under a name that would invite conflation (research.md R4).

- [x] **T008 RESOLVED, and the risk did not materialise.** `IDispatcher` was not widened and
  Phase 1's independence claim (research.md R1) holds.

  The dispatcher needs no observer handle: it records into its own `TierEventCounters`, and
  the servers read the result through `tier_event_stats()`, which they already call. The only
  `interfaces` change is **two fields on an existing struct** — not a signature change, and
  emphatically not `batch_lookup`'s return type.

  **One caveat that contradicts `contracts/idispatcher.md`.** That contract says this area is
  "compiler-enforced; nothing silently keeps working". True of a signature change; **false of
  a struct field.** The other three implementors build `TierEventStats::default()`, so the
  workspace compiled with zero errors and nothing forced them to update. A future implementor
  can therefore silently under-report. Fix the contract's claim in Phase 2.

- [x] **T009 done differently than written.** `ServiceCounters` was **not** extended and
  `CountersObserver` gained nothing — both would have been the observer route T006 dropped.

  Instead `serve_metrics` takes a `tier_event_stats()` snapshot beside the memory-tier and
  NVMe snapshots it already takes, and renders the two counters from it.

  **This cost a wasted hardware run, and the lesson is worth more than the task.** The
  counters were first added only to `telemetry.rs`, because research R3 recorded "Export |
  telemetry.rs — OTel observable counters". But there are **two** metrics paths: `telemetry.rs`
  exports to an OTLP collector, while `/metrics` — the endpoint actually scraped — is
  hand-rolled Prometheus text in `metrics.rs::serve_metrics`. `certus_lookup_hits_total`
  appears there because it is rendered from `ServiceCounters`, not because OTel exports it. So
  the first attempt exported correct numbers to a collector nobody reads, and the hardware
  reading showed the counters simply absent. **Both export paths are now wired.**

- [x] **T010** [P] Export both as OTel observable counters
  (`apps/certus-server-yaml/src/telemetry.rs:111-125`), named
  `certus.remote_lookup_hits_total` and `certus.remote_lookup_misses_total` — dots in the
  declaration, underscores in Prometheus, matching the existing pair.

- [x] **T011** Test that the counters move only on remote service, and are untouched by a
  purely local hit. **Must be shown to fail** if `on_remote_lookup` is wired to the local
  path — the failure mode that would make the counter agree with `lookup_hits` and look
  plausible while measuring nothing.

## Phase 1c result: the benefit, measured 2026-09-28

**Remote lookup serves 0.369% of what it is asked** — 2 443 hits of 661 534 forwards,
0.81% of all hits, **0.255pp** of a 31.3% hit rate, against **27.5% of stores declined in
the same run**. Full reading, caveats and the two non-results in `research.md` R6;
`remote-benefit.sh` beside this file re-runs it.

This is what Phase 1 was for. It does not decide whether remote lookup should stay — one
workload with 262 migrations cannot — but the question is now empirical instead of
unanswerable, which it was not before the counter existed.

## Phase 1d: Close the two accounting holes (FR-024)

- [x] **T012** Count held-back entries. Entries whose handles fail to open are excluded from
  the batch and reported `ok = 0` (`translate.rs:571-583`), so the counting loop never sees
  them. They must be counted — as misses or as a third category, decided in T013.

- [x] **T013** Decide and record whether a handle-resolution failure is a *miss* or an
  *error*. It is not obviously either: the key may well be resident, and the failure is the
  caller's handle. **Recommendation**: a third `errors` count, because calling it a miss would
  corrupt the hit rate with a client-side fault. Needs a decision before T014.

- [x] **T014** Replace `Err(_) => {}` (`translate.rs:604-605`) so every dispatched entry lands
  in exactly one of hits / misses / errors. `KeyNotFound` → miss; everything else → error.

- [x] **T015** Extend `on_lookup` to carry the error count, or add it alongside — same
  defaulting constraint as T006.

- [x] **T016** Test the FR-024 identity directly: for a batch mixing a hit, a miss, a
  transport failure and an unopenable handle, assert
  `hits + misses + errors == entries requested`. **Must be shown to fail** against today's
  code, which drops two of those four.

## Phase 1d result

**T013 decided: an unopenable handle is an error, not a miss.** The key may well be resident;
the fault is in the caller's handle, so calling it a miss would corrupt the hit rate with a
client-side fault. Recorded at the definition in `translate.rs` as well as here.

The tally was extracted as a **pure function**, `Translator::tally_lookup(requested, results)`,
for a reason worth keeping: the identity cannot otherwise be tested without a GPU, because
every handle fails to open in a test process, so the dispatched set is always empty there. The
alternative — asserting on counters after driving `op_lookup` — can only ever observe the
all-held-back case, which is exactly the case that used to count zero.

`on_lookup` carries all three counts in **one** call rather than gaining a second method: split
across two, a host could implement one and not the other and silently break the identity.

Both T016 tests were **shown to fail** against a mutation restoring the two original holes
(`errors = 0` instead of `requested - results.len()`, and `Outcome::Error => {}`), then to pass
without it.

## Phase 1e: Documentation (FR-030, FR-031)

- [x] **T017** [P] Update `components/dispatcher/specs/001-dispatcher-cache-interface/` for the
  `KeyNotFound`-versus-`IoError` behaviour change. T004 changes what `batch_lookup` returns for
  a remote miss, and that component's spec is the artifact describing its contract.

- [x] **T018** [P] Document the new observer method and the accounting rule beside their
  definitions in `lib/shmq-dispatcher/src/translate.rs` — it owns no `specs/`, so the site is
  the record (FR-031), following how `check_state` documents its own widening.

- [x] **T019** [P] Record in this spec which of FR-024..FR-026 Phase 1 satisfies and which
  await later phases, so a reader is not left inferring it from the task list.

- [x] **T020** Update `research.md` R2 with T001's and T005's actual readings. A research
  document asserting a prediction without its outcome is the failure this phase is structured
  to avoid.

## Phase 1f: Gate

- [x] **T021** Gate run 2026-09-28. `cargo test --all -- --test-threads 1`: **1361 passed, 0
  failed**. `cargo test --doc`: 61 passed, 0 failed.

  **The three static gates have pre-existing drift, and the honest gate is therefore "no new
  findings", not "clean".** Measured, not assumed — the clippy baseline was taken by stashing
  this work and re-running:

  | Gate | Clean tree | With this work | Where |
  |---|---|---|---|
  | `clippy -- -D warnings` | 12 errors | 12 errors | `izyre.rs` (16 sites), `iblock_device.rs` |
  | `cargo doc --no-deps` | 4 warnings | 4 warnings | `igpu_services.rs` ×2, `idispatcher.rs:199`, `pipeline.rs:221` |
  | `cargo fmt --check` | 5 files | 5 files | `eviction-replay-benchmark` ×3, `pipeline.rs`, `zyre/build.rs` |

  Every one attributed by `git blame` to another author or an earlier commit. CLAUDE.md requires
  a warning-free `cargo doc`, so the tree does not currently meet its own standard; fixing that
  is not this feature's change and is not folded in silently.

  **Do not run `cargo fmt` unscoped, or `-p dispatcher`**: it reformats `pipeline.rs`, which is
  unrelated drift. That happened twice here and was reverted twice.

- [~] **T022 not needed for Phase 1d — and saying why is the point.** No hardware re-run was
  made after T012..T016, deliberately: T002 established that **neither accounting hole fired in
  this workload** (nothing was held back, no non-`KeyNotFound` error occurred), so a re-run
  would read `certus_lookup_errors_total 0` and confirm nothing. A green run that cannot
  distinguish the fix from its absence is not evidence.

  The instruction still stands for any *later* hardware run: rebuild **both** the server and the
  node agent, because the provenance digest spans all crates and a stale agent is refused at the
  handshake (FR-051) — correctly, but it looks like a broken run.

  Exercising the error path on hardware needs a workload that actually produces one (a
  deliberately bad handle, or an injected transport failure). Worth doing; it is a new task, not
  this one.

## Dependencies

```text
T001 ──┬─> T003 ──> T004 ──> T005 ──> T020
T002 ──┘                      │
                              ├─> T017
T006 ──> T007 ──> T008 ──> T009 ─┬─> T010 ──> T011
                                 └─> T018
T012 ──> T013 ──> T014 ──> T015 ──> T016
T005, T011, T016 ──> T021 ──> T022
```

T001 blocks T004 by intent, not by data: the fix must not land before the baseline exists.

T008 is the risk. If it cannot be resolved without widening `IDispatcher`, it invalidates
Phase 1's independence and the phasing needs revisiting — which is why it says *stop and
report* rather than *carry on into Phase 2*.

## Out of scope for Phase 1, restated

The `ServedBy` taxonomy, `IDispatcher::batch_lookup`'s return type, the `LOOKUP` byte
widening and its open five-value decision, `dispatcher-p2p`, `remote-lookup`, and both
verification-bearing component specs. All are Phases 2–4.

**Do not infer from this list which of those phases owns each item** — `dispatcher-p2p` lands
in Phase 2 because the compiler forces it to, leaving Phase 4 to own verification and the
component specs. There is no longer an `IRemoteLookup` delta in any phase: the two remote
taxonomy values were collapsed on 2026-09-29, so no tier crosses that interface. See plan.md's
boundary note.

---

# Phase 2 — the `ServedBy` interface (tasked 2026-09-29)

**Decided before tasking**: remote attribution is IN. Remote lookup serves 0.81% of hits, which
is an argument for removing the *feature* later, not for declining to account for it while it is
in the tree. `REMOTE` is one value (the `REMOTE_DRAM`/`REMOTE_SSD` split stays withdrawn).

- [x] **T101** Add `ServedBy` to `components/interfaces`: `Dram | Ssd | Remote | Miss |
  SizeMismatch | Error`, with `is_hit()` true for the first three. Defined once here (FR-032);
  no other crate may restate the value space.

- [x] **T102** Add `LookupOutcome { served_by, result }`. **Not** `Result<ServedBy, E>`: the
  tier-on-`Ok` encoding cannot express `Miss`/`SizeMismatch`/`Error`, which are the `Err` cases,
  and would push a third of the taxonomy into a per-server error→tier mapping — which is how the
  two servers would drift.

- [x] **T103** Widen `IDispatcher::batch_lookup` to `Vec<LookupOutcome>`. Signature change, so
  every implementor is a compile error until updated — all four, per plan.md's boundary note.

- [x] **T104** `components/dispatcher`: attribute at each resolution site. `MemoryTier` warm hit
  → `Dram`; every cold sub-path (pooled read, inline fallback, staging post-pass, no-drives) →
  `Ssd`; the remote-delivery arm → `Remote`; `KeyNotFound` after the remote attempt → `Miss`;
  everything else → `Error`. The tier is already known at each site — this is propagation, not
  new bookkeeping.

- [x] **T105** `components/dispatcher-p2p`: its SSD→GPU cold path attributes `Ssd` per FR-014
  **even though it does not synchronously populate DRAM**. Not a placeholder — FR-014 already
  fixes this value, and FR-027 forbids shipping a value no test can fail against.

- [x] **T106** The two mock implementors (`remote-lookup/src/seams.rs`,
  `lib/shmq-dispatcher`'s test mock). Mocks must return a value consistent with what they
  simulate, not a constant — a mock that always says `Dram` makes every attribution test vacuous
  (FR-028).

- [x] **T107** Test the invariants of `contracts/idispatcher.md`: length and order,
  `served_by.is_hit() ⇔ result.is_ok()`, and `Miss ⇔ Err(KeyNotFound)` after any remote attempt.
  **Each must be shown to fail** against a deliberately wrong attribution.

# Phase 3 — the wire byte

- [x] **T108** Widen `LOOKUP`'s per-key byte in `lib/shmq-dispatcher`: `0` not served, `1` Dram,
  `2` Ssd, `3` Remote. `0` keeps its exact meaning. `PENDING` MUST NOT appear here — see
  `contracts/served-by.md`; the non-zero range means "delivered", and a pending key was not.

- [x] **T109** Fix `workload-node-agent::split_by_lookup`: `*served == 1` → `!= 0`, and move its
  byte-`2`-is-not-a-hit assertion to the unassigned range (`4..=255`), where the original
  reasoning still holds. **This is the only reader that breaks**; the consumer sweep in
  `contracts/served-by.md` records why the connector does not.

- [x] **T110** Test that a conforming server never emits a value outside `0..=3`, and that an
  unknown non-zero value is read as *served, tier unknown* rather than not-served.

- [x] **T111** Render the per-tier hit counts the byte now permits, on both metrics paths —
  `/metrics` and OTel, in step, per Phase 1's T009 lesson.

## Phase 2/3 result: attribution measured on hardware, 2026-09-29

4 instances (node2 + node5, one per NUMA domain), one RDMA group, `--until 10 --rate inf`,
cold-formatted, both binaries rebuilt on both hosts.

| | |
|---|---|
| served keys | **300 845** |
| `lookup_hits_dram` | 239 677 — **79.6%** |
| `lookup_hits_ssd` | 59 073 — **19.6%** |
| `remote_lookup_hits` | 2 095 — **0.69%** |

**Three independent checks close exactly**, which is what makes this trustworthy rather
than merely plausible:
1. **The partition holds per instance and in aggregate**: dram + ssd + remote == hits, on
   all four servers separately (SC-017).
2. **The server agrees with the client to the digit**: 300 845 served == the generator's
   own independent count of keys that returned data.
3. **FR-024 closes**: hits + misses == 959 167 == total key references.

**THE RUN CAUGHT A REAL DEFECT THAT THE WHOLE UNIT SUITE MISSED.** The first attempt
reported 300 845 served on the servers against **224 848** at the generator — short by
*exactly* ssd + remote. A second `== 1` reading survived in
`workload-node-agent::count_results` (the reporting path), besides the one already fixed
in `split_by_lookup` (the store-decision path). Its unit test used `[1, 0, 1]`, which
cannot distinguish `== 1` from `!= 0` — the same blind spot mutation testing found in the
cold-serve counter, in a different file. **A fixture whose values are all 0 or 1 cannot
test a tiered byte.** The test now uses `[1, 2, 3, 0]` and was mutation-verified.

That the discrepancy equalled ssd + remote *precisely* is what identified the cause in one
step: an accounting identity that fails by a recognisable quantity names its own defect.

# Phase 4 — verification and the component specs

- [x] **T112** Feature spec for attribution in `components/dispatcher-p2p/specs/`, covering
  FR-014's cold-path difference. Verification-bearing, so it is a spec and not a comment.

- [x] **T113** Feature spec for `components/remote-lookup/specs/` covering what it contributes
  to attribution. Note it contributes no *tier* — the delta is withdrawn — so this is narrower
  than the original plan assumed.

- [x] **T114** FR-027/FR-028 tests in both dispatchers: every attribution value has a test that
  fails if that value is mis-assigned, and the mocks are shown to model residency faithfully
  enough that the assertions are not vacuous.

- [x] **T115** FR-029: run the suites under the `integrity-check` feature as well as default.
  Phase 1 never did this and recorded it as unaddressed.

- [~] **T116 — the p2p-native run now RUNS, and getting there found two real defects.**
  Attempted 2026-09-30 at the user's direction. The verification itself is still open; what
  the attempt produced is worth more than the task.

  **Two defects, both pre-existing this task, both fixed (`e69bd306`):**
  1. **`CERTUS_PROFILE=full-p2p` could not build the yaml server at all** — `EvictionEvent`
     existed as two identical types, one per dispatcher, while the transport host hardcoded
     `dispatcher`'s. Present on the base branch too, so not caused by this feature. The type
     crosses a component boundary and now lives in `interfaces`, re-exported by both.
  2. **The p2p server reported 5 526 hits with dram 0 and ssd 0.** Its `tier_event_stats()`
     returned `TierEventStats::default()`. **This is the Phase 1 caveat materialising inside
     the same feature**: adding fields to that struct is not compiler-enforced, so an
     implementor silently under-reports. Per-key attribution had been correct all along —
     only its aggregate face was missing, which is exactly why the invariant test passed and
     nothing caught it. The missing test now exists in that component and is
     mutation-verified against the precise defect.

  **SC-008 was also wrong and is corrected.** It claimed a repeat read reports `Ssd` twice.
  That cannot be right: this component backfills DRAM *asynchronously*, so a later read may
  legitimately be `Dram`, and "twice `Ssd`" holds only inside a window shorter than the
  backfill that no black-box client can observe. FR-014's actual claim is that promotion is
  not on the *serving* path, whose observable form is a higher `Ssd` share than `dispatcher`
  on one workload. **My first experiment design tested the wrong thing** and would have
  produced a false refutation.

  **RESULT 2026-09-30: the control arm is established, the p2p arm CANNOT COMPLETE A RUN,
  and that is now the blocker rather than the harness.** Two instances on node2, `--until
  10 --rate inf --seed 1`, identical workload, binaries swapped between arms:

  | arm | generator exit | hits | dram | ssd | **ssd share** |
  |---|---|---|---|---|---|
  | **dispatcher** | 0 (valid) | 329 171 | 223 708 | 105 463 | **32.03%** |
  | **p2p** | 3 (INVALID) | 12 246 | 12 246 | 0 | 0% |

  **The comparison is not valid, because p2p aborted at 3.7% of the work.** Its 0% is
  therefore vacuous, not a refutation: at 12 246 hits the tier had not begun to overflow,
  and the control arm shows SSD traffic only appears well beyond that point. The partition
  held exactly in both arms, which is the one thing this run does confirm on both
  dispatchers under load.

  **p2p fails the same way every time — three attempts — and the cause is a client-side
  timeout, not a crash.** The servers were verified alive *after* each run (`9400=y
  9401=y`), and the generator reported "instance became unreachable ... Resource
  temporarily unavailable (os error 11)". That wording is the **known read-timeout
  misattribution**: `ClientError` has no `TimedOut` variant, so a timeout is rendered as
  unreachability. It cost three attempts to see past, and it is exactly the follow-up
  already recorded against the generator.

  So the working hypothesis is that **`dispatcher-p2p` cannot sustain this workload at
  `--rate inf`** — roughly 27x less work completed before the client's deadline expired.
  Whether that is a performance characteristic of the BAR1/D2D cold path or a defect is
  not established here and must not be asserted.

  **PACED RUN, 2026-09-30 — pacing does NOT help, which refutes the throughput
  hypothesis.** Both arms paced at `--rate 0.02` (50x slower than real time):

  | arm | rate | exit | hits | ssd share |
  |---|---|---|---|---|
  | p2p | `inf` | 3 | 12 246 | 0% |
  | **p2p** | **0.02** | **3** | **13 011** | 0% |
  | dispatcher | 0.02 | 0 | 324 328 | **27.12%** |

  Delivering the workload **50x slower moved p2p from 12 246 hits to 13 011** — no
  material change. So "`dispatcher-p2p` cannot sustain the arrival rate" is **refuted**;
  the failure is tied to the work done, not to how fast it arrives.

  **And the p2p server is HEALTHY at the point of failure.** Its log shows checkpoints
  completing in ~2 ms (`54 722 extents, 3.6% used`) right up to the moment this script's own
  teardown stopped it ("actor shutting down, 3 clients connected"). Nothing hung or crashed.
  The generator's error is specifically an **agent** connection failure — `os error 11`
  (EAGAIN) between generator and agent, not between agent and server.

  **Refined hypothesis, NOT established: a per-operation latency spike in the p2p path
  exceeds the generator↔agent deadline.** This is the one shape consistent with every
  observation: pacing lowers the *arrival rate* but not per-operation latency, so it cannot
  help a single slow operation breach a socket deadline. For scale, the *control* arm —
  which passes — already shows `latency p50 734 µs, p99 214 ms, max 3.23 s`. A p2p tail
  beyond the agent's read deadline needs only to be somewhat worse than that.

  **A framing point that matters before anyone treats this as a regression: p2p has
  apparently NEVER been exercised through the yaml server.** The `full-p2p` profile did not
  build until the `EvictionEvent` fix earlier today, so there is no working baseline to A/B
  against, and this behaviour may be long-standing and simply never reachable before.
  Its unit tests pass; this is an integration-path finding.

  **T116 stays OPEN, and the blocker is now specific and no longer about attribution.**
  What is verified on p2p: the taxonomy value, the three interface invariants, and the route
  partition — the last confirmed on hardware, twice, under load. What is not: the `Ssd` share
  comparison, because p2p cannot complete a run.

  **Next steps, in order:**
  1. Instrument or trace the p2p agent path for a latency tail — the `--read-timeout`
     the generator lacks would also make this diagnosable instead of inferable.
  2. Fix the generator's timeout-vs-unreachable misattribution. It has now misdirected this
     task four times and is the reason the throughput hypothesis survived as long as it did.
  3. Only then retry the share comparison.

## Out of scope, decided 2026-09-29

**`HIT_PENDING`.** The plumbing exists (`check_state::PENDING`, `CHECK_PENDING`,
`compat.lookup_result_pending()`) and has no caller, because the load path deliberately collapses
`PENDING` to absent. Enabling it changes vLLM's *scheduling*, so it changes the hit rates these
phases exist to report — it needs its own change and its own before/after, not to be bundled
here. Full reasoning and the bounding facts in `contracts/served-by.md`.
