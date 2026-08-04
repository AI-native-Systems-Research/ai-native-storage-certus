# Contract: Serving-Tier Taxonomy and Its gRPC Surface

**Version**: 1
**Status**: Draft
**Producers**: `components/dispatcher`, `components/dispatcher-p2p` (via `components/remote-lookup` for the two remote values)
**Consumers**: `apps/certus-server`, `apps/certus-server-yaml`, and any `Lookup` client

This is the normative reference for what each attribution value means and how it crosses the
gRPC boundary. The Rust-side interface delta is `contracts/idispatcher.md`.

## The taxonomy

Seven meaningful values. Every looked-up key gets exactly one.

| Value | Data delivered? | Meaning |
| --- | --- | --- |
| `DRAM` | yes | Local memory tier hit; the block was already resident. |
| `SSD` | yes | A local data drive was read to serve this request. |
| `REMOTE_DRAM` | yes | A peer served it and advertised it as memory-tier resident. |
| `REMOTE_SSD` | yes | A peer served it and advertised it as SSD resident — i.e. the peer had to read its own disk. |
| `MISS` | no | Not found in any tier, local or remote. |
| `SIZE_MISMATCH` | no | Present, but at a different size than requested. |
| `ERROR` | no | Attempted and failed for some other reason. |

Three properties of this taxonomy are load-bearing and easy to get wrong:

1. **It describes the route, not the residency.** In `dispatcher`, an SSD hit is promoted
   into DRAM as part of being served, so "served from SSD" and "now in DRAM" are both true of
   one request. `SSD` is the honest answer because it is what the request cost.
2. **`REMOTE_SSD` does not mean data crossed the fabric from disk.** The RDMA read is always
   out of the responding peer's DRAM; a disk-tier key is promoted into the peer's memory tier
   *before* the transfer. `REMOTE_SSD` therefore means "a peer's disk read was on this
   request's critical path."
3. **`REMOTE_SSD` is a first-touch property.** Because serving from disk leaves the entry in
   the peer's DRAM, the next remote fetch of the same key is expected to report
   `REMOTE_DRAM`. A fixed holder configuration does not produce a stable `REMOTE_SSD`
   fraction, and any test or report that assumes otherwise is mis-specified.

### Hits versus non-hits

`DRAM`, `SSD`, `REMOTE_DRAM`, `REMOTE_SSD` are hits. `MISS`, `SIZE_MISMATCH`, `ERROR` are not.
A hit is reported if and only if the lookup succeeded, so a consumer can compute an object
hit rate as `hits / total` without needing to know the error taxonomy.

### Why `SIZE_MISMATCH` is not folded into `MISS`

The cache *has* the key; it does not have it at the requested size. Reporting `MISS` would
conflate "you must populate this from scratch" with "your size model disagrees with what is
stored", which are different problems for a caller. Keeping it separate also means this
feature changes no dispatcher behaviour: a size mismatch currently yields
`InvalidParameter`, and because the remote-delivery pass selects only `KeyNotFound`, such
keys are never offered to peers. Folding it into `MISS` would have made size-mismatched keys
remote-eligible — a behaviour change smuggled in under an attribution feature.

### Why `ERROR` exists

A lookup that was attempted and failed is neither a hit nor a miss. Without a bucket for it,
"every request is attributed" is false, and the aggregate counters cannot be made to sum to
the request count — which is precisely today's defect: the server increments `lookup_misses`
only for `KEY_NOT_FOUND` and counts every other failure as neither.

`ERROR` is deliberately flat. It does not record which tier was being attempted when the
failure occurred. That refinement is deferred; it is not needed to measure hit rate.

## gRPC surface

### Enum

Added to both `apps/certus-server/proto/dispatcher.proto` and
`apps/certus-server-yaml/proto/dispatcher.proto`, identically. Naming follows the existing
`ErrorCode` convention in the same file (prefix repeated on each value, explicit
`_UNSPECIFIED = 0`).

