# Drift Resolution Proposals — dispatcher, branch `001-rdma-cache-transfer`

Generated: 2026-10-21. Based on: `drift-report.md` § *Sync 2026-10-21 (branch 001-rdma-cache-transfer)*. HEAD `c9429de6d`, base `origin/unstable` `460a6f554`. All proposals are `approved: false`; nothing is applied. Every `old` string below occurs exactly once in its file (checked).

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 7 |
| Align (Spec → Code) | 3 |
| Human Decision | 3 |
| New Specs | 0 |
| Remove from Spec | 0 |

### RB-01 · 001 contract read_to_bounce # Errors (D-R1)

**Direction**: BACKFILL · **Confidence**: HIGH

Spec `specs/001-dispatcher-cache-interface/contracts/idispatcher.md:112-114` against code `src/lib.rs:4034-4051`.

`specs/001-dispatcher-cache-interface/contracts/idispatcher.md` — old:

```text
        /// `NoSpace` (reserve full now), `NeverFits` (never can be), `Io` (the
        /// read failed; the lease has been freed), `NotInitialized`.
```

new:

```text
        /// `NoSpace` (reserve full now), `NeverFits` (never can be), `Io` (the
        /// read failed; the lease has been freed, or, if a read ended without its
        /// completion, leaked and logged; either way the caller owns no lease),
        /// `NotInitialized`.
```

**Rationale**: FR-062 and the code agree that the Unsettled path leaks the lease. The contract says it is always freed. The same wording in components/interfaces/src/idispatcher.rs:895-897 (which also leaves out NotInitialized) is a doc-comment fix for the interfaces owner and is not in this component's specs/.

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RB-02 · 001/FR-001 (D-R2)

**Direction**: BACKFILL · **Confidence**: HIGH

Spec `specs/001-dispatcher-cache-interface/spec.md:314` against code `src/lib.rs:3988-4059; interfaces/src/idispatcher.rs:898-907`.

`specs/001-dispatcher-cache-interface/spec.md` — old:

```text
Contract in interfaces `001-interfaces` FR-031a; its caller is remote-lookup `002-remote-lookup-rdma` FR-039.)*
```

new:

```text
Contract in interfaces `001-interfaces` FR-031a; its caller is remote-lookup `002-remote-lookup-rdma` FR-039.)*
  - **Peer block-tier serving** (see FR-062): `read_to_bounce`, `release_bounce`. *(Sync 2026-10-21, branch `001-rdma-cache-transfer`.)*
```

**Rationale**: FR-001 is the full method inventory, and the branch added two trait methods.

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RB-03 · 001 Key Entities: Pipelined Reader + new Bounce Lease (D-R3)

**Direction**: BACKFILL · **Confidence**: HIGH

Spec `specs/001-dispatcher-cache-interface/spec.md:434` against code `src/pipeline.rs:1269-1398`.

`specs/001-dispatcher-cache-interface/spec.md` — old:

```text
used by the cold-load staging serve, FR-053). Memory-tier SPDK+CUDA co-registration is required.
```

new:

```text
used by the cold-load staging serve, FR-053); (4) `read_into_bounce` (single object into a bounce lease, no GPU, queue depth 16, `BOUNCE_READ_TIMEOUT_MS`, drains every read and reports `Settled`/`Unsettled`; used by `read_to_bounce`, FR-062). Memory-tier SPDK+CUDA co-registration is required.
- **Bounce Lease**: A range of the memory tier's bounce reserve (`IMemoryTier::bounce_alloc`), outside the cache's slots, holding one block-tier entry read for a peer by `read_to_bounce` (FR-062). It is never in the dispatch map and is returned with `release_bounce`; a lease a drive may still be writing is leaked, never returned.
```

**Rationale**: The variant list is an inventory, and a new public pipeline function was added.

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RB-04 · 001 Key Entities: Memory-Tier Slot (D-R4)

**Direction**: BACKFILL · **Confidence**: MEDIUM

Spec `specs/001-dispatcher-cache-interface/spec.md:429` against code `memory-tier/src/lib.rs:347-348,625-631`.

`specs/001-dispatcher-cache-interface/spec.md` — old:

```text
Eviction is capacity-based (bytes used vs pool size).
```

new:

```text
Eviction is capacity-based (bytes used vs the cache's capacity, which is the pool less the memory tier's bounce reserve, at most an eighth of the pool; FR-062).
```

