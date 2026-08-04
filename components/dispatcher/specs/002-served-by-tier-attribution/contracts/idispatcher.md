# Interface Contract: IDispatcher — `served_by` delta

**Crate**: `interfaces` | **Feature gate**: `spdk`

This documents **only what feature 002 changes** on `IDispatcher`. The full trait surface is
`components/dispatcher/specs/001-dispatcher-cache-interface/contracts/idispatcher.md`, which
remains authoritative for everything not listed here. This is the sole `interfaces`-crate
change for 002 and lands as its own commit ahead of the implementations.

## New types

```rust
/// Which tier served a looked-up key, or why it was not served.
///
/// Describes the *route taken*, not the entry's residency afterwards: a key read
/// from SSD and promoted into DRAM as part of being served is `Ssd`, because that
/// is what the request cost.
///
/// Exactly one value applies to every looked-up key; there is no "unknown".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServedBy {
    /// Local memory tier (DRAM) hit. Data was already resident.
    Dram,
    /// Local SSD. The block was read from a data drive to serve this request,
    /// whether it was then promoted into DRAM (`dispatcher`) or delivered
    /// straight to the GPU with an asynchronous DRAM backfill (`dispatcher-p2p`).
    Ssd,
    /// A peer served it, and the peer advertised it as memory-tier resident.
    RemoteDram,
    /// A peer served it, and the peer advertised it as SSD resident — meaning the
    /// peer had to read its own SSD. The fabric transfer itself is always out of
    /// the peer's DRAM.
    RemoteSsd,
    /// The key was not found in any tier, local or remote.
    Miss,
    /// The key was present, but at a different size than requested. Distinct from
    /// `Miss`: the key exists.
    SizeMismatch,
    /// The lookup was attempted and failed for a reason other than absence or size
    /// mismatch. Deliberately flat — it does not record which tier was attempted.
    Error,
}

impl ServedBy {
    /// True for the four values in which data reached the caller's destination.
    pub fn is_hit(&self) -> bool;
}

/// The per-key outcome of a batched lookup: what happened, and where it came from.
///
/// `served_by` is always meaningful, including on the failure paths — which is what
/// makes "every request lands in exactly one bucket" enforceable.
#[derive(Debug, Clone)]
pub struct LookupOutcome {
    pub served_by: ServedBy,
    pub result: Result<(), DispatcherError>,
}
```

## Changed method

```rust
/// Batch lookup: retrieve multiple cache entries concurrently.
///
/// Returns one `LookupOutcome` per input entry, in the same order. Each carries
/// both the result and the tier that served it.
fn batch_lookup(
    &self,
    entries: &[(CacheKey, Vec<IpcHandle>)],
) -> Vec<LookupOutcome>;
```

Previously:

```rust
fn batch_lookup(
    &self,
    entries: &[(CacheKey, Vec<IpcHandle>)],
) -> Vec<Result<(), DispatcherError>>;
```

### Why a struct rather than `Vec<Result<ServedBy, DispatcherError>>`

The obvious encoding — putting the tier on the `Ok` arm — cannot express `Miss`,
`SizeMismatch`, or `Error`, because those are exactly the cases where the result is `Err`.
It would push the non-hit third of the taxonomy into a `DispatcherError`-to-tier mapping
implemented separately in each of the two servers, which is how the two would drift.
Keeping `served_by` unconditional makes the total-accounting invariant (FR-002, FR-024) a
property of one type rather than of two call sites.

## Invariants

Implementations MUST uphold, and tests MUST verify:

1. **Length and order.** `result.len() == entries.len()`, and index *i* corresponds to
   `entries[i]`.
2. **Totality.** Every `LookupOutcome` carries one of the seven values. There is no eighth
   value and no sentinel.
3. **Hit agreement.** `served_by.is_hit()` if and only if `result.is_ok()`.
4. **`Miss` ⇔ absence.** `served_by == Miss` if and only if `result` is
   `Err(DispatcherError::KeyNotFound(_))`, after any remote attempt has been made.
5. **`SizeMismatch` ⇔ size disagreement.** Reported when and only when the dispatch map
   returned `LookupResult::MismatchSize` for that key.
6. **Overwrite coherence.** Whenever an implementation rewrites `result` after first
   assigning it — the failed-batched-sync path and the concurrent-promotion recovery pass
   both do — it MUST rewrite `served_by` in the same step.
7. **Route, not residency.** A key promoted into DRAM in the course of being served is
   `Ssd`. A key served out of DRAM after waiting for another thread's promotion is `Dram`.
8. **Remote precedence.** A key that missed locally and was then served remotely reports
   only its remote tier. It is never reported as both a miss and a hit.
9. **Cost.** Attribution introduces no lock, no per-key allocation, and no additional
   `IDispatchMap` call, and does not alter outcome, latency, or ordering.

## Attribution sites

Every site in `batch_lookup` that assigns a result must assign an attribution. For
`components/dispatcher` as of this feature's baseline:

| Site | `dispatcher/src/lib.rs` | `served_by` |
| --- | --- | --- |
| `LookupResult::NotExist` | :2090 | `Miss`, unless a later remote attempt supersedes it |
| `LookupResult::MismatchSize` | :2093 | `SizeMismatch` |
| `LookupResult::MemoryTier` warm hit | :2099 | `Dram` |
| `LookupResult::BlockDevice` → cold promote | :2128, served :2167-2465 | `Ssd` |
| Cold sub-path: no drives bound | :2178-2204 | `Ssd` |
| Cold sub-path: pooled read | :2313-2330 | `Ssd` |
| Cold sub-path: inline fallback | :2331 | `Ssd` |
| Cold sub-path: staging post-pass | :2455 | `Ssd` |
| Remote delivery | :2516-2576 | `RemoteDram` / `RemoteSsd` from the peer's advertisement |
| Remote fetch failed | :2519-2522 | `Miss` if no peer held it, else `Error` |
| Failed batched sync (overwrite) | :2588-2590 | `Error` |
| Concurrent-promotion recovery (overwrite) | :2615-2620 | `Dram` on success |
| Init / receptacle failure, whole batch | :2017-2042 | `Error` |

`components/dispatcher-p2p` mirrors this at `dispatcher-p2p/src/lib.rs:1618-2041`, with two
differences: its cold path is SSD → GPU BAR1 ring → D2D with **no synchronous DRAM
promotion** (still `Ssd` — see spec FR-014), and it rejects multi-region N>1 requests at
`:1624` (`Error` — see spec FR-015).

## Related interface deltas

`IRemoteLookup::batch_lookup` must carry the peer's advertised tier out so the two remote
values can be assigned. That delta is specified in `contracts/served-by.md` alongside the
proto surface, because the same taxonomy crosses both boundaries.

## Migration

Compiler-enforced; nothing silently keeps working. Implementors:
`components/dispatcher/src/lib.rs:1631`, `components/dispatcher-p2p/src/lib.rs:1121`,
`components/remote-lookup/src/seams.rs:629` (`unimplemented!`),
`apps/certus-server/src/service.rs:1093` (test mock). Production call sites:
`apps/certus-server-yaml/src/service.rs:420`, `apps/certus-server/src/service.rs:441`. Plus
roughly fourteen call sites in the two dispatchers' own test suites.

The two mocks are the risk, not the two implementations. A mock that reports a plausible
tier without modelling residency makes every attribution assertion vacuous — the exact
failure mode `MockDispatchMap` had for pin accounting, where two compensating infidelities
made the tests pass while asserting nothing. FR-028 exists for this reason: each attribution
test must be observed to fail against a deliberately wrong attribution before it is trusted.