```protobuf
// Which tier served a looked-up entry, or why it was not served.
//
// Describes the route the request took, not where the entry resides afterwards:
// a block read from SSD and promoted into DRAM while serving is SERVED_BY_SSD.
//
// A conforming server never emits SERVED_BY_UNSPECIFIED. Clients observing it are
// talking to a server that predates serving-tier attribution.
enum ServedBy {
  SERVED_BY_UNSPECIFIED = 0;
  // Local memory tier hit; already resident.
  SERVED_BY_DRAM = 1;
  // A local data drive was read to serve this request.
  SERVED_BY_SSD = 2;
  // A peer served it, advertising memory-tier residency.
  SERVED_BY_REMOTE_DRAM = 3;
  // A peer served it, advertising SSD residency: the peer read its own disk.
  // The fabric transfer itself is always out of the peer's DRAM.
  SERVED_BY_REMOTE_SSD = 4;
  // Not found in any tier, local or remote.
  SERVED_BY_MISS = 5;
  // Present, but at a different size than requested.
  SERVED_BY_SIZE_MISMATCH = 6;
  // Attempted and failed for some other reason.
  SERVED_BY_ERROR = 7;
}
```

### Field

`EntryResult` currently uses field numbers 1-4, so the addition takes 5:

```protobuf
message EntryResult {
  uint64 key = 1;
  bool success = 2;
  ErrorCode error_code = 3;
  string error_message = 4;
  // Which tier served this entry. Populated on Lookup responses; see
  // "Scope on other RPCs" below for the other RPCs that reuse this message.
  ServedBy served_by = 5;
}
```

### Compatibility

- **Adding a field and an enum is a backward- and forward-compatible proto3 change.** An old
  client decoding a new server's response ignores field 5. A new client decoding an old
  server's response sees the field absent and reads the proto3 default, `0` —
  `SERVED_BY_UNSPECIFIED`.
- **That default is the version-detection mechanism**, which is why the zero value must exist
  and must never be emitted. A client seeing `SERVED_BY_UNSPECIFIED` on a successful lookup
  knows the server predates attribution and must report "attribution unsupported" rather than
  guessing a tier.
- **A conforming server never emits it.** This is a server-side obligation with no wire
  enforcement, so it needs a test: assert no `Lookup` response carries the zero value.
- **The Rust change is compiler-enforced.** Both servers construct `EntryResult` through
  fully-exhaustive struct literals with no `..Default::default()` — two literals per server,
  inside a `success_result`/`error_result` helper pair that funnels 22 success and 47 error
  call sites. So exactly four literals must gain the field and the compiler finds them all.
  Only two call sites need a *real* tier rather than a default: the `success_result` call in
  each server's `lookup` handler. The other twenty belong to RPCs that reuse the message.
- **Nothing else in Rust is at risk.** All four `tonic_build` consumers regenerate at build
  time; no `.pb.rs` is checked in. Clients read `EntryResult` by field access, never by
  exhaustive match or full-struct equality.
- **Python is not compiler-enforced and must be handled deliberately.** Three sets of
  *checked-in* generated stubs exist, all produced by hand-run `generate_pb.sh` scripts
  reading `apps/certus-server/proto/dispatcher.proto`. They will keep working un-regenerated
  (additive field, and every consumer reads by attribute), but they will not expose
  `served_by` until regenerated. Two of the three are **already stale on unrelated messages**,
  so regenerating them pulls in drift this feature did not cause — that must be a separate,
  visible step rather than a silent side effect. One of these stub sets is also copied to
  remote nodes by the multi-node test script, so staleness propagates to the cluster.
- **There is no proto lint or compatibility gate in CI** — no `buf`, no `protolock`, and no
  proto reference in the Jenkinsfile or the GitHub workflows. Compatibility here is a review
  obligation, not an automated one.

### Scope on other RPCs

`EntryResult` is reused as the result type of ten RPCs (`Populate`, `Lookup`, `Remove`,
`Touch`, `Reserve`, `CopyToStore`, `CommitStore`, `AbortStore`, `Pin`, `Unpin`). `served_by`
is meaningful only for `Lookup`.

The contract is therefore:

- **`Lookup`**: `served_by` MUST be populated on every entry, and MUST NOT be
  `SERVED_BY_UNSPECIFIED`.
- **All other RPCs**: `served_by` is unspecified-by-design and MUST be left at
  `SERVED_BY_UNSPECIFIED`. Consumers MUST NOT read it.

