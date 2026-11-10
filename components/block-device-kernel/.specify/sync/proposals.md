# Drift Resolution Proposals — block-device-kernel

Generated: 2026-10-14T17:31:02Z
Based on: `.specify/sync/drift-report.json` from 2026-10-14T17:31:02Z (HEAD `5d123390`, branch `fix/block-device-kernel-timeout-cancel`)
Mode: BATCH. Nothing applied. **Every proposal is `approved: false`.**
Spec file: `specs/001-block-device-kernel/spec.md` (line numbers below are for the file at HEAD).
This replaces the 2026-08-20 proposals (both of those were resolved: FR-021/SC-006 ALIGN fixed 2026-09-03; FR-027 backfilled 2026-08-20).

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 5 |
| Align (Spec → Code) | 3 |
| Supersede (replace an inaccurate spec note) | 1 |
| Human Decision | 0 (one spec gap is raised in the drift report, not as a proposal) |
| New Specs | 0 |
| Remove from Spec | 0 |

Rule used to choose a direction: backfill only where the code is intended, working behaviour (the branch's tested fix, or the framework's real actor contract). Align where the code breaks a requirement that is correct as written (FR-012, FR-025, NFR-004). Never backfill a spec to describe a bug.

---

### Proposal 1: 001-block-device-kernel/NFR-006

- **id**: `bdk-2026-10-14-01`
- **Type**: **BACKFILL**
- **Requirement**: NFR-006 — what `on_idle()` returns
- **Spec says** (`spec.md:149`): "The actor `on_idle()` remains active (returns `true`) while clients are connected or inflight operations exist."
- **Code does**: `!self.clients.is_empty() || !self.inflight.is_empty() || !self.pending_cancels.is_empty()` (`src/actor.rs:946`). The `pending_cancels` term is new on this branch. The framework reads the `bool` as "did useful work": `false` only increases `idle_count` and leads to a 10 ms park after 10,000,000 idle calls. It never ends the loop (`lib/component-framework/crates/component-core/src/actor.rs:83-92`, `:661-681`).
- **Old spec text** (`spec.md:149`, exact):

  ```
  - **NFR-006**: The actor `on_idle()` remains active (returns `true`) while clients are connected or inflight operations exist.
  ```
- **New spec text**:

  ```
  - **NFR-006**: The actor `on_idle()` returns `true` (the component-framework "did work" hint, which keeps the actor polling instead of parking) while clients are connected, inflight operations exist, or timed-out operations still have an `AsyncCancel` queued for submission (`pending_cancels`); otherwise it returns `false`. The return value is a park hint, not a liveness signal — returning `false` never terminates the actor loop (see FR-024). Consequently the actor busy-polls and does not park while any client is connected.
  ```
- **Rationale**: The new condition comes from the branch's tested fix. It must hold, or a queued cancel could sit while the actor parks. The "did work" meaning is the framework's documented contract and is outside this component. Same wording should go into `plan.md:75` ("Actor stays alive while clients connected"), which repeats the old claim.
- **Confidence**: HIGH
- **approved**: false

---

### Proposal 2: 001-block-device-kernel/FR-024

- **id**: `bdk-2026-10-14-02`
- **Type**: **BACKFILL**
- **Requirement**: FR-024 — graceful shutdown
- **Spec says** (`spec.md:135`): "Component MUST support graceful shutdown via `ControlMessage::Shutdown` which causes `on_idle()` to return `false`, terminating the actor loop."
- **Code does**: `Shutdown` sets `shutdown_requested` (`src/actor.rs:931-933`). `on_idle` then returns `false` without doing any work (`:938-940`). The loop actually ends because `BlockDeviceKernelComponent::shutdown()` sends `Shutdown` and calls `ActorHandle::deactivate()` (`src/lib.rs:186-192`). That closes the channel and joins the thread (core `actor.rs:218-226`; loop `break` on `ChannelError::Closed`, `:684-697`). Inflight ops get no completion.
- **Old spec text** (`spec.md:135`, exact):

  ```
  - **FR-024**: Component MUST support graceful shutdown via `ControlMessage::Shutdown` which causes `on_idle()` to return `false`, terminating the actor loop.
  ```
- **New spec text**:

  ```
  - **FR-024**: Component MUST support graceful shutdown via `shutdown()` (also exposed as `IBlockDeviceAdmin::shutdown`), which sends `ControlMessage::Shutdown` to the actor and then deactivates it. On `Shutdown` the actor sets its shutdown flag, after which `on_idle()` returns `false` immediately and no longer polls clients, harvests completions or checks timeouts. The actor loop itself terminates when `ActorHandle::deactivate()` closes the control channel and joins the actor thread. Async operations still in flight at shutdown receive no completion.
  ```