**Rationale**: On this branch the memory tier takes the reserve out of the top of the pool. Tests that size a pool to the cache exactly now set set_bounce_reserve(0) (repair_test_eviction_window_tinylfu.rs:190,260).

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RB-05 · 001 Assumptions: memory-tier calls (D-R5)

**Direction**: BACKFILL · **Confidence**: HIGH

Spec `specs/001-dispatcher-cache-interface/spec.md:470` against code `src/lib.rs (mt.* call sites)`.

`specs/001-dispatcher-cache-interface/spec.md` — old:

```text
(only calls insert/get/peek/remove/evict_next/oldest_keys/touch).
```

new:

```text
(it calls `insert`, `get`, `peek`, `remove`, `contains`, `oldest_keys`, `touch`, `batch_touch`, `capacity`, `used`, `pool_info`, `is_dma_capable`, and, for FR-062, `bounce_alloc`/`bounce_free`).
```

**Rationale**: The list was already short before this branch. This branch adds two more calls. The replacement list comes from grepping the mt.* calls in src/lib.rs and src/background.rs (evict_next is no longer called).

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RB-06 · 001 contracts/errors.md NotInitialized row (D-R6)

**Direction**: BACKFILL · **Confidence**: MEDIUM

Spec `specs/001-dispatcher-cache-interface/contracts/errors.md:32` against code `src/lib.rs:3995-4000,4055-4059`.

`specs/001-dispatcher-cache-interface/contracts/errors.md` — old:

```text
| `NotInitialized` | all methods except `shutdown` | Receptacles not bound or `initialize()` not called |
```

new:

```text
| `NotInitialized` | all methods except `shutdown`, `read_to_bounce` and `release_bounce` | Receptacles not bound or `initialize()` not called. `read_to_bounce` reports the same condition as `BounceError::NotInitialized` (FR-062); `release_bounce` returns nothing |
```

**Rationale**: The row claims every method, but read_to_bounce returns a different error type.

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RB-07 · 001/FR-062 leak classes + depth (Q-R3)

**Direction**: BACKFILL · **Confidence**: MEDIUM

Spec `specs/001-dispatcher-cache-interface/spec.md:411-414` against code `src/pipeline.rs:1338-1397; src/lib.rs:4028`.

`specs/001-dispatcher-cache-interface/spec.md` — old:

```text
    completion (a timeout, or an error ahead of a controller reset) leaves the lease
    **leaked**, logged as an error, never freed.
```

new:

```text
    completion (`Completion::Timeout`, or any `Completion::Error` naming an operation — a
    controller reset or the actor shutting down) leaves the lease **leaked**, logged as an
    error, never freed. A disconnected completion channel, a failed submit and a failed
    channel checkout are settled. At most 16 segments are in flight.
```

**Rationale**: Says exactly what the code does, so a later change to the classes would show up as drift.

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RA-01 · code hygiene (Public APIs require doc comments; clippy -D warnings) (Q-R1)

**Direction**: ALIGN · **Confidence**: HIGH · **Severity**: minor

- Spec says: CLAUDE.md: unsafe public functions need doc comments with a # Safety section, and clippy must be warning-free.
- Code does: The doc comment for read_into_bounce, # Safety included, sits above `pub enum BounceReadError` (src/pipeline.rs:1280-1300). The function (:1302) has none, so clippy reports missing_safety_doc. src/lib.rs:7698 reports assertions_on_constants.
- **Change**: Move the doc block from above BounceReadError to just above `pub unsafe fn read_into_bounce` and give the enum its own one-line doc. Turn the first assert in the_bounce_read_timeout_cannot_fire_or_overflow into a `const _: () = assert!(...)` or compare against a runtime value.

**Rationale**: This is a regression introduced on this branch. Pre-existing clippy warnings elsewhere are out of scope.

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RA-02 · 001/FR-062 safety (lease bounds) (Q-R2)

**Direction**: ALIGN · **Confidence**: MEDIUM · **Severity**: moderate

- Spec says: FR-062: the read is `len` rounded up to the block size, into a lease from bounce_alloc.
- Code does: bounce_alloc sizes the lease as len rounded up to 4096 (memory-tier/src/lib.rs:701). read_into_bounce writes len rounded up to block_size (pipeline.rs:1312). A block size over 4 KiB would write past the lease.
- **Change**: In read_to_bounce, before bounce_alloc, return NeverFits if `block_size > 4096` (or `4096 % block_size != 0`). Alternatively, call bounce_alloc with len rounded up to block_size. Add a test with a mock drive whose block size is 8192.