Stating this explicitly is the point. The alternative — a field that is sometimes meaningful
depending on which RPC produced the message — is the kind of ambiguity that gets discovered
by a wrong dashboard six months later. A future feature may extend population to other RPCs
(for example, attributing what a `Touch`-with-promote actually did); until then the field's
silence there is contractual rather than accidental.

## The `IRemoteLookup` delta

The two remote values require the peer's advertised tier to leave `remote-lookup`. Today:

```rust
fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>>;
```

The tier is already known — it arrives as `Avail::{None, Memory, Disk}` in the peer's
KEY_RESPONSE and is retained per peer for the whole operation — but it is discarded at three
points: the result projection reads key state only, key state has no tier dimension, and
`Avail` is not exported into the `interfaces` crate.

The contract this feature requires:

- `IRemoteLookup::batch_lookup` MUST return, per key, the advertised tier of the peer that
  served it, alongside the existing success/failure result.
- The tier MUST be derived from the peer's advertised availability, **never from the
  operation's phase.** Phase and tier are correlated but not equivalent: a peer-DRAM hit can
  finalize in Phase 2, a disk fetch can occur in Phase 1, phase is stored per operation
  rather than per key, and it transitions on quorum and timeout rather than on any tier
  event.
- A key satisfied as a **single-flight follower** owns no landing slot of its own, so its
  tier MUST be taken from the leading operation's record. It MUST NOT be left unattributed.
- A key satisfied via the `AlreadyExists` publish path has a recorded peer that did not fill
  DRAM; its tier MUST NOT be read from that peer's advertisement without validation.
- A peer advertising `Avail::None` is not a holder and contributes no tier.
- **No wire-protocol change.** `WIRE_VERSION` stays at 1 and an unmodified peer remains
  interoperable. This is a hard constraint, not a preference: the codec frames by record
  count with no length prefix and no spare or reserved field, so appending a byte to an
  existing message would mis-align an old decoder from the second record onward and fail
  silently rather than detectably. There is no capability negotiation to gate a change on,
  and bumping the version makes old peers drop every frame as unknown. A *new message type*
  would be compatible where a new field is not — which is the shape any future serve-time
  ground-truth feature must take.

## Internal-to-proto mapping

One-to-one, no reinterpretation at the server boundary. A server MUST NOT infer a tier from
an error code, a latency, or any other proxy; it may only translate what the dispatcher
returned.

| `ServedBy` (Rust) | `ServedBy` (proto) | `success` | Typical `error_code` |
| --- | --- | --- | --- |
| `Dram` | `SERVED_BY_DRAM` | true | — |
| `Ssd` | `SERVED_BY_SSD` | true | — |
| `RemoteDram` | `SERVED_BY_REMOTE_DRAM` | true | — |
| `RemoteSsd` | `SERVED_BY_REMOTE_SSD` | true | — |
| `Miss` | `SERVED_BY_MISS` | false | `ERROR_CODE_KEY_NOT_FOUND` |
| `SizeMismatch` | `SERVED_BY_SIZE_MISMATCH` | false | `ERROR_CODE_INVALID_PARAMETER` |
| `Error` | `SERVED_BY_ERROR` | false | `ERROR_CODE_IO_ERROR` and others |

## Counter reconciliation

The servers' aggregate lookup counters must become consistent with the per-entry field, which
means correcting an existing defect rather than adding to it. Today `lookup_hits` counts
successes and `lookup_misses` counts only `KEY_NOT_FOUND`; every other failure is counted as
neither, so the two do not sum to the number of entries requested.

Required:

- Hits, misses, and errors MUST sum to entries requested, for every batch.
- The aggregate counters MUST agree with the `served_by` values in the same responses.
- Attribution MUST NOT depend on a client draining the eviction event stream. (The existing
  `certus_evictions_total` does have that dependency — it only advances inside the
  `TakeEvents` handler — which is a separate defect, noted and out of scope.)

Whether `SIZE_MISMATCH` is counted under errors or gets its own counter is an implementation
choice for `plan.md`; the invariant is that the sum is complete either way.

Splitting the aggregate counters *by tier* is explicitly not required here. This feature makes
a tiered hit rate computable by a client from the responses; exporting it as server-side
per-tier metrics is observability work belonging to the `certus-server` OTel series, which
today attaches only `op` and `drive` attributes and has no `tier` dimension anywhere.