- **Rationale**: The observable result (shutdown stops IO and joins the thread) is correct and intended. Only the mechanism in the spec is wrong. The last sentence records current behaviour. A reviewer who wants in-flight ops to be completed or cancelled at shutdown should reject that sentence and open an ALIGN task instead (`tasks.md:32` T011 already plans a test for this case).
- **Confidence**: HIGH (mechanism); MEDIUM (whether "no completion" should be the contract)
- **approved**: false

---

### Proposal 3: 001-block-device-kernel/Edge case "Async operation timeout"

- **id**: `bdk-2026-10-14-03`
- **Type**: **BACKFILL**
- **Requirement**: edge case on async timeout (also covers SC-005)
- **Spec says** (`spec.md:103`): "Timed-out operations produce `Completion::Timeout` and an `AsyncCancel` is submitted to io_uring."
- **Code does**: removes the op from `inflight`, sends `Timeout` at once and queues the key (`src/actor.rs:841-845`). `submit_pending_cancels` (`:856-875`) pushes oldest-first. On a full SQ it calls `submit()` and retries once (`:864-868`). Anything that still does not fit stays queued (`:869-871`) and is retried every tick, including when `inflight` is empty (`:822-825`). The cancel's CQE (`user_data 0`) and the late CQE of the cancelled op match no `inflight` entry and are ignored (`:777`).
- **Old spec text** (`spec.md:103`, exact):

  ```
  - **Async operation timeout**: Timed-out operations produce `Completion::Timeout` and an `AsyncCancel` is submitted to io_uring.
  ```
- **New spec text**:

  ```
  - **Async operation timeout**: A timed-out operation is removed from the inflight map and `Completion::Timeout` is delivered immediately. An `AsyncCancel` for it is queued (`pending_cancels`, FIFO) and submitted to io_uring oldest-first; if the submission queue is full, the actor calls `submit()` to free slots and retries once, and a cancel that still does not fit stays queued and is retried on every subsequent idle-loop tick. The cancel is never dropped, but it may reach io_uring after `Completion::Timeout` was delivered, possibly several poll cycles later. The cancelled operation's own late CQE and the cancel's CQE (`user_data == 0`) are ignored.
  ```
- **Rationale**: This is the fix found by formal verification (`1e6d0b3f`), tested by `src/repair_test_timeout_cancel.rs:55-115`. The spec should state the "never dropped" guarantee so later changes cannot quietly lose it. It should also be honest that the cancel can be deferred.
- **Confidence**: HIGH
- **approved**: false

---

### Proposal 4: 001-block-device-kernel/Key Entity `KernelHandler`

- **id**: `bdk-2026-10-14-04`
- **Type**: **BACKFILL**
- **Requirement**: Key Entities — `KernelHandler`
- **Spec says** (`spec.md:156`): "Owns the file descriptor (`OwnedFd`), `DeviceConfig`, `IoUring` ring, client sessions map, inflight operations map, and optional logger."
- **Code does**: the struct at `src/actor.rs:103-118` also has `next_handle` (`:109`), `pending_cancels: VecDeque<u64>` (`:113`, new), `shutdown_requested` (`:115`) and the feature-gated `telemetry: Arc<TelemetryStats>` (`:116-117`).
- **Old spec text** (`spec.md:156`, exact):

  ```
  - **`KernelHandler`**: The actor handler implementing `ActorHandler<ControlMessage>`. Owns the file descriptor (`OwnedFd`), `DeviceConfig`, `IoUring` ring, client sessions map, inflight operations map, and optional logger.
  ```
- **New spec text**:

  ```
  - **`KernelHandler`**: The actor handler implementing `ActorHandler<ControlMessage>`. Owns the file descriptor (`OwnedFd`), `DeviceConfig`, `IoUring` ring, client sessions map, inflight operations map, the FIFO of timed-out operation keys whose `AsyncCancel` has not yet been submitted (`pending_cancels`, see the async-timeout edge case), the next-`OpHandle` counter, the shutdown flag, optional logger, and (with `--features telemetry`) the shared `TelemetryStats`.
  ```
- **Rationale**: Describes the struct as it is. `pending_cancels` is the state behind the new guarantee in Proposal 3.
- **Confidence**: HIGH
- **approved**: false

---

### Proposal 5: 001-block-device-kernel/Implementation note "fsync markers"