**Rationale**: The SAFETY comment at lib.rs:4016 assumes 4 KiB. No dispatcher drive today has a block size over 4 KiB, so this is latent.

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RA-03 · 001/US-13 AS1 'nothing is promoted and the cache is untouched' (T-R1)

**Direction**: ALIGN · **Confidence**: HIGH · **Severity**: minor

- Spec says: AS1: on success nothing is promoted and the cache is untouched.
- Code does: Only the failure test asserts this, with mt.used()==0 (lib.rs:7584). No test checks MockDispatchMap on the success path.
- **Change**: In read_to_bounce_reads_the_drive_the_key_is_on, assert that mt.used()==0 and that the mock dispatch map has no entry or calls after both reads.

**Rationale**: This is a test gap. The code is correct (there are no dm/insert calls in lib.rs:3988-4053).

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RH-01 · 001/US-13 AS4 vs FR-014 shutdown (real hang) (D-R7)

**Direction**: HUMAN_DECISION · **Confidence**: HIGH

read_to_bounce holds the data_drives read guard (src/lib.rs:4001) for a read that has no time limit. shutdown takes data_drives.write() (:2186) before signal_stop (:2191). The actor's own shutdown is the only thing that ends a hung read (actor.rs:1464), so a hung drive deadlocks shutdown. parking_lot RwLock is task-fair, so readers on healthy drives (lib.rs:808,958,2502,3182,3764) also block once shutdown is waiting.

Options:
- A (ALIGN, code): in read_to_bounce, clone the Arc'd block_dev_iface (and the pool) and drop the data_drives guard before the read; ChannelPool would need to be Arc'd or its lease made owned. Spec: add to FR-062 'MUST NOT hold the data_drives lock across the read'.
- B (ALIGN, code): in shutdown, signal_stop every drive before taking data_drives.write(), so pending reads end (Completion::Error → Unsettled, lease leaked) and the guard is released.
- C (BACKFILL): accept it and document under FR-014/FR-062 that shutdown waits for every bounce read and a hung drive hangs shutdown.

Questions:
- Is shutdown with a hung drive a case that must end (container restart, SIGTERM)?
- Is A's refactor (an owned channel lease) acceptable, or is B the smaller fix?

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RH-02 · 001/US-13 'Why this priority' (D-R8)

**Direction**: HUMAN_DECISION · **Confidence**: MEDIUM

The rationale says 'measured SSD hit rates are single-digit percent'. Spec 002 (tasks.md:327) records lookup_hits_ssd 19.6% on a 4-instance run, and other runs have shown 15.0% and 18.4%. The single-digit figure comes from the endpoint's research D9 and refers to workload-generator runs with a well-sized memory tier.

Options:
- A: cite the source and qualify it: 'measured workload-generator runs with a well-sized memory tier show single-digit-percent SSD hit rates (certus-transfer-endpoint research D9); undersized tiers have measured 15-20% (spec 002)'.
- B: drop the number: 'block-tier serving is a completeness path, off the hot path when the memory tier is sized to the working set'.

Questions:
- Which runs should the priority claim rest on?

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---

### RH-03 · 002 Assumption (spec.md:751-753), contract (contracts/idispatcher.md:30-32), SC-003 (spec.md:684-687) (D-R9)

**Direction**: HUMAN_DECISION · **Confidence**: MEDIUM

Spec 002 says a responder promotes a disk-resident key into its own DRAM before the RDMA write, so no fabric byte comes from a peer's disk, and the peer-triggered promotion count measures that. With certus-transfer-endpoint plus read_to_bounce, the responder reads into a bounce lease and does not promote. The bytes come from the SSD via DRAM, nothing is promoted, and peer_triggered_promotions does not move. This is true of remote-lookup today but not of the new transport. No app wires the endpoint in yet (no Cargo dependents), so nothing is false in production yet.

Options:
- A: amend now. Make the Assumption and contract remote-lookup-specific, and add that the transfer endpoint serves block-tier entries via read_to_bounce without promoting, so SC-003's promotion count does not apply to it.
- B: defer to the PR that wires the endpoint into certus-server-yaml, and record it as an open item in align-tasks.md.

Questions:
- Should the responder-side 'cold work peers cause' counter also cover bounce reads (a new counter on the endpoint or the dispatcher)?

**Action**: [ ] Approve  [ ] Reject  [ ] Modify

---
