# memory-tier — Creusot properties (clean-slate re-run, skill shape)

**Role 2/3 output.** Every id below is from `verif_memory-tier_property_inventory.md`
(M=54 properties, 101 attachments, N=17 methods). Verdicts here were produced by
**re-authoring contracts from that inventory** onto standalone mirrors of
`src/allocator.rs` + `src/lib.rs`, then running `cargo creusot` to green and
fault-injecting for non-vacuity. Provenance rule honoured: a property is **P**
only where the contract was authored **and** the VCs discharged in *this* worktree
(`verif/creusot/memory-tier-rerun`).

- Headline: `Proved (43 files) ✔` — **40 proof functions, 71 leaf VCs (Z3), 27 why3 sessions**, 3 zero-VC clone files.
- Legend: **P** proved natively · **⊘** tool boundary (contract-run-failure captured, or no dischargeable logical content: raw pointer / atomic / FFI / feature-gated) · **⤴** delegated across a real interface boundary (owned by another component).

## Global-invariant ledger (proved once, propagated by id)

| rank | id | attach | verdict | proof functions (leaf VCs) |
|---|---|---|---|---|
| 1 | INIT-GATE | 16 | **P** | gate_mutator_result (1), gate_mutator_option (1), gate_readonly_option (1), gate_readonly_scalar (1), gate_noop (1) — 5 shapes cover all 16 non-`initialize` methods |
| 2 | USED-LE-CAP | 7 | **P** | alloc_admits (2), allocate_split (4) |
| 3 | USED-CONSERVE | 6 | **P** | deallocate_used (1), allocate_split (4), deallocate_merge (3), lifecycle_alloc_free (5) |
| 4 | CONTAINS-REFLECTS | 6 | **P** | contains_reflects_insert (1), _insert_len (1), _remove (1), _absent (0), _clear (1), _trace (7) — logic-level `FMap<u64,SlotMirror>` |
| 5 | COALESCE | 5 | **P** | coalesce_prev (7), coalesce_next (3), deallocate_merge (3) |
| 6 | ALIGN-4K | 4 | **P** | align_up (15), leftover_offset_aligned (4), align_up_idempotent (2) |
| 7 | SPACE-REUSE | 4 | **P** | space_reuse_free_grows (1), space_reuse_two_cycles (3), lifecycle_alloc_free (5) |
| 8 | PTR-IN-BOUNDS | 4 | **P** | ptr_in_bounds (1) |
| 9 | CAP-CONST | 2 | **P** | initialize_state (1), mutator_preserves_init (1) |
| 10 | FREECAP-DERIVED | 2 | **P** | free_capacity (1) |
| — | INIT-MONOTONIC | (supports #1) | **P** | initialize_state (1), mutator_preserves_init (1) |

All 11 global invariants **P** natively. These are standalone mirrors: a green VC
covers the mirrored arithmetic/branching, not the shipped `HashMap`/`AtomicBool`/
raw-`*mut u8`/FFI function. Binding the mirror & the `FMap` model to the shipped
`std::HashMap`/`FreeList` is the trusted step (boundary ledger below).

## Per-method verdict (N=17, keyed to inventory ids)

| # | method | verdict | proved obligations (native) | boundaries |
|---|--------|---------|-----------------------------|-----------|
| 1 | initialize | **P** (default build) | INIT-ZERO, INIT-DOUBLE, INIT-CAPACITY, INIT-EMPTY, INIT-MONOTONIC, INIT-ERR-NO-STATE-CHANGE, INIT-EP-NOT-CONNECTED, CAP-CONST | INIT-SPDK-ALLOC-FAILED ⊘ (feature `spdk`/FFI) |
| 2 | insert | **P** (core) | INSERT-ZERO, INSERT-DEDUP, INSERT-DEDUP-NO-ALLOC, ALIGN-4K, USED-LE-CAP, USED-CONSERVE, COALESCE, SPACE-REUSE, PTR-IN-BOUNDS, CONTAINS-REFLECTS, INIT-GATE | INSERT-POOLFULL ⊘ (free-region first-fit search over trusted BTreeMap), INSERT-SUCCESS raw-ptr write ⊘ |
| 3 | get | **P** (partial) | GET-ABSENT-NONE, GET-NO-MUTATION, ALIGN-4K, PTR-IN-BOUNDS, INIT-GATE | GET-ROUNDTRIP raw-ptr identity ⊘, GET-TOUCHES ⤴ (eviction-policy) |
| 4 | peek | **P** (partial) | PEEK-ABSENT-NONE, PEEK-NOTOUCH, ALIGN-4K, PTR-IN-BOUNDS, INIT-GATE | PEEK-RETURNS raw-ptr ⊘ |
| 5 | evict_next | **P** (partial) | EVICT-FREES, EVICT-EMPTY-NONE, EVICT-DEALLOC-GUARDED, USED-CONSERVE, COALESCE, SPACE-REUSE, CONTAINS-REFLECTS, INIT-GATE | victim selection ⤴ (eviction-policy), TELEM-EVICTCOUNT ⊘ (feature) |
| 6 | evict_next_for_key | **P** | EVICTKEY-ALIAS (key ignored, == evict_next) | inherits evict_next's ⤴/⊘ |
| 7 | oldest_keys | **P** (partial) | OLDEST-GUARD-EMPTY, OLDEST-NOREMOVE, INIT-GATE | OLDEST-BOUNDED, OLDEST-ORDER ⤴ (eviction-policy, flag #2) |
| 8 | remove | **P** | REMOVE-FREES, REMOVE-NOTFOUND, USED-CONSERVE, COALESCE, CONTAINS-REFLECTS, INIT-GATE | — |
| 9 | touch | **P** (partial) | TOUCH-ABSENT-NOOP, TOUCH-NODATA, INIT-GATE | TOUCH-UPDATES ⤴ (ep.touch) |
| 10 | batch_touch | **P** (partial) | BATCHTOUCH-ALL, BATCHTOUCH-SKIP (element), BATCHTOUCH-EMPTY, BATCH-EP-NOT-CONNECTED, BATCH-NO-MUTATION, INIT-GATE | whole-batch fold + ep handles ⤴ (trusted iteration) |
| 11 | contains | **P** | CONTAINS-REFLECTS, INIT-GATE | — |
| 12 | capacity | **P** | CAP-CONST, USED-LE-CAP, FREECAP-DERIVED, INIT-GATE | — |
| 13 | used | **P** | USED-LE-CAP, USED-CONSERVE, FREECAP-DERIVED, INIT-GATE | — |
| 14 | pool_info | **P** (partial) | ALIGN-4K, PTR-IN-BOUNDS, INIT-GATE | POOLINFO-RETURNS raw-ptr ⊘ |
| 15 | is_dma_capable | ⊘ (unique) | INIT-GATE | DMA-IFF-SPDK ⊘ (trusted atomic-flag read; no dischargeable logic) |
| 16 | clear | **P** | CLEAR-EMPTIES, CLEAR-COUNT, USED-CONSERVE, COALESCE, CONTAINS-REFLECTS, INIT-GATE | — |
| 17 | telemetry_snapshot | **P** (default build) | TELEM-SNAPSHOT-ZERO (feature off) | live counters ⊘ (feature+atomics), TELEM-EVICTCOUNT ⊘ |

## Scores

- **Strict (native-proved, all lanes) = 5/17**: capacity, used, contains, remove, clear (zero ⊘/⤴/feature rows). Adding the two default-build-strict methods (initialize, telemetry_snapshot — every default-build obligation native; only the `spdk`/telemetry-on feature paths ⊘) gives **7/17** under a default-build reading.
- **Relaxed = 16/16**: denominator drops `is_dma_capable` (sole substantive obligation DMA-IFF-SPDK is a trusted atomic-flag read). All 16 remaining methods have every owned obligation P or a disclosed ⊘/⤴ boundary, none left ⧗, and there is **no open code-vs-spec gap**.

## Boundary ledger (`#[trusted]` / mirror / model — what is NOT proved)

- **Whole-function mirrors** of `allocator.rs`/`lib.rs`: the crate cannot build under Creusot (`BTreeMap`/`HashMap`, `AtomicBool`/`RwLock`, raw `*mut u8`, `libc`/SPDK). Each mirror transcribes the arithmetic/branching once the trusted container lookups & atomic loads have produced their scalar/bool results. Green covers the **mirror**; the mirror↔source correspondence is checked by inspection against the cited line ranges.
- **`FMap<u64,SlotMirror>` model** of the `HashMap` slot map: CONTAINS-REFLECTS proved on the model; binding it to `std::HashMap` insert/remove/contains/get is trusted.
- **Raw-pointer identity/validity** (GET-ROUNDTRIP, PEEK-RETURNS, POOLINFO-RETURNS, INSERT-SUCCESS): out of Creusot's model → ⊘.
- **Atomic/RwLock protocol** (`initialized`, `spdk_allocated` reads; interleavings): modelled as plain `bool`; concurrency itself is out of scope → Loom, not Creusot.
- **Feature-gated paths** (`spdk` alloc, telemetry-on counters): ⊘ in the default build.
- **Eviction-policy delegation** (⤴): victim selection, oldest ordering/bound, ep.touch — owned by the eviction-policy component across the `IEvictionPolicy` receptacle.

## Flags (surfaced to spec/interface owners — no shared crate modified)

1. **`evict_next_for_key` doc divergence** — the shared `interfaces/src/imemory_tier.rs` doc-comment still promises same-shard eviction; code is a pure alias (key ignored). Property is consistent spec+code; the doc is stale. **`components/interfaces/` was NOT modified.**
2. **Two undocumented code-only error cases** proved here — INIT-EP-NOT-CONNECTED, BATCH-EP-NOT-CONNECTED (receptacle absent). Recommend the spec record them.
3. **`oldest_keys` order/bound** live in eviction-policy (⤴), not memory-tier — memory-tier owns only OLDEST-GUARD-EMPTY / OLDEST-NOREMOVE.
