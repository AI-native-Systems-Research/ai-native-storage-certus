# Drift Resolution Proposals: memory-tier (branch 001-rdma-cache-transfer)

Generated: 2026-10-22T00:30:00Z  
Based on: drift-report.md section 'Sync 2026-10-21 (branch 001-rdma-cache-transfer, full re-analysis)' (`drift-report.md`)  
Tree: `c9429de6` vs origin/unstable 460a6f55

Every proposal is `approved: false`, and nothing has been applied. `old` strings were checked: each occurs exactly once in its file at `c9429de6` (P14's second edit is descriptive, not literal).

## Summary

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 10 |
| Align (Spec → Code) | 3 |
| Human Decision | 1 |
| New Specs | 0 |
| Remove from Spec | 0 |

**Real bug:** P1. When `pool_size` is not a multiple of 4 KiB, bounce leases are not page-aligned.

Still open from 2026-10-14 (unchanged in substance): FR-013 (ALIGN, align-tasks P3); SC-3 (ALIGN, align-tasks P4); U2 disconnected receptacle (decision, 2026-10-14 P9); 2026-10-14 P2 interface docs (folded into P8 here).

Suggested order: P1 (code and test first), then P2–P7 and P12 (spec wording), P8 (interfaces, re-stamp all components LAST), P9–P11, P13 (decision), P14.

## Proposals

### Proposal P1: 001-memory-tier / FR-031 / FR-004 / NFR-010: lease alignment

**Direction**: ALIGN  
**Confidence**: HIGH  
**Severity**: moderate  
**REAL BUG**

- Spec says: FR-004: All allocations are 4 KiB-aligned for NVMe DMA compatibility; FR-031: the reserve is whole pages at the top of the pool
- Code does: reserve start = pool_ptr + (pool_size - reserve). That is 4 KiB-aligned only if pool_size % 4096 == 0, which initialize does not require. Example: pool_size=64*4096+100 gives cache_size=229476, so every lease is misaligned by 100 bytes, and the dispatcher DMAs device reads into leases
- Location: `components/memory-tier/src/lib.rs:290-292,347-348,715; components/dispatcher/src/lib.rs:4005-4040; apps/certus-server-yaml/src/main.rs:203-219`

**Code change**: In initialize, round cache_size down to a page: `let cache_size = (pool_size - reserve) & !4095;` (the reserve then starts page-aligned; the sub-page tail, pool_size % 4096 bytes, is left unused by both and still counted in pool_info). Alternative: reject `pool_size % 4096 != 0` with InvalidSize. Add a test `a_reserve_in_an_unaligned_pool_is_page_aligned` (initialize(64*4096+100), pretend_dma, assert lease.ptr as usize % 4096 == 0); it fails on the current code.

`components/memory-tier/specs/001-memory-tier/spec.md`:207: apply with the code change

Old:
```
| FR-031 | `initialize` sets aside a bounce reserve of `min(requested, pool_size / 8)` whole pages at the top of the pool (default
```
New:
```
| FR-031 | `initialize` sets aside a bounce reserve of `min(requested, pool_size / 8)` whole pages, starting on a 4 KiB boundary, at the top of the pool (default
```

**Decision point**: Round cache_size down (keeps accepting any pool_size) or reject unaligned pool_size (changes US3 acceptance; a breaking error for raw-byte --memory-tier-size). Recommended: round down.

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P2: 001-memory-tier / FR-007

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: A single first-fit `FreeList` allocator and a single `HashMap<CacheKey, Slot>` slot map serve the whole pool
- Code does: One FreeList + HashMap serve the cache (pool_size less the reserve); a second FreeList in Mutex<Bounce> serves the reserve
- Location: `components/memory-tier/src/lib.rs:83-89,98,353-361`

`components/memory-tier/specs/001-memory-tier/spec.md`:183

Old:
```
| FR-007 | A single first-fit `FreeList` allocator and a single `HashMap<CacheKey, Slot>` slot map serve the whole pool | Implementation |
```
New:
```
| FR-007 | A single first-fit `FreeList` allocator and a single `HashMap<CacheKey, Slot>` slot map serve the whole cache (the pool less the bounce reserve); the reserve has its own allocator (FR-032) | Implementation |
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P3: 001-memory-tier / FR-018

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: clear() removes all entries, resets allocators, returns count
- Code does: clear() resets only the cache allocator, to cache_size; the bounce allocator and its live leases are untouched
- Location: `components/memory-tier/src/lib.rs:652-666`

`components/memory-tier/specs/001-memory-tier/spec.md`:194

Old:
```
| FR-018 | clear() removes all entries, resets allocators, returns count | Unit test |
```
New:
```
| FR-018 | clear() removes all entries, resets the cache allocator (the bounce reserve and its leases are untouched), returns count | Unit test |
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P4: 001-memory-tier / FR-023

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: exception list covers insert size==0, batch_touch([]), is_dma_capable, telemetry_snapshot, initialize
- Code does: set_bounce_reserve checks the flag inverted (errors once set); bounce_alloc returns BounceError::NotInitialized; bounce_free and bounce_usage do not check the flag (before init: bounce_free panics as 'not a live lease', bounce_usage returns (0, 0))
- Location: `components/memory-tier/src/lib.rs:673-738`

`components/memory-tier/specs/001-memory-tier/spec.md`:199

Old:
```
`is_dma_capable()` and `telemetry_snapshot()` do not check the flag (their results are well-defined before initialization); `initialize()` rejects an already-set flag | Implementation |
```
New:
```
`is_dma_capable()`, `telemetry_snapshot()` and `bounce_usage()` do not check the flag (their results are well-defined before initialization; `bounce_usage()` returns `(0, 0)`), and neither does `bounce_free()`, for which no lease can be live before initialization, so it panics (FR-032); `bounce_alloc()` returns `BounceError::NotInitialized`; `initialize()` and `set_bounce_reserve()` reject an already-set flag | Implementation |
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P5: 001-memory-tier / FR-032 + US8 acceptance (NotInitialized)

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: bounce_alloc answers NoSpace or NeverFits
- Code does: also BounceError::NotInitialized before initialize, asserted by a_lease_larger_than_the_reserve_never_fits
- Location: `components/memory-tier/src/lib.rs:686-688,1017-1021`

`components/memory-tier/specs/001-memory-tier/spec.md`:208

Old:
```
`bounce_alloc` answers `NoSpace` (transient) or `NeverFits` (oversize, zero, no reserve, or a pool that is not DMA-capable);
```
New:
```
`bounce_alloc` answers `NotInitialized` before `initialize`, then `NoSpace` (transient) or `NeverFits` (oversize, zero, no reserve, or a pool that is not DMA-capable);
```

`components/memory-tier/specs/001-memory-tier/spec.md`:127-128

Old:
```
  `NeverFits` when it never can — larger than the reserve, no reserve, zero length, or a pool
  that is not DMA-capable
```
New:
```
  `NeverFits` when it never can — larger than the reserve, no reserve, zero length, or a pool
  that is not DMA-capable; `NotInitialized` before `initialize`
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P6: 001-memory-tier / Overview (D3)

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: The pool is held as a single, unsharded structure guarded by one `RwLock<Pool>`: a single first-fit free-list allocator and a single `HashMap<CacheKey, Slot>` slot map.
- Code does: the cache is the pool less a bounce reserve with its own allocator
- Location: `components/memory-tier/src/lib.rs:91-109,347-361`

`components/memory-tier/specs/001-memory-tier/spec.md`:18

Old:
```
Eviction decisions are delegated to an external `IEvictionPolicy` component via a receptacle binding, which provides pluggable eviction strategies (currently LRU) and its own internal synchronization; eviction-order touches are performed after releasing the pool lock.
```
New:
```
Eviction decisions are delegated to an external `IEvictionPolicy` component via a receptacle binding, which provides pluggable eviction strategies (currently LRU) and its own internal synchronization; eviction-order touches are performed after releasing the pool lock. The top of the pool (by default 16 MiB, at most an eighth) is a bounce reserve outside the cache, with its own allocator behind a mutex, leased to serve block-tier entries to peers (User Story 8); `capacity()`, `used()`, eviction and `clear()` cover only the cache.
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P7: 001-memory-tier / Implementation Notes + SAFETY comment (D4)

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: immutable state fields (`pool_ptr`, `pool_size`, `pool_id`); all mutable access is serialized through the `RwLock<Pool>`
- Code does: cache_size is also immutable after init; the reserve is serialized by Mutex<Bounce>; leased bytes are owned by the lease holder
- Location: `components/memory-tier/src/lib.rs:91-115`

`components/memory-tier/specs/001-memory-tier/spec.md`:271

Old:
```
After initialization the immutable state fields (`pool_ptr`, `pool_size`, `pool_id`) do not change;
```
New:
```
After initialization the immutable state fields (`pool_ptr`, `pool_size`, `cache_size`, `pool_id`) do not change;
```

`components/memory-tier/specs/001-memory-tier/spec.md`:272

Old:
```
and all mutable access is serialized through the `RwLock<Pool>`.
```
New:
```
and all mutable allocator state is serialized: the cache's through the `RwLock<Pool>`, the bounce reserve's through `Mutex<Bounce>`; the bytes of a lease belong to its holder until `bounce_free`.
```

`components/memory-tier/src/lib.rs`:111-113 (comment only)

Old:
```
// RwLock<Pool> serializes access. Immutable fields (pool_ptr, pool_size, spdk_allocated)
```
New:
```
// RwLock<Pool> and Mutex<Bounce> serialize access. Immutable fields (pool_ptr, pool_size, cache_size, spdk_allocated)
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P8: 001-memory-tier / IMemoryTier docs (D5; also completes 2026-10-14 P2)

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: capacity: 'Return the total pool capacity in bytes.'; pool_info: 'for CUDA host registration'; bounce_alloc/NeverFits omit zero length; bounce_alloc omits NotInitialized
- Code does: capacity = pool less reserve; pool_info includes the reserve; len == 0 is NeverFits; NotInitialized before init
- Location: `components/memory-tier/src/lib.rs:625-650,684-706`

`components/interfaces/src/imemory_tier.rs`:197

Old:
```
        /// Return the total pool capacity in bytes.
```
New:
```
        /// Return the bytes the cache may use: the pool size less the bounce
        /// reserve (see [`set_bounce_reserve`](Self::set_bounce_reserve)).
```

`components/interfaces/src/imemory_tier.rs`:203

Old:
```
        /// Return the base pointer and size of the pool for CUDA host registration.
```
New:
```
        /// Return the base pointer and size of the whole pool, bounce reserve
        /// included, for CUDA host and RDMA registration.
```

`components/interfaces/src/imemory_tier.rs`:227-229

Old:
```
        /// now; `NeverFits` if they never can — larger than the reserve, no
        /// reserve, or a pool that is not DMA-capable.
```
New:
```
        /// now; `NeverFits` if they never can — zero, larger than the reserve, no
        /// reserve, or a pool that is not DMA-capable; `NotInitialized` before
        /// `initialize`.
```

`components/interfaces/src/imemory_tier.rs`:108-109

Old:
```
    /// Can never be satisfied: larger than the whole reserve, no reserve, or a
    /// pool the device cannot DMA into. Retrying cannot help.
```
New:
```
    /// Can never be satisfied: zero length, larger than the whole reserve, no
    /// reserve, or a pool the device cannot DMA into. Retrying cannot help.
```

**Note**: This branch already edits imemory_tier.rs, so every component re-stamps anyway; the deferral reason for 2026-10-14 P2 (insert / NotInitialized docs) no longer applies, so it can ride along. Re-stamp all components LAST.

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P9: 001-memory-tier / US8 bounce_free not-live case (D6)

**Direction**: ALIGN  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: bounce_free(lease) panics on a lease that is not live, or whose length differs
- Code does: both panic (lib.rs:724-731); only the wrong-length case is tested (lib.rs:1026-1030)

**Code change**: Add `#[should_panic(expected = "not a live lease")] fn freeing_a_lease_twice_is_loud()` (alloc, free, free again) and list it in US8 Test Coverage.

`components/memory-tier/specs/001-memory-tier/spec.md`:135

Old:
```
  `freeing_with_the_wrong_length_is_loud`, `clear_empties_the_cache_and_leaves_the_reserve`
```
New:
```
  `freeing_with_the_wrong_length_is_loud`, `freeing_a_lease_twice_is_loud`, `clear_empties_the_cache_and_leaves_the_reserve`
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P10: 001-memory-tier / Test comment cites wrong FR (D7)

**Direction**: ALIGN  
**Confidence**: HIGH  
**Severity**: trivial

- Location: `components/memory-tier/src/lib.rs:953`

`components/memory-tier/src/lib.rs`:953

Old:
```
    /// FR-012: a full cache cannot reach the reserve, and leasing never evicts.
```
New:
```
    /// FR-031 (and certus-transfer-endpoint FR-012): a full cache cannot reach
    /// the reserve, and leasing never evicts.
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P11: 001-memory-tier / tasks.md / plan.md inventories (D8)

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: minor

- Spec says: tasks.md:13 '12 unit tests in lib.rs'; plan.md:111-126 lists 12 lib.rs tests, no reserve
- Code does: 20 tests in lib.rs (8 bounce), 1 regression, 9 allocator = 30; all pass (default and --features telemetry)

`components/memory-tier/specs/001-memory-tier/tasks.md`:13

Old:
```
- [ ] Confirm all 12 unit tests in lib.rs and the 1 regression test in `repair_test_insert_track_failure.rs` pass (`cargo test -p memory-tier`)
```
New:
```
- [x] Confirm all 20 unit tests in lib.rs (8 of them for the bounce reserve) and the 1 regression test in `repair_test_insert_track_failure.rs` pass (`cargo test -p memory-tier`; 30 passed with default features and with `--features telemetry`, 2026-10-21)
```

`components/memory-tier/specs/001-memory-tier/plan.md`:126

Old:
```
| `peek_does_not_update_eviction_order` | Peek does not promote |
```
New:
```
| `peek_does_not_update_eviction_order` | Peek returns the entry (order unchecked) |
| `the_default_reserve_is_taken_quietly_and_capped` | 16 MiB default; capped at pool/8; pool_info covers the reserve |
| `the_reserve_is_set_before_initialize_only` | Whole pages; error after init; zero = no reserve |
| `the_cache_and_the_reserve_never_share_bytes` | Full cache cannot reach the reserve; leasing never evicts |
| `leases_are_transient_until_freed_and_coalesce` | NoSpace when full; freed leases coalesce |
| `a_pool_that_is_not_dma_capable_never_fits` | NeverFits on an mmap pool |
| `a_lease_larger_than_the_reserve_never_fits` | NeverFits for oversize/zero; NotInitialized before init |
| `freeing_with_the_wrong_length_is_loud` | bounce_free panics on a length mismatch |
| `clear_empties_the_cache_and_leaves_the_reserve` | clear leaves leases alone |

The regression test `insert_fails_cleanly_when_policy_refuses_to_track` is in `src/repair_test_insert_track_failure.rs`.
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P12: 001-memory-tier / U3 pretend_dma + U4 set_bounce_reserve error variant

**Direction**: BACKFILL  
**Confidence**: MEDIUM  
**Severity**: minor

- Code does: set_bounce_reserve after init returns MemoryTierError::AllocationFailed (lib.rs:676-678); US8 tests lease from an mmap pool via #[cfg(test)] pretend_dma set in setup() (lib.rs:103-106,690-693,778)

`components/memory-tier/specs/001-memory-tier/spec.md`:122

Old:
```
- `set_bounce_reserve(bytes)` overrides it before `initialize` and is an error afterwards;
```
New:
```
- `set_bounce_reserve(bytes)` overrides it before `initialize` and returns `AllocationFailed` afterwards;
```

`components/memory-tier/specs/001-memory-tier/spec.md`:135: if P9 is also approved, apply P9's edit to this line first

Old:
```
  `freeing_with_the_wrong_length_is_loud`, `clear_empties_the_cache_and_leaves_the_reserve`
```
New:
```
  `freeing_with_the_wrong_length_is_loud`, `clear_empties_the_cache_and_leaves_the_reserve`
- Unit tests run on an mmap pool; a test-only `pretend_dma` flag lets it lease as if DMA-capable
```

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P13: 001-memory-tier / U5 reserve taken on a non-DMA pool

**Direction**: HUMAN_DECISION  
**Confidence**: MEDIUM  
**Severity**: minor

- Code does: on an mmap pool initialize still carves the reserve (default 16 MiB) out of the cache, though every bounce_alloc there returns NeverFits
- Location: `components/memory-tier/src/lib.rs:347-348,694-700`

- **Option A**: Keep it (current). Document in FR-031: 'the reserve is taken whether or not the pool is DMA-capable'. Simple; capacity is independent of the SPDK env.
- **Option B**: ALIGN: take a zero reserve when !spdk_allocated (cfg(test) pretend_dma aside), so mmap pools keep the full cache. capacity() then depends on the allocation path.

Questions:
- Is a node without SPDK ever expected to serve block-tier entries to peers?
- Should capacity() be the same for a given pool_size regardless of SPDK?

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P14: 001-memory-tier / Sync-artifact maintenance (D9 + carried FR-013/SC-3/U2)

**Direction**: BACKFILL  
**Confidence**: HIGH  
**Severity**: trivial

- Spec says: align-tasks.md:52,57 and proposals.md:284 name the disconnected-receptacle draft 'FR-031'; align-tasks line refs are at 78f60ab8
- Code does: FR-031 is now the bounce reserve; the evict_next/touch/unwrap sites moved (evict_next lib.rs:492-524, touches :465-466,573-574,611-612, unwraps :387,445,488,498,538,569,659)

`components/memory-tier/.specify/sync/align-tasks.md`:52,57

Old:
```
(proposed FR-031)
```
New:
```
(proposed FR-033; FR-031/FR-032 are now the bounce reserve)
```

`components/memory-tier/.specify/sync/align-tasks.md`:14-15,27-30,55

Old:
```
(descriptive) every src/lib.rs line reference in the FR-013, SC-3 and P9 entries, as of 78f60ab8
```
New:
```
the c9429de6 line numbers listed in this proposal's code_does
```

**Note**: Not a spec change; FR-013, SC-3 (ALIGN) and U2/P9 (decision) remain open and are unchanged in substance.

**Approved**: false

- [ ] Approve
- [ ] Reject
- [ ] Modify

---