- **id**: `bdk-2026-10-14-05`
- **Type**: **BACKFILL**
- **Requirement**: Implementation Notes — ignored CQEs
- **Spec says** (`spec.md:206`): "CQEs with `user_data & (1 << 63) != 0` are treated as internal fsync markers and silently ignored."
- **Code does**: the filter exists (`src/actor.rs:700`, `:769`). Nothing in the crate sets bit 63 or submits an fsync. The only producer is the new test's filler NOPs (`src/repair_test_timeout_cancel.rs:9-10`, `:82`). Cancel SQEs use `user_data(0)` (`src/actor.rs:663`, `:861`). Handles start at 1 (`:134`).
- **Old spec text** (`spec.md:206`, exact):

  ```
  - CQEs with `user_data & (1 << 63) != 0` are treated as internal fsync markers and silently ignored.
  ```
- **New spec text**:

  ```
  - CQEs with `user_data & (1 << 63) != 0` are silently ignored: bit 63 of `user_data` is reserved for internal SQEs that are not tracked operations. The component itself currently submits no such SQEs (there is no fsync — durability comes from `O_DSYNC`, FR-004); the unit test that fills the submission queue uses the bit for its filler NOPs. `AsyncCancel` SQEs use `user_data == 0`, which matches no `OpHandle` (handles start at 1), so their CQEs are ignored too.
  ```
- **Rationale**: "fsync markers" is left over from the sibling `block-device-filesys` design. The filter behaves as described, but its stated purpose does not exist in this crate.
- **Confidence**: HIGH
- **approved**: false

---

### Proposal 6: 001-block-device-kernel/Implementation note on `on_idle`

- **id**: `bdk-2026-10-14-06`
- **Type**: **SUPERSEDE** (the note is replaced by the corrected NFR-006/FR-024 wording)
- **Requirement**: Implementation Notes — `on_idle()` return value
- **Spec says** (`spec.md:207`): "The `on_idle()` actor method returns `false` only when shutdown is requested; it returns `true` while clients or inflight ops exist to keep the actor thread alive."
- **Code does**: returns `false` after shutdown (`src/actor.rs:938-940`) **and** whenever there are no clients, no inflight ops and no pending cancels (`:946`). The value is a park hint (core `actor.rs:661-681`). The thread ends only on `deactivate()` (`src/lib.rs:186-192`).
- **Old spec text** (`spec.md:207`, exact):

  ```
  - The `on_idle()` actor method returns `false` only when shutdown is requested; it returns `true` while clients or inflight ops exist to keep the actor thread alive.
  ```
- **New spec text**:

  ```
  - The `on_idle()` return value is the component-framework "did work" hint (see NFR-006), not a liveness signal: it is `false` once shutdown is requested and whenever there are no clients, no inflight ops and no queued cancels, and `true` otherwise. The actor thread ends only when `shutdown()` deactivates the actor (FR-024).
  ```
- **Rationale**: The current note contradicts itself ("only when shutdown" vs. the idle-with-no-clients case) and gets the framework contract wrong. It is replaced, not just extended, so it agrees with Proposals 1 and 2.
- **Confidence**: HIGH
- **approved**: false

---

### Proposal 7: 001-block-device-kernel/FR-012

- **id**: `bdk-2026-10-14-07`
- **Type**: **ALIGN**
- **Requirement**: FR-012 — `AbortOp` submits `AsyncCancel` and sends `AbortAck`
- **Spec says** (`spec.md:123`): "Component MUST support `AbortOp` by submitting an `AsyncCancel` SQE to io_uring and sending an `AbortAck` completion."
- **Code does**: `handle_abort` (`src/actor.rs:662-673`) discards the push result (`let _ = self.ring.submission().push(&cancel_sqe);`, `:667`). It then removes the op from `inflight` (`:671`) and sends `AbortAck` (`:672`) anyway. On a full SQ no cancel is ever submitted, and the kernel op keeps running untracked. This is the same defect the branch fixed for timeouts. It was already there before the branch.
- **Proposed code change** (spec unchanged):

  ```rust
  fn handle_abort(&mut self, client_id: u64, target: OpHandle) {
      // Only cancel ops the actor still tracks; the AsyncCancel goes through the
      // same never-dropped queue as timed-out ops.
      if self.inflight.remove(&target.0).is_some() {
          self.pending_cancels.push_back(target.0);
          self.submit_pending_cancels();
      }
      self.send_completion(client_id, Completion::AbortAck { handle: target });
  }
  ```
  Decision for the implementer: today an abort for an unknown/completed handle still pushes a cancel. Keep that, or skip the cancel (as above). Either way, `AbortAck` is sent.
- **Task**: apply the change. Add a unit test next to `src/repair_test_timeout_cancel.rs`: fill the SQ, call `handle_abort` on a tracked handle, and assert a `user_data == 0` CQE shows up. Run clippy and tests in both feature sets.
- **Rationale**: The spec is correct; the code breaks it in the full-SQ case. Backfilling "may silently drop the cancel" would describe a bug in the spec.
- **Confidence**: HIGH
- **approved**: false

---

### Proposal 8: 001-block-device-kernel/FR-025 (with FR-002 and edge case `spec.md:104`)

- **id**: `bdk-2026-10-14-08`
- **Type**: **ALIGN**
- **Requirement**: FR-025 — unbounded backlog, reclaimed when the session is removed; FR-002 — disconnect is logged at debug
- **Spec says** (`spec.md:104`): "...a permanently full-or-disconnected client's completions accumulate without limit until either the ring frees up or `ControlMessage::DisconnectClient` removes the session (dropping the backlog with it)." (`spec.md:113`, FR-002: "...log ... client connect/disconnect operations at debug level.")
- **Code does**: nothing in the crate ever builds `ControlMessage::DisconnectClient`. The only mentions are the variant (`src/actor.rs:82`) and its handler (`:925-930`). `IBlockDevice` has no disconnect method (`components/interfaces/src/iblock_device.rs:557-589`). A client that drops its `ClientChannels` keeps its session, and its `pending` backlog grows (`src/actor.rs:55-60`) until component shutdown. The disconnect debug log (`:926-928`) is dead code.
- **Proposed code change**: in `poll_clients` (`src/actor.rs:890-913`), detect a dead client and remove it through the existing disconnect path. A client is dead when its ingress receiver reports the producer is gone (`try_recv` → `ChannelError::Closed`) or its callback sender reports the consumer is gone (`try_send` → `Closed`). Removing it drops the backlog and runs the FR-002 debug log. First check that `component_core` SPSC endpoints really report `Closed` when the other side is dropped. If they do not, add a liveness query to the channel or a disconnect method to `IBlockDevice` (an interface change, which affects every implementor).
- **Task**: implement, plus a unit test that drops a client's `ClientChannels` and asserts the session is removed.
- **Alternative if the reviewer rejects the code change** (BACKFILL, not proposed): amend `spec.md:104` and FR-025 to say "no disconnect path exists today; the session and backlog of a dropped client are retained until component shutdown".
- **Rationale**: The spec describes a clean-up that cannot happen. Without it, memory grows without limit, which FR-025's own justification (robustness against stalled clients) argues against. So the spec intent wins.
- **Confidence**: MEDIUM (depends on what the SPSC channel reports; there may be an interface decision)
- **approved**: false

---

### Proposal 9: 001-block-device-kernel/NFR-004

- **id**: `bdk-2026-10-14-09`
- **Type**: **ALIGN**
- **Requirement**: NFR-004 — every `unsafe` block has a `// SAFETY:` comment
- **Spec says** (`spec.md:147`): "Unsafe code blocks MUST include `// SAFETY:` justification comments."
- **Code does**: no SAFETY comment at `src/config.rs:87` (`std::mem::zeroed()` for `libc::stat`), `src/config.rs:179` (`libc::close(raw_fd)` in the O_DIRECT-check error branch), `benches/throughput.rs:88`, `benches/latency.rs:52`, `benches/latency.rs:76` (`posix_memalign` blocks). The rest of `src/` is compliant, including the new `:863`/`:867`.
- **Proposed code change**: add comments only. For example, `// SAFETY: libc::stat is a plain C struct; the all-zero bit pattern is a valid value.` (`config.rs:87`); `// SAFETY: raw_fd came from File::into_raw_fd above and is closed exactly once on this error path.` (`config.rs:179`); `// SAFETY: posix_memalign is given a valid out-pointer; the result is checked before use and freed by libc_free.` (benches).
- **Task**: add the 5 comments; run `cargo clippy -p block-device-kernel --all-targets -- -D warnings`.
- **Rationale**: The rule is correct and also a project-wide convention (root `CLAUDE.md`). The code is simply incomplete.
- **Confidence**: HIGH
- **approved**: false

---

## Not proposed (recorded in the drift report only)

- **Buffer lifetime after `Timeout`/`AbortAck`** (drift-report "Spec gap"): no requirement exists either way, so there is nothing to backfill or align. It needs a human decision first (who owns the buffer until the kernel's CQE arrives).
- **Header `spec.md:11` "Last Synced"**: metadata. Update it when approved proposals are applied.
- **`plan.md:75`**: same wrong claim as NFR-006. Update it with the same wording when Proposal 1 is applied.
